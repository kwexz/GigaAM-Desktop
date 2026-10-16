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

struct Artifact<'a> {
    name: &'a str,
    url: &'a str,
    bytes: u64,
    sha256: &'a str,
}

const ARTIFACTS: &[Artifact<'static>] = &[
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
    download_artifacts(directory, status, cancel, ARTIFACTS)
}

fn download_artifacts(
    directory: &Path,
    status: &Arc<Mutex<ModelStatus>>,
    cancel: &AtomicBool,
    artifacts: &[Artifact<'_>],
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
    for artifact in artifacts {
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
    if !artifacts_valid_for(&staging, artifacts) {
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
    artifacts_valid_for(directory, ARTIFACTS)
}

fn artifacts_valid_for(directory: &Path, artifacts: &[Artifact<'_>]) -> bool {
    artifacts
        .iter()
        .all(|artifact| verify(&directory.join(artifact.name), artifact).is_ok())
}

fn verify(path: &Path, artifact: &Artifact<'_>) -> Result<()> {
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
    use std::{
        net::TcpListener,
        sync::mpsc,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_directory(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("gigaam-{name}-{}-{unique}", std::process::id()))
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn serve_once(body: &'static [u8]) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        (format!("http://{address}/artifact"), handle)
    }

    #[test]
    fn partial_file_is_never_installed() {
        let directory = temp_directory("model-partial-test");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("v3_e2e_ctc.int8.onnx.partial"), b"partial").unwrap();
        assert!(!artifacts_valid(&directory));
        assert!(!directory.join("v3_e2e_ctc.int8.onnx").exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn complete_generation_requires_marker_size_and_hash() {
        let directory = temp_directory("model-generation-test");
        fs::create_dir_all(&directory).unwrap();
        let bytes = b"verified artifact";
        let hash = sha256(bytes);
        let artifact = Artifact {
            name: "model.bin",
            url: "unused",
            bytes: bytes.len() as u64,
            sha256: &hash,
        };

        fs::write(directory.join(artifact.name), bytes).unwrap();
        assert!(artifacts_valid_for(
            &directory,
            std::slice::from_ref(&artifact)
        ));
        assert!(!directory.join(READY_MARKER).exists());

        fs::write(directory.join(READY_MARKER), b"verified\n").unwrap();
        assert!(directory.join(READY_MARKER).is_file());
        fs::write(directory.join(artifact.name), b"wrong artifact").unwrap();
        assert!(!artifacts_valid_for(&directory, &[artifact]));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn cancellation_keeps_previous_generation() {
        let directory = temp_directory("model-cancel-test");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("existing.bin"), b"keep me").unwrap();
        let status = Arc::new(Mutex::new(ModelStatus {
            installed: true,
            downloading: true,
            downloaded_bytes: 0,
            total_bytes: 1,
            error: None,
        }));
        let cancel = AtomicBool::new(true);
        let hash = sha256(b"x");
        let artifact = Artifact {
            name: "model.bin",
            url: "http://127.0.0.1:1/not-used",
            bytes: 1,
            sha256: &hash,
        };

        let error = download_artifacts(&directory, &status, &cancel, &[artifact]).unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert_eq!(
            fs::read(directory.join("existing.bin")).unwrap(),
            b"keep me"
        );
        assert!(
            !directory
                .with_extension("staging")
                .join(READY_MARKER)
                .exists()
        );
        let _ = fs::remove_dir_all(&directory);
        let _ = fs::remove_dir_all(directory.with_extension("staging"));
    }

    #[test]
    fn downloads_verifies_and_atomically_replaces_generation() {
        let directory = temp_directory("model-download-test");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("old.bin"), b"old generation").unwrap();
        let body = b"new model generation";
        let hash = sha256(body);
        let (url, server) = serve_once(body);
        let artifact = Artifact {
            name: "model.bin",
            url: &url,
            bytes: body.len() as u64,
            sha256: &hash,
        };
        let status = Arc::new(Mutex::new(ModelStatus {
            installed: false,
            downloading: true,
            downloaded_bytes: 0,
            total_bytes: body.len() as u64,
            error: None,
        }));

        download_artifacts(
            &directory,
            &status,
            &AtomicBool::new(false),
            std::slice::from_ref(&artifact),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(directory.join("model.bin")).unwrap(), body);
        assert!(directory.join(READY_MARKER).is_file());
        assert!(!directory.join("old.bin").exists());
        assert!(!directory.with_extension("previous").exists());
        assert!(artifacts_valid_for(&directory, &[artifact]));
        assert_eq!(status.lock().unwrap().downloaded_bytes, body.len() as u64);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn rejects_download_with_wrong_hash_without_replacing_generation() {
        let directory = temp_directory("model-hash-test");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("old.bin"), b"old generation").unwrap();
        let body = b"downloaded bytes";
        let (url, server) = serve_once(body);
        let artifact = Artifact {
            name: "model.bin",
            url: &url,
            bytes: body.len() as u64,
            sha256: &sha256(b"different bytes"),
        };
        let status = Arc::new(Mutex::new(ModelStatus {
            installed: true,
            downloading: true,
            downloaded_bytes: 0,
            total_bytes: body.len() as u64,
            error: None,
        }));

        let error = download_artifacts(&directory, &status, &AtomicBool::new(false), &[artifact])
            .unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("integrity verification"));
        assert_eq!(
            fs::read(directory.join("old.bin")).unwrap(),
            b"old generation"
        );
        assert!(!directory.join("model.bin").exists());
        let _ = fs::remove_dir_all(&directory);
        let _ = fs::remove_dir_all(directory.with_extension("staging"));
    }

    #[test]
    fn manager_rejects_parallel_downloads() {
        let directory = temp_directory("model-parallel-test");
        let manager = ModelManager::new(directory.clone());
        manager.status.lock().unwrap().downloading = true;
        let (sender, _receiver) = mpsc::channel();
        let error = manager.start(Some(sender)).unwrap_err();
        assert!(error.to_string().contains("already active"));
        let _ = fs::remove_dir_all(directory);
    }
}
