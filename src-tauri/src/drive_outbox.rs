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
        // Hold a short-lived SQLite write claim across transport: a second
        // process cannot submit this job concurrently. No capture DB is held.
        let tx=self.db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let job=tx.query_row("SELECT id,payload,attempts FROM drive_jobs WHERE state='pending' AND next_ms<=?1 ORDER BY next_ms,id LIMIT 1",[now],|r|Ok(Job{id:r.get(0)?,payload:r.get(1)?,attempts:r.get(2)?})).optional()?;
        let Some(job)=job else {tx.commit()?;return Ok(false)};
        match upload(&job){
            Ok(())=>{tx.execute("UPDATE drive_jobs SET state='synced' WHERE id=?1",[&job.id])?;}
            Err(auth)=>{
                let attempts=job.attempts.saturating_add(1);
                let delay=1000i64.saturating_mul(1i64<<attempts.min(10)).min(900000);
                tx.execute("UPDATE drive_jobs SET state=?2,attempts=?3,next_ms=?4 WHERE id=?1",params![job.id,if auth{"auth_required"}else{"pending"},attempts,now.saturating_add(delay)])?;
            }
        }
        tx.commit()?;Ok(true)
    }
    pub fn resume_after_auth(&self)->rusqlite::Result<()> {self.db.execute("UPDATE drive_jobs SET state='pending',next_ms=0 WHERE state='auth_required'",[])?;Ok(())}
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
}
