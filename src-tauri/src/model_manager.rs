use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const READY_MARKER: &str = ".verified";

struct Artifact {
    name: &'static str,
    url: &'static str,
    bytes: u64,
    sha256: &'static str,
}

const ARTIFACTS: &[Artifact] = &[
    Artifact {
        name: "v3_e2e_ctc.int8.onnx",
        url: "https://huggingface.co/istupakov/gigaam-v3-onnx/resolve/322c3b29492673eb7d0b434bfa9dfb8653e34d02/v3_e2e_ctc.int8.onnx?download=true",
        bytes: 224_893_347,
        sha256: "2e3fcb7a7b66030336fd10c2fcfb033bd1dc7e1bf238fe5cfd83b1d0cfc9d28e",
    },
    Artifact {
        name: "v3_e2e_ctc_vocab.txt",
        url: "https://huggingface.co/istupakov/gigaam-v3-onnx/resolve/322c3b29492673eb7d0b434bfa9dfb8653e34d02/v3_e2e_ctc_vocab.txt?download=true",
        bytes: 2_007,
        sha256: "142de7570b3de5b3035ce111a89c228e80e6085273731d944093ddf24fa539cd",
    },
    Artifact {
        name: "silero_vad.onnx",
        url: "https://raw.githubusercontent.com/snakers4/silero-vad/1e261b036686cd0017d500ee96acd1c4ba572a9d/src/silero_vad/data/silero_vad.onnx",
        bytes: 2_327_524,
        sha256: "1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3",
    },
];

#[derive(Clone, Serialize)]
pub struct ModelStatus {
    pub installed: bool,
    pub downloading: bool,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
}

pub struct ModelManager {
    directory: PathBuf,
    status: Arc<Mutex<ModelStatus>>,
    cancel: Arc<AtomicBool>,
}

impl ModelManager {
    pub fn new(directory: PathBuf) -> Self {
        let installed = generation_valid(&directory);
        Self {
            directory,
            status: Arc::new(Mutex::new(ModelStatus {
                installed,
                downloading: false,
                downloaded_bytes: 0,
                total_bytes: ARTIFACTS.iter().map(|artifact| artifact.bytes).sum(),
                error: None,
            })),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn status(&self) -> ModelStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn directory(&self) -> PathBuf {
        self.directory.clone()
    }

    pub fn start(&self, completion: Option<Sender<bool>>) -> Result<()> {
        let mut status = self.status.lock().unwrap();
        if status.downloading {
            bail!("model download already active");
        }
        status.downloading = true;
        status.downloaded_bytes = 0;
        status.error = None;
        drop(status);
        self.cancel.store(false, Ordering::Relaxed);
        let directory = self.directory.clone();
        let status = self.status.clone();
        let cancel = self.cancel.clone();
        std::thread::spawn(move || {
            let result = download_all(&directory, &status, &cancel);
            if result.is_err() {
                let _ = fs::remove_dir_all(directory.with_extension("staging"));
            }
            let mut state = status.lock().unwrap();
            state.downloading = false;
            state.installed = result.is_ok() && generation_valid(&directory);
            state.error = result.err().map(|error| error.to_string());
            if let Some(completion) = completion {
                let _ = completion.send(state.installed);
            }
        });
        Ok(())
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn download_all(
    directory: &Path,
    status: &Arc<Mutex<ModelStatus>>,
    cancel: &AtomicBool,
) -> Result<()> {
    let staging = directory.with_extension("staging");
    let previous = directory.with_extension("previous");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("GigaAM-Desktop/0.1")
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60 * 30))
        .build()?;
    for artifact in ARTIFACTS {
        if cancel.load(Ordering::Relaxed) {
            bail!("model download cancelled");
        }
        let final_path = staging.join(artifact.name);
        if verify(&final_path, artifact).is_ok() {
            status.lock().unwrap().downloaded_bytes += artifact.bytes;
            continue;
        }
        let partial = staging.join(format!("{}.partial", artifact.name));
        let result = (|| -> Result<()> {
            let mut response = client.get(artifact.url).send()?.error_for_status()?;
            let mut file = fs::File::create(&partial)?;
            let mut hasher = Sha256::new();
            let mut size = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    drop(file);
                    let _ = fs::remove_file(&partial);
                    bail!("model download cancelled");
                }
                let read = response.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                file.write_all(&buffer[..read])?;
                hasher.update(&buffer[..read]);
                size += read as u64;
                status.lock().unwrap().downloaded_bytes += read as u64;
            }
            file.sync_all()?;
            let hash = format!("{:x}", hasher.finalize());
            if size != artifact.bytes || hash != artifact.sha256 {
                let _ = fs::remove_file(&partial);
                bail!("downloaded model failed integrity verification");
            }
            fs::rename(&partial, &final_path)
                .with_context(|| format!("failed to install {}", artifact.name))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&partial);
        }
        result?;
    }
    if !artifacts_valid(&staging) {
        bail!("staged model generation is incomplete");
    }
    if cancel.load(Ordering::Relaxed) {
        let _ = fs::remove_dir_all(&staging);
        bail!("model download cancelled");
    }
    fs::write(staging.join(READY_MARKER), b"verified\n")?;
    let _ = fs::remove_dir_all(&previous);
    if directory.exists() {
        fs::rename(directory, &previous)?;
    }
    if let Err(error) = fs::rename(&staging, directory) {
        if previous.exists() {
            let _ = fs::rename(&previous, directory);
        }
        return Err(error.into());
    }
    let _ = fs::remove_dir_all(previous);
    Ok(())
}

pub fn generation_valid(directory: &Path) -> bool {
    directory.join(READY_MARKER).is_file() && artifacts_valid(directory)
}

fn artifacts_valid(directory: &Path) -> bool {
    ARTIFACTS
        .iter()
        .all(|artifact| verify(&directory.join(artifact.name), artifact).is_ok())
}

fn verify(path: &Path, artifact: &Artifact) -> Result<()> {
    if fs::metadata(path)?.len() != artifact.bytes {
        bail!("size mismatch");
    }
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if format!("{:x}", hasher.finalize()) != artifact.sha256 {
        bail!("hash mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_is_never_installed() {
        let directory =
            std::env::temp_dir().join(format!("gigaam-model-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("v3_e2e_ctc.int8.onnx.partial"), b"partial").unwrap();
        assert!(!artifacts_valid(&directory));
        assert!(!directory.join("v3_e2e_ctc.int8.onnx").exists());
        let _ = fs::remove_dir_all(directory);
    }
}
