//! Durable queue scheduling. Network transport is injected for restart tests.
use rusqlite::{Connection, params};

pub struct Outbox { db: Connection }
#[derive(Debug, PartialEq)]
pub struct Job { pub id: String, pub payload: String, pub attempts: u32 }
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
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct UploadPayload {
    pub folder: String,
    pub key: String,
    pub name: String,
    pub path: std::path::PathBuf,
}

pub fn start_worker(root: std::path::PathBuf) {
    std::thread::spawn(move || {
        let path=root.join("drive-outbox.sqlite");
        loop {
            if crate::google_drive::connected() {
                if let Ok(mut queue)=Outbox::open(&path) {
                    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|v|v.as_millis() as i64).unwrap_or(0);
                    let _=queue.run_one(now,|job| {
                        let payload:UploadPayload=serde_json::from_str(&job.payload).map_err(|_|true)?;
                        crate::google_drive::upload_snapshot(&payload.folder,&payload.key,&payload.name,&payload.path).map(|_|()).map_err(|error|error.contains("revoked")||error.contains("expired")||error.contains("Connect Google"))
                    });
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
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
