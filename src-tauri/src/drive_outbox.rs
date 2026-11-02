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

pub fn conflict_heads(manifests:&[serde_json::Value])->Vec<String>{
    let parents:std::collections::HashSet<&str>=manifests.iter().flat_map(|value|{
        value["parent_revision"].as_str().into_iter().chain(value["merge_parents"].as_array().into_iter().flatten().filter_map(|parent|parent.as_str()))
    }).collect();
    let mut heads:Vec<String>=manifests.iter().filter_map(|value|value["revision"].as_str()).filter(|revision|!parents.contains(revision)).map(str::to_owned).collect();
    heads.sort();heads.dedup();heads
}
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
    upload_recording_with(root,snapshot,&folder,|key,name,path|crate::google_drive::upload_snapshot(&folder,key,name,path))
}

fn upload_recording_with(root:&std::path::Path,snapshot:&SnapshotPayload,_folder:&str,mut upload:impl FnMut(&str,&str,&std::path::Path)->Result<String,String>)->Result<(),String>{
    let mut manifest=snapshot.manifest.clone();
    let audio=manifest["audio"].as_array_mut().ok_or("Invalid queued manifest")?;
    if audio.len()!=snapshot.audio.len(){return Err("Invalid queued audio count".into());}
    for(index,path)in snapshot.audio.iter().enumerate(){
        let key=format!("{}:{}:{index}",snapshot.recording_id,snapshot.revision);
        let name=format!("{}-{}-{index}.{}",snapshot.recording_id,snapshot.revision,audio[index]["original_filename"].as_str().and_then(|s|std::path::Path::new(s).extension()).and_then(|s|s.to_str()).unwrap_or("audio"));
        audio[index]["file_id"]=upload(&key,&name,path)?.into();
    }
    let path=root.join("drive-payloads").join(format!("{}-manifest.json",snapshot.revision));
    let staged=path.with_extension("json.partial");
    std::fs::write(&staged,manifest.to_string()).map_err(|_|"Manifest staging failed")?;
    if path.exists(){
        let existing=std::fs::read(&path).map_err(|_|"Queued manifest unavailable")?;
        if existing!=manifest.to_string().as_bytes(){return Err("Snapshot manifest changed during retry".into());}
        let _=std::fs::remove_file(staged);
    }else{std::fs::rename(staged,&path).map_err(|_|"Manifest publication failed")?;}
    upload(&format!("{}:{}:manifest",snapshot.recording_id,snapshot.revision),&format!("{}-{}.json",snapshot.recording_id,snapshot.revision),&path)?;
    let lineage_directory=root.join("drive-lineage");std::fs::create_dir_all(&lineage_directory).map_err(|_|"Could not persist uploaded lineage")?;
    let target=lineage_directory.join(format!("{}.json",snapshot.recording_id));
    let staged=target.with_extension("json.partial");
    std::fs::write(&staged,serde_json::json!({"original_recording_id":manifest["original_recording_id"],"device_id":manifest["device_id"],"revision":manifest["revision"]}).to_string()).map_err(|_|"Could not stage uploaded lineage")?;
    // Keep the last acknowledged parent if publication is interrupted. A
    // durable SQLite row avoids the Windows remove-then-rename crash window.
    let db=rusqlite::Connection::open(root.join("drive-outbox.sqlite")).map_err(|_|"Lineage store unavailable")?;
    db.busy_timeout(std::time::Duration::from_secs(5)).map_err(|_|"Lineage store unavailable")?;
    db.execute_batch("PRAGMA synchronous=FULL;CREATE TABLE IF NOT EXISTS drive_local_lineage(recording_id TEXT PRIMARY KEY,manifest TEXT NOT NULL)").map_err(|_|"Lineage schema unavailable")?;
    db.execute("INSERT INTO drive_local_lineage(recording_id,manifest) VALUES(?1,?2) ON CONFLICT(recording_id) DO UPDATE SET manifest=excluded.manifest",params![snapshot.recording_id,std::fs::read_to_string(&staged).map_err(|_|"Lineage staging unavailable")?]).map_err(|_|"Lineage persistence failed")?;
    let _=std::fs::remove_file(staged);
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
    let parent=snapshot_lineage(root,id)?;
    manifest["original_recording_id"]=parent.as_ref().map(|value|value.original_id.as_str()).unwrap_or(id).into();
    // A restored copy has a new local ID. Its organization snapshot still uses
    // that ID; keep it separate from the stable cross-device recording ID.
    manifest["organization_recording_id"]=id.into();
    manifest["parent_revision"]=parent.as_ref().map(|value|value.revision.clone()).into();
    manifest["revision"]=revision.clone().into();
    manifest["device_id"]=device_id(root)?.into();
    if let Some(nodes)=manifest["organization"].as_array_mut(){
        let device=device_id(root)?;
        let canonical=|id:&str|{
            if id.starts_with("drive-folder-")&&id.len()==77&&id[13..].bytes().all(|c|c.is_ascii_hexdigit()){id.to_owned()}
            else{format!("drive-folder-{:x}",Sha256::digest(format!("{device}:{id}").as_bytes()))}
        };
        let folders:std::collections::HashMap<String,String>=nodes.iter().filter(|node|node["kind"]=="folder").filter_map(|node|node["id"].as_str().map(|id|(id.to_owned(),canonical(id)))).collect();
        manifest_folder_mappings(root,&folders)?;
        for node in nodes{
            if node["kind"]=="folder"{if let Some(id)=node["id"].as_str().and_then(|id|folders.get(id)).cloned(){node["id"]=id.into();}}
            if let Some(parent)=node["parent_id"].as_str().and_then(|id|folders.get(id)).cloned(){node["parent_id"]=parent.into();}
        }
    }
    let payload=SnapshotPayload{revision:revision.clone(),recording_id:id.into(),manifest,audio};
    Outbox::open(&root.join("drive-outbox.sqlite")).and_then(|queue|queue.enqueue(&revision,&serde_json::to_string(&payload).unwrap())).map_err(|_|"Could not persist snapshot job".into())
}

fn manifest_folder_mappings(root:&std::path::Path,folders:&std::collections::HashMap<String,String>)->Result<(),String>{
    // Optional app DB: unit harnesses only contain the isolated queue DB.
    let path=root.join("drive-folder-mapping.sqlite");
    let mut db=Connection::open(path).map_err(|_|"Folder mapping store unavailable")?;
    db.execute_batch("PRAGMA synchronous=FULL;CREATE TABLE IF NOT EXISTS mappings(cloud_id TEXT PRIMARY KEY,local_id TEXT NOT NULL)").map_err(|_|"Folder mapping schema unavailable")?;
    let tx=db.transaction().map_err(|_|"Folder mapping transaction unavailable")?;
    for(local,cloud)in folders{tx.execute("INSERT OR IGNORE INTO mappings(cloud_id,local_id) VALUES(?1,?2)",params![cloud,local]).map_err(|_|"Folder mapping write failed")?;}
    tx.commit().map_err(|_|"Folder mapping commit failed".into())
}

pub fn local_folder_mappings(root:&std::path::Path)->Result<std::collections::HashMap<String,String>,String>{
    let path=root.join("drive-folder-mapping.sqlite");if !path.exists(){return Ok(Default::default());}
    let db=Connection::open(path).map_err(|_|"Folder mapping store unavailable")?;
    let mut stmt=db.prepare("SELECT cloud_id,local_id FROM mappings").map_err(|_|"Folder mappings unavailable")?;
    stmt.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).map_err(|_|"Folder mappings unavailable")?.collect::<rusqlite::Result<_>>().map_err(|_|"Folder mappings invalid".into())
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

#[derive(serde::Serialize,serde::Deserialize,Debug,PartialEq)]
pub struct Lineage { pub original_id:String,pub device_id:String,pub revision:String }

pub fn snapshot_lineage(root:&std::path::Path,id:&str)->Result<Option<Lineage>,String>{
    let db_path=root.join("drive-outbox.sqlite");
    if db_path.exists(){
        use rusqlite::OptionalExtension;
        let db=rusqlite::Connection::open(&db_path).map_err(|_|"Lineage store unavailable")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS drive_local_lineage(recording_id TEXT PRIMARY KEY,manifest TEXT NOT NULL)").map_err(|_|"Lineage schema unavailable")?;
        if let Some(value)=db.query_row("SELECT manifest FROM drive_local_lineage WHERE recording_id=?1",[id],|row|row.get::<_,String>(0)).optional().map_err(|_|"Lineage lookup failed")?{
            let value:serde_json::Value=serde_json::from_str(&value).map_err(|_|"Invalid persisted lineage")?;
            return Ok(Some(Lineage{original_id:value["original_recording_id"].as_str().ok_or("Original identity missing")?.into(),device_id:value["device_id"].as_str().ok_or("Device identity missing")?.into(),revision:value["revision"].as_str().ok_or("Revision missing")?.into()}));
        }
    }
    let latest=root.join("drive-lineage").join(format!("{id}.json"));
    let path=if latest.exists(){latest}else{root.join("live").join(id).join("manifest.json")};
    if !path.exists(){return Ok(None);}
    let bytes=std::fs::read(path).map_err(|_|"Original snapshot lineage unavailable")?;
    let value:serde_json::Value=serde_json::from_slice(&bytes).map_err(|_|"Invalid original snapshot lineage")?;
    Ok(Some(Lineage{original_id:value["original_recording_id"].as_str().or_else(||value["recording"]["id"].as_str()).ok_or("Original identity missing")?.into(),device_id:value["device_id"].as_str().unwrap_or("legacy").into(),revision:value["revision"].as_str().unwrap_or("legacy").into()}))
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
    #[test]fn failed_audio_never_publishes_manifest_and_retry_keeps_order(){
        let root=std::env::temp_dir().join(format!("gigaam-publish-{}",crate::new_id()));std::fs::create_dir_all(root.join("drive-payloads")).unwrap();let a=root.join("a");let b=root.join("b");std::fs::write(&a,b"a").unwrap();std::fs::write(&b,b"b").unwrap();
        let snapshot=super::SnapshotPayload{revision:"rev".into(),recording_id:"record".into(),manifest:serde_json::json!({"audio":[{"original_filename":"a.ogg"},{"original_filename":"b.ogg"}]}),audio:vec![a,b]};
        let mut calls=Vec::new();assert!(super::upload_recording_with(&root,&snapshot,"folder",|key,_,_|{calls.push(key.to_owned());if key.ends_with(":1"){Err("network".into())}else{Ok("remote-a".into())}}).is_err());assert_eq!(calls,vec!["record:rev:0","record:rev:1"]);
        calls.clear();super::upload_recording_with(&root,&snapshot,"folder",|key,_,path|{calls.push(key.to_owned());if key.ends_with("manifest"){let value:serde_json::Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();assert_eq!(value["audio"][0]["file_id"],"remote-a");assert_eq!(value["audio"][1]["file_id"],"remote-b");}Ok(if key.ends_with(":0"){"remote-a"}else{"remote-b"}.into())}).unwrap();assert_eq!(calls,vec!["record:rev:0","record:rev:1","record:rev:manifest"]);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]fn concurrent_revisions_keep_both_heads_without_timestamp_winner(){
        let versions=vec![serde_json::json!({"revision":"base"}),serde_json::json!({"revision":"left","parent_revision":"base"}),serde_json::json!({"revision":"right","parent_revision":"base"})];
        assert_eq!(super::conflict_heads(&versions),vec!["left","right"]);
        let mut merged=versions;merged.push(serde_json::json!({"revision":"next","parent_revision":"left"}));assert_eq!(super::conflict_heads(&merged),vec!["next","right"]);
    }
    #[test]fn explicit_merge_retires_only_selected_parent_heads(){
        let values=vec![serde_json::json!({"revision":"left"}),serde_json::json!({"revision":"right"}),serde_json::json!({"revision":"unrelated"}),serde_json::json!({"revision":"merged","merge_parents":["left","right"]})];
        assert_eq!(super::conflict_heads(&values),vec!["merged","unrelated"]);
    }
    #[test]fn queued_resolution_keeps_parent_set_across_failure_and_restart(){
        let root=std::env::temp_dir().join(format!("gigaam-merge-retry-{}",crate::new_id()));std::fs::create_dir_all(&root).unwrap();let audio=root.join("audio.ogg");std::fs::write(&audio,b"original audio").unwrap();
        super::enqueue_recording(&root,"record",serde_json::json!({"version":1,"merge_parents":["left","right"]}),&[audio]).unwrap();
        let mut revision=String::new();{
            let mut queue=super::Outbox::open(&root.join("drive-outbox.sqlite")).unwrap();queue.run_one(0,|job|{let snapshot:super::SnapshotPayload=serde_json::from_str(&job.payload).unwrap();revision=snapshot.revision;assert_eq!(snapshot.manifest["merge_parents"],serde_json::json!(["left","right"]));Err(false)}).unwrap();
        }
        let mut queue=super::Outbox::open(&root.join("drive-outbox.sqlite")).unwrap();queue.run_one(2000,|job|{let snapshot:super::SnapshotPayload=serde_json::from_str(&job.payload).unwrap();assert_eq!(snapshot.revision,revision);assert_eq!(snapshot.manifest["merge_parents"],serde_json::json!(["left","right"]));assert_eq!(std::fs::read(&snapshot.audio[0]).unwrap(),b"original audio");Ok(())}).unwrap();assert_eq!(queue.status().unwrap().synced,1);drop(queue);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]fn acknowledged_local_lineage_takes_precedence_over_imported_parent(){
        let root=std::env::temp_dir().join(format!("gigaam-parent-{}",crate::new_id()));std::fs::create_dir_all(root.join("drive-lineage")).unwrap();
        std::fs::write(root.join("drive-lineage/local.json"),r#"{"original_recording_id":"original","revision":"uploaded","device_id":"device"}"#).unwrap();
        let lineage=super::snapshot_lineage(&root,"local").unwrap().unwrap();assert_eq!(lineage.revision,"uploaded");assert_eq!(lineage.original_id,"original");std::fs::remove_dir_all(root).unwrap();
    }
    #[test]fn restored_copy_retains_original_identity_on_next_upload(){
        let root=std::env::temp_dir().join(format!("gigaam-lineage-{}",crate::new_id()));let live=root.join("live").join("local-copy");std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("manifest.json"),r#"{"original_recording_id":"original","device_id":"other-device","revision":"parent"}"#).unwrap();let path=live.join("microphone.ogg");std::fs::write(&path,b"audio").unwrap();
        super::enqueue_recording(&root,"local-copy",serde_json::json!({"version":1}),&[path]).unwrap();let mut queue=super::Outbox::open(&root.join("drive-outbox.sqlite")).unwrap();
        queue.run_one(0,|job|{let snapshot:super::SnapshotPayload=serde_json::from_str(&job.payload).unwrap();assert_eq!(snapshot.manifest["original_recording_id"],"original");assert_eq!(snapshot.manifest["parent_revision"],"parent");assert_ne!(snapshot.manifest["device_id"],"other-device");Ok(())}).unwrap();drop(queue);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]fn distinguishes_auth_failures_from_transient_transport(){
        for error in ["Google access expired or was revoked. Reconnect Drive.","Connect Google Drive first","invalid_client","invalid_grant"]{assert!(super::requires_auth(error));}
        for error in ["Drive upload interrupted","Remote verification unavailable","HTTP 429","HTTP 503"]{assert!(!super::requires_auth(error));}
    }
    #[test]fn recording_job_contains_immutable_audio_and_manifest(){
        let root=std::env::temp_dir().join(format!("gigaam-record-job-{}",crate::new_id()));std::fs::create_dir_all(&root).unwrap();let path=root.join("mic.ogg");std::fs::write(&path,b"audio").unwrap();
        super::enqueue_recording(&root,"session",serde_json::json!({"version":1,"segments":[],"tracks":[{"offset_seconds":0.5}]}),&[path.clone()]).unwrap();std::fs::write(path,b"edited").unwrap();
        let mut queue=super::Outbox::open(&root.join("drive-outbox.sqlite")).unwrap();queue.run_one(0,|job|{let snapshot:super::SnapshotPayload=serde_json::from_str(&job.payload).unwrap();assert_eq!(std::fs::read(&snapshot.audio[0]).unwrap(),b"audio");assert_eq!(snapshot.manifest["audio"][0]["bytes"],5);assert_eq!(snapshot.manifest["tracks"][0]["offset_seconds"],0.5);Ok(())}).unwrap();drop(queue);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]fn queued_tree_uses_canonical_ids_and_matching_parent_references(){
        let root=std::env::temp_dir().join(format!("gigaam-tree-job-{}",crate::new_id()));std::fs::create_dir_all(&root).unwrap();let path=root.join("mic.ogg");std::fs::write(&path,b"audio").unwrap();
        super::enqueue_recording(&root,"record",serde_json::json!({"organization":[{"id":"folder","kind":"folder","parent_id":null},{"id":"record","kind":"recording","parent_id":"folder"}]}),&[path]).unwrap();
        let mut queue=super::Outbox::open(&root.join("drive-outbox.sqlite")).unwrap();queue.run_one(0,|job|{let snapshot:super::SnapshotPayload=serde_json::from_str(&job.payload).unwrap();let nodes=&snapshot.manifest["organization"];assert!(nodes[0]["id"].as_str().unwrap().starts_with("drive-folder-"));assert_eq!(nodes[1]["parent_id"],nodes[0]["id"]);Ok(())}).unwrap();drop(queue);std::fs::remove_dir_all(root).unwrap();
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
