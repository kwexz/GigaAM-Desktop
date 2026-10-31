//! Durable queue scheduling. Network transport is injected for restart tests.
use rusqlite::{Connection, params};

pub struct Outbox { db: Connection }
#[derive(Debug, PartialEq)]
pub struct Job { pub id: String, pub payload: String, pub attempts: u32 }
#[derive(serde::Serialize)]
pub struct QueueStatus { pub pending:u64,pub uploading:u64,pub synced:u64,pub auth_required:u64 }
fn requires_auth(error:&str)->bool {
    error.contains("revoked") || error.contains("expired") || error.contains("Connect Google") || error.contains("invalid_client") || error.contains("invalid_grant")
}
#[derive(serde::Serialize,serde::Deserialize)]
pub struct SnapshotPayload { pub revision:String, pub recording_id:String, pub manifest:serde_json::Value, pub audio:Vec<std::path::PathBuf> }
impl Outbox {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let db=Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS drive_jobs(id TEXT PRIMARY KEY,payload TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'pending',attempts INTEGER NOT NULL DEFAULT 0,next_ms INTEGER NOT NULL DEFAULT 0);")?;
        Ok(Self{db})
    }
    pub fn enqueue(&self,id:&str,payload:&str)->rusqlite::Result<()> {
        self.db.execute("INSERT OR IGNORE INTO drive_jobs(id,payload) VALUES(?1,?2)",params![id,payload])?;Ok(())
    }
    pub fn run_one(&mut self,now:i64,mut upload:impl FnMut(&Job)->Result<(),bool>)->rusqlite::Result<bool>{
        use rusqlite::OptionalExtension;
        // Commit the claim before network I/O so enqueue never waits for upload.
        let tx=self.db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let job=tx.query_row("SELECT id,payload,attempts FROM drive_jobs WHERE (state='pending' OR state='uploading') AND next_ms<=?1 ORDER BY next_ms,id LIMIT 1",[now],|r|Ok(Job{id:r.get(0)?,payload:r.get(1)?,attempts:r.get(2)?})).optional()?;
        let Some(job)=job else {tx.commit()?;return Ok(false)};
        tx.execute("UPDATE drive_jobs SET state='uploading',next_ms=?2 WHERE id=?1",params![job.id,now.saturating_add(3600000)])?;
        tx.commit()?;
        match upload(&job){
            Ok(())=>{self.db.execute("UPDATE drive_jobs SET state='synced' WHERE id=?1",[&job.id])?;}
            Err(auth)=>{
                let attempts=job.attempts.saturating_add(1);
                let delay=1000i64.saturating_mul(1i64<<attempts.min(10)).min(900000);
                self.db.execute("UPDATE drive_jobs SET state=?2,attempts=?3,next_ms=?4 WHERE id=?1",params![job.id,if auth{"auth_required"}else{"pending"},attempts,now.saturating_add(delay)])?;
            }
        }
        Ok(true)
    }
    pub fn resume_after_auth(&self)->rusqlite::Result<()> {self.db.execute("UPDATE drive_jobs SET state='pending',next_ms=0 WHERE state='auth_required'",[])?;Ok(())}
    pub fn recover_interrupted(&self)->rusqlite::Result<()> {self.db.execute("UPDATE drive_jobs SET state='pending',next_ms=0 WHERE state='uploading'",[])?;Ok(())}
    pub fn status(&self)->rusqlite::Result<QueueStatus>{
        let count=|state:&str|self.db.query_row("SELECT count(*) FROM drive_jobs WHERE state=?1",[state],|row|row.get(0));
        Ok(QueueStatus{pending:count("pending")?,uploading:count("uploading")?,synced:count("synced")?,auth_required:count("auth_required")?})
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct UploadPayload {
    pub folder: String,
    pub key: String,
    pub name: String,
    pub path: std::path::PathBuf,
}

pub fn start_worker(root: std::path::PathBuf) {
    static STARTED:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true,std::sync::atomic::Ordering::AcqRel){return;}
    std::thread::spawn(move || {
        let path=root.join("drive-outbox.sqlite");
        if let Ok(queue)=Outbox::open(&path){let _=queue.recover_interrupted();}
        loop {
            if crate::google_drive::connected() {
                if let Ok(mut queue)=Outbox::open(&path) {
                    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|v|v.as_millis() as i64).unwrap_or(0);
                    let _=queue.run_one(now,|job| {
                        let result = if let Ok(snapshot)=serde_json::from_str::<SnapshotPayload>(&job.payload) {
                            upload_recording(&root,&snapshot)
                        } else {
                            let payload:UploadPayload=serde_json::from_str(&job.payload).map_err(|_|true)?;
                            crate::google_drive::upload_snapshot(&payload.folder,&payload.key,&payload.name,&payload.path).map(|_|())
                        };
                        result.map_err(|error|requires_auth(&error))
                    });
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}

fn upload_recording(root:&std::path::Path,snapshot:&SnapshotPayload)->Result<(),String>{
    let folder=crate::google_drive::ensure_folder()?;
    let mut manifest=snapshot.manifest.clone();
    let audio=manifest["audio"].as_array_mut().ok_or("Invalid queued manifest")?;
    if audio.len()!=snapshot.audio.len(){return Err("Invalid queued audio count".into());}
    for(index,path)in snapshot.audio.iter().enumerate(){
        let key=format!("{}:{}:{index}",snapshot.recording_id,snapshot.revision);
        let name=format!("{}-{}-{index}.{}",snapshot.recording_id,snapshot.revision,audio[index]["original_filename"].as_str().and_then(|s|std::path::Path::new(s).extension()).and_then(|s|s.to_str()).unwrap_or("audio"));
        audio[index]["file_id"]=crate::google_drive::upload_snapshot(&folder,&key,&name,path)?.into();
    }
    let path=root.join("drive-payloads").join(format!("{}-manifest.json",snapshot.revision));
    let staged=path.with_extension("json.partial");
    std::fs::write(&staged,manifest.to_string()).map_err(|_|"Manifest staging failed")?;
    std::fs::rename(staged,&path).map_err(|_|"Manifest publication failed")?;
    crate::google_drive::upload_snapshot(&folder,&format!("{}:{}:manifest",snapshot.recording_id,snapshot.revision),&format!("{}-{}.json",snapshot.recording_id,snapshot.revision),&path)?;
    Ok(())
}

pub fn enqueue_recording(root:&std::path::Path,id:&str,mut manifest:serde_json::Value,paths:&[std::path::PathBuf])->Result<(),String>{
    use sha2::{Digest,Sha256};use std::io::Read;
    let revision=crate::new_id();let directory=root.join("drive-payloads").join(&revision);
    std::fs::create_dir_all(&directory).map_err(|_|"Could not create immutable snapshot")?;
    let mut audio=Vec::new();let mut metadata=Vec::new();
    for(index,source)in paths.iter().enumerate(){
        let path=directory.join(index.to_string());
        std::fs::copy(source,&path).map_err(|_|"Could not snapshot source audio")?;
        let mut file=std::fs::OpenOptions::new().read(true).write(true).open(&path).map_err(|_|"Snapshot unavailable")?;
        file.sync_all().map_err(|_|"Snapshot flush failed")?;
        let mut hash=Sha256::new();let mut bytes=0u64;let mut buffer=[0u8;65536];
        loop{let n=file.read(&mut buffer).map_err(|_|"Snapshot checksum failed")?;if n==0{break;}bytes+=n as u64;hash.update(&buffer[..n]);}
        metadata.push(serde_json::json!({"original_filename":source.file_name().and_then(|s|s.to_str()),"bytes":bytes,"sha256":format!("{:x}",hash.finalize())}));audio.push(path);
    }
    if audio.is_empty(){return Err("Original audio missing; no snapshot queued".into());}
    manifest["audio"]=metadata.into();
    manifest["original_recording_id"]=id.into();
    manifest["revision"]=revision.clone().into();
    manifest["device_id"]=device_id(root)?.into();
    let payload=SnapshotPayload{revision:revision.clone(),recording_id:id.into(),manifest,audio};
    Outbox::open(&root.join("drive-outbox.sqlite")).and_then(|queue|queue.enqueue(&revision,&serde_json::to_string(&payload).unwrap())).map_err(|_|"Could not persist snapshot job".into())
}

fn device_id(root:&std::path::Path)->Result<String,String>{
    let path=root.join("drive-device-id");
    if let Ok(value)=std::fs::read_to_string(&path){if !value.trim().is_empty(){return Ok(value);}}
    let id=crate::new_id();
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path){
        Ok(mut file)=>{use std::io::Write;file.write_all(id.as_bytes()).and_then(|_|file.sync_all()).map_err(|_|"Device identity write failed")?;Ok(id)}
        Err(error)if error.kind()==std::io::ErrorKind::AlreadyExists=>std::fs::read_to_string(path).map_err(|_|"Device identity unavailable".into()),
        Err(_)=>Err("Device identity unavailable".into()),
    }
}

pub fn resume(root:&std::path::Path)->Result<(),String>{
    Outbox::open(&root.join("drive-outbox.sqlite")).and_then(|queue|queue.resume_after_auth()).map_err(|_|"Could not resume Drive queue".into())
}

pub fn enqueue_immutable(root:&std::path::Path, folder:&str,key:&str,name:&str,source:&std::path::Path)->Result<(),String>{
    use sha2::{Digest,Sha256};
    let identity=format!("{:x}",Sha256::digest(key.as_bytes()));
    let directory=root.join("drive-payloads");std::fs::create_dir_all(&directory).map_err(|_|"Could not create durable payload directory")?;
    let path=directory.join(&identity);
    if !path.exists(){
        let staged=directory.join(format!("{identity}.{}",crate::new_id()));
        std::fs::copy(source,&staged).map_err(|_|"Could not snapshot backup source")?;
        std::fs::OpenOptions::new().write(true).open(&staged).and_then(|file|file.sync_all()).map_err(|_|"Could not flush backup payload")?;
        std::fs::rename(staged,&path).map_err(|_|"Could not publish backup payload")?;
    }
    let payload=UploadPayload{folder:folder.into(),key:key.into(),name:name.into(),path};
    Outbox::open(&root.join("drive-outbox.sqlite")).and_then(|queue|queue.enqueue(key,&serde_json::to_string(&payload).unwrap())).map_err(|_|"Could not persist upload queue".into())
}

#[cfg(test)]mod tests{
    #[test]fn distinguishes_auth_failures_from_transient_transport(){
        for error in ["Google access expired or was revoked. Reconnect Drive.","Connect Google Drive first","invalid_client","invalid_grant"]{assert!(super::requires_auth(error));}
        for error in ["Drive upload interrupted","Remote verification unavailable","HTTP 429","HTTP 503"]{assert!(!super::requires_auth(error));}
    }
    #[test]fn recording_job_contains_immutable_audio_and_manifest(){
        let root=std::env::temp_dir().join(format!("gigaam-record-job-{}",crate::new_id()));std::fs::create_dir_all(&root).unwrap();let path=root.join("mic.ogg");std::fs::write(&path,b"audio").unwrap();
        super::enqueue_recording(&root,"session",serde_json::json!({"version":1,"segments":[],"tracks":[{"offset_seconds":0.5}]}),&[path.clone()]).unwrap();std::fs::write(path,b"edited").unwrap();
        let mut queue=super::Outbox::open(&root.join("drive-outbox.sqlite")).unwrap();queue.run_one(0,|job|{let snapshot:super::SnapshotPayload=serde_json::from_str(&job.payload).unwrap();assert_eq!(std::fs::read(&snapshot.audio[0]).unwrap(),b"audio");assert_eq!(snapshot.manifest["audio"][0]["bytes"],5);assert_eq!(snapshot.manifest["tracks"][0]["offset_seconds"],0.5);Ok(())}).unwrap();drop(queue);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]fn retry_survives_restart_and_keeps_immutable_payload(){
        let path=std::env::temp_dir().join(format!("gigaam-outbox-{}.sqlite",crate::new_id()));
        {let mut queue=super::Outbox::open(&path).unwrap();queue.enqueue("snapshot","original").unwrap();queue.enqueue("snapshot","replacement").unwrap();assert!(queue.run_one(0,|job|{assert_eq!(job.payload,"original");Err(false)}).unwrap());assert!(!queue.run_one(1,|_|panic!()).unwrap());}
        {let mut queue=super::Outbox::open(&path).unwrap();assert!(queue.run_one(2000,|job|{assert_eq!(job.attempts,1);assert_eq!(job.payload,"original");Ok(())}).unwrap());assert!(!queue.run_one(99999,|_|panic!()).unwrap());}
        let _=std::fs::remove_file(path);
    }
    #[test]fn revoked_access_pauses_until_reconnect(){
        let path=std::env::temp_dir().join(format!("gigaam-outbox-{}.sqlite",crate::new_id()));
        let mut queue=super::Outbox::open(&path).unwrap();queue.enqueue("a","{}").unwrap();queue.run_one(0,|_|Err(true)).unwrap();assert!(!queue.run_one(99999,|_|panic!()).unwrap());queue.resume_after_auth().unwrap();assert!(queue.run_one(99999,|_|Ok(())).unwrap());drop(queue);let _=std::fs::remove_file(path);
    }
    #[test]fn network_work_does_not_block_enqueue(){
        let path=std::env::temp_dir().join(format!("gigaam-outbox-{}.sqlite",crate::new_id()));
        let mut queue=super::Outbox::open(&path).unwrap();queue.enqueue("a","{}").unwrap();
        queue.run_one(0,|_|{let other=super::Outbox::open(&path).unwrap();other.enqueue("b","{}").unwrap();Ok(())}).unwrap();
        assert!(queue.run_one(0,|job|{assert_eq!(job.id,"b");Ok(())}).unwrap());drop(queue);let _=std::fs::remove_file(path);
    }
    #[test]fn crashed_upload_claim_is_retried_only_after_expiry(){
        let path=std::env::temp_dir().join(format!("gigaam-lease-{}.sqlite",crate::new_id()));
        {let mut queue=super::Outbox::open(&path).unwrap();queue.enqueue("a","original").unwrap();let _=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||queue.run_one(100,|_|panic!("simulated worker crash"))));}
        let mut queue=super::Outbox::open(&path).unwrap();assert!(!queue.run_one(3599999,|_|panic!("lease not expired")).unwrap());queue.recover_interrupted().unwrap();assert!(queue.run_one(100,|job|{assert_eq!(job.payload,"original");Ok(())}).unwrap());drop(queue);let _=std::fs::remove_file(path);
    }
    #[test]fn immutable_payload_survives_source_changes(){
        let root=std::env::temp_dir().join(format!("gigaam-payload-{}",crate::new_id()));std::fs::create_dir_all(&root).unwrap();
        let source=root.join("source");std::fs::write(&source,b"original").unwrap();
        super::enqueue_immutable(&root,"folder","snapshot:1","audio.ogg",&source).unwrap();
        std::fs::write(&source,b"changed").unwrap();
        super::enqueue_immutable(&root,"folder","snapshot:1","audio.ogg",&source).unwrap();
        let mut queue=super::Outbox::open(&root.join("drive-outbox.sqlite")).unwrap();
        queue.run_one(0,|job|{let payload:super::UploadPayload=serde_json::from_str(&job.payload).unwrap();assert_eq!(std::fs::read(payload.path).unwrap(),b"original");Ok(())}).unwrap();
        drop(queue);std::fs::remove_dir_all(root).unwrap();
    }
}
