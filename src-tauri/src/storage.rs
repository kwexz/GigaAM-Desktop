use std::{path::Path, str::FromStr};

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::{
    SegmentState, SourceTrack, TranscriptSegment, Transcription, TranscriptionStatus,
};

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn restored_snapshot(&self, remote_id:&str)->rusqlite::Result<Option<String>> {
        self.connection.execute_batch("CREATE TABLE IF NOT EXISTS drive_restores(remote_id TEXT PRIMARY KEY,local_id TEXT NOT NULL REFERENCES transcriptions(id) ON DELETE CASCADE)")?;
        self.connection.query_row("SELECT local_id FROM drive_restores WHERE remote_id=?1",[remote_id],|row|row.get(0)).optional()
    }
    pub fn restore_snapshot(&mut self,remote_id:&str,record:&Transcription,segments:&[TranscriptSegment])->rusqlite::Result<String>{
        self.restore_snapshot_organized(remote_id,record,segments,&serde_json::Value::Null)
    }
    pub fn restore_snapshot_organized(&mut self,remote_id:&str,record:&Transcription,segments:&[TranscriptSegment],manifest:&serde_json::Value)->rusqlite::Result<String>{
        if let Some(id)=self.restored_snapshot(remote_id)?{return Ok(id);}
        let tx=self.connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(id)=tx.query_row("SELECT local_id FROM drive_restores WHERE remote_id=?1",[remote_id],|row|row.get::<_,String>(0)).optional()?{return Ok(id);}
        insert_transcription(&tx,record)?;
        for segment in segments {
            if segment.transcription_id!=record.id || !segment.start_seconds.is_finite() || !segment.end_seconds.is_finite() || segment.start_seconds<0.0 || segment.end_seconds<segment.start_seconds{return Err(rusqlite::Error::InvalidQuery);}
            tx.execute("INSERT INTO transcript_segments(id,transcription_id,segment_index,start_seconds,end_seconds,text,source_track,speaker_id,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![segment.id,segment.transcription_id,segment.index,segment.start_seconds,segment.end_seconds,segment.text,source_track_value(segment.source_track),segment.speaker_id,segment_state_value(segment.state)])?;
        }
        tx.execute("INSERT INTO drive_restores(remote_id,local_id) VALUES(?1,?2)",params![remote_id,record.id])?;
        // Three-way folder-name reconciliation: apply a remote rename only
        // while the local value still matches the last imported baseline.
        tx.execute_batch("CREATE TABLE IF NOT EXISTS drive_folder_baselines(local_id TEXT PRIMARY KEY REFERENCES recording_folders(id) ON DELETE CASCADE,name TEXT NOT NULL)")?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS drive_folder_locations(local_id TEXT PRIMARY KEY REFERENCES recording_folders(id) ON DELETE CASCADE,parent_id TEXT,position INTEGER NOT NULL)")?;
        if let Some(nodes)=manifest["organization"].as_array(){
            // Include empty folders too; validate every source parent chain
            // before committing any recording or tree data.
            let device=manifest["device_id"].as_str().unwrap_or("legacy");
            let folder_id=|id:&str|{
                if let Some(local)=manifest["local_folder_mappings"][id].as_str(){return local.to_owned();}
                if id.starts_with("drive-folder-")&&id.len()==77&&id[13..].bytes().all(|c|c.is_ascii_hexdigit()){return id.to_owned();}
                use sha2::{Digest,Sha256};format!("drive-folder-{:x}",Sha256::digest(format!("{device}:{id}").as_bytes()))
            };
            let mut imported=std::collections::HashSet::new();
            for folder in nodes.iter().filter(|node|node["kind"]=="folder"){
                let source=folder["id"].as_str().filter(|id|!id.is_empty()).ok_or(rusqlite::Error::InvalidQuery)?;
                let mut cursor=Some(source);let mut seen=std::collections::HashSet::new();
                while let Some(id)=cursor{
                    if !seen.insert(id)||seen.len()>64{return Err(rusqlite::Error::InvalidQuery);}
                    let node=nodes.iter().find(|node|node["kind"]=="folder"&&node["id"].as_str()==Some(id)).ok_or(rusqlite::Error::InvalidQuery)?;
                    cursor=node["parent_id"].as_str();
                }
                let name=folder["name"].as_str().filter(|name|!name.trim().is_empty()&&name.len()<=1024).ok_or(rusqlite::Error::InvalidQuery)?;
                if tx.execute("INSERT OR IGNORE INTO recording_folders(id,parent_id,name,position) VALUES(?1,NULL,?2,?3)",params![folder_id(source),name,folder["position"].as_i64().unwrap_or(0).max(0)])?==1{imported.insert(folder_id(source));}
                let local_id=folder_id(source);
                let baseline:Option<String>=tx.query_row("SELECT name FROM drive_folder_baselines WHERE local_id=?1",[&local_id],|row|row.get(0)).optional()?;
                if let Some(previous)=baseline {
                    tx.execute("UPDATE recording_folders SET name=?2 WHERE id=?1 AND name=?3",params![local_id,name,previous])?;
                }
                tx.execute("INSERT INTO drive_folder_baselines(local_id,name) VALUES(?1,?2) ON CONFLICT(local_id) DO UPDATE SET name=excluded.name",params![local_id,name])?;
            }
            // Set parents only for newly imported folders; existing local
            // hierarchy is retained until explicit conflict resolution.
            for folder in nodes.iter().filter(|node|node["kind"]=="folder"){
                let id=folder_id(folder["id"].as_str().ok_or(rusqlite::Error::InvalidQuery)?);
                let parent=folder["parent_id"].as_str().map(&folder_id);
                let position=folder["position"].as_i64().unwrap_or(0).max(0);
                let baseline:Option<(Option<String>,i64)>=tx.query_row("SELECT parent_id,position FROM drive_folder_locations WHERE local_id=?1",[&id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
                let local:(Option<String>,i64)=tx.query_row("SELECT parent_id,position FROM recording_folders WHERE id=?1",[&id],|row|Ok((row.get(0)?,row.get(1)?)))?;
                if imported.contains(&id)||baseline.as_ref()==Some(&local){
                    // A valid source tree can still cycle with a conflicting
                    // local move. Keep local structure rather than introduce it.
                    let mut cursor=parent.clone();let mut seen=std::collections::HashSet::new();let mut safe=true;
                    while let Some(current)=cursor{if current==id||!seen.insert(current.clone()){safe=false;break;}cursor=tx.query_row("SELECT parent_id FROM recording_folders WHERE id=?1",[current],|row|row.get(0)).optional()?.flatten();}
                    if safe{tx.execute("UPDATE recording_folders SET parent_id=?2,position=?3 WHERE id=?1",params![id,parent,position])?;}
                }
                tx.execute("INSERT INTO drive_folder_locations(local_id,parent_id,position) VALUES(?1,?2,?3) ON CONFLICT(local_id) DO UPDATE SET parent_id=excluded.parent_id,position=excluded.position",params![id,parent,position])?;
            }
            let original=manifest["original_recording_id"].as_str().or_else(||manifest["recording"]["id"].as_str()).unwrap_or("");
            if let Some(recording)=nodes.iter().find(|node|node["kind"]=="recording"&&node["id"].as_str()==Some(original)){
                let mut parent=recording["parent_id"].as_str();let mut chain=Vec::new();let mut seen=std::collections::HashSet::new();
                while let Some(id)=parent{
                    if !seen.insert(id)||chain.len()>=64{return Err(rusqlite::Error::InvalidQuery);}
                    let folder=nodes.iter().find(|node|node["kind"]=="folder"&&node["id"].as_str()==Some(id)).ok_or(rusqlite::Error::InvalidQuery)?;
                    chain.push(folder);parent=folder["parent_id"].as_str();
                }
                let mut local_parent:Option<String>=None;
                for folder in chain.iter().rev(){
                    let name=folder["name"].as_str().filter(|name|!name.trim().is_empty()&&name.len()<=1024).ok_or(rusqlite::Error::InvalidQuery)?;
                    let source_folder=folder["id"].as_str().ok_or(rusqlite::Error::InvalidQuery)?;
                    let id=folder_id(source_folder);
                    tx.execute("INSERT OR IGNORE INTO recording_folders(id,parent_id,name,position) VALUES(?1,?2,?3,?4)",params![id,local_parent,name,folder["position"].as_i64().unwrap_or(0).max(0)])?;local_parent=Some(id);
                }
                tx.execute("INSERT INTO recording_positions(recording_id,folder_id,position) VALUES(?1,?2,?3)",params![record.id,local_parent,recording["position"].as_i64().unwrap_or(0).max(0)])?;
            }
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS drive_lineage(local_id TEXT PRIMARY KEY REFERENCES transcriptions(id) ON DELETE CASCADE,original_id TEXT,device_id TEXT,revision TEXT)")?;
        tx.execute("INSERT INTO drive_lineage(local_id,original_id,device_id,revision) VALUES(?1,?2,?3,?4)",params![record.id,manifest["original_recording_id"].as_str(),manifest["device_id"].as_str(),manifest["revision"].as_str()])?;
        tx.commit()?;Ok(record.id.clone())
    }
    pub fn clear_transcript(&mut self, id: &str) -> rusqlite::Result<()> {
        self.connection.execute("DELETE FROM transcript_segments WHERE transcription_id=?1", [id])?;
        Ok(())
    }
    pub fn complete_live_archive(
        &mut self,
        id: &str,
        duration: f64,
        audio_path: Option<&str>,
        now_ms: i64,
    ) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE transcriptions SET duration_seconds=?2,audio_path=?3,status='completed',completed_ms=?4,error=NULL
             WHERE id=?1 AND status='transcribing' AND cancel_requested=0",
            params![id, duration, audio_path, now_ms],
        )?;
        if changed != 1 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        transaction.commit()
    }

    pub fn begin_live_finalization(&mut self, id: &str, now_ms: i64) -> rusqlite::Result<()> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS live_finalizations (
                transcription_id TEXT PRIMARY KEY REFERENCES transcriptions(id) ON DELETE CASCADE,
                state TEXT NOT NULL CHECK(state IN ('finalizing','completed','failed')),
                updated_ms INTEGER NOT NULL
            );",
        )?;
        self.connection.execute(
            "INSERT INTO live_finalizations(transcription_id,state,updated_ms) VALUES (?1,'finalizing',?2)
             ON CONFLICT(transcription_id) DO UPDATE SET state='finalizing',updated_ms=excluded.updated_ms",
            params![id, now_ms],
        )?;
        Ok(())
    }

    pub fn finish_live_finalization(&mut self, id: &str, success: bool, now_ms: i64) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE live_finalizations SET state=?2,updated_ms=?3 WHERE transcription_id=?1",
            params![id, if success { "completed" } else { "failed" }, now_ms],
        )?;
        Ok(())
    }

    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn memory() -> rusqlite::Result<Self> {
        let connection = Connection::open_in_memory()?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn list_folders(&self) -> rusqlite::Result<Vec<(String, Option<String>, String, i64)>> {
        let mut statement = self.connection.prepare("SELECT id,parent_id,name,position FROM recording_folders ORDER BY position,id")?;
        statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?.collect()
    }

    pub fn load_organization(&self) -> rusqlite::Result<serde_json::Value> {
        let mut nodes = Vec::new();
        for (id, parent, name, position) in self.list_folders()? {
            nodes.push(serde_json::json!({"id":id,"parent_id":parent,"name":name,"position":position,"kind":"folder"}));
        }
        let mut statement = self.connection.prepare("SELECT t.id,p.folder_id,t.source_filename,COALESCE(p.position,ROW_NUMBER() OVER (ORDER BY t.created_ms DESC,t.id)-1) FROM transcriptions t LEFT JOIN recording_positions p ON p.recording_id=t.id ORDER BY t.created_ms DESC,t.id")?;
        for row in statement.query_map([], |row| Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?)))? {
            let (id,parent,name,position)=row?;
            nodes.push(serde_json::json!({"id":id,"parent_id":parent,"name":name,"position":position,"kind":"recording"}));
        }
        Ok(serde_json::Value::Array(nodes))
    }

    pub fn save_organization_if_unchanged(&mut self, previous: serde_json::Value, nodes: serde_json::Value) -> Result<(), String> {
        let normalize = |value: serde_json::Value| {
            let mut entries = value.as_array().cloned().unwrap_or_default();
            entries.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
            entries
        };
        if !previous.is_array() || normalize(self.load_organization().map_err(|error| error.to_string())?) != normalize(previous) {
            return Err("Organization changed; refresh and retry".into());
        }
        self.save_organization(nodes)
    }

    pub fn save_organization(&mut self, nodes: serde_json::Value) -> Result<(), String> {
        let nodes = nodes.as_array().ok_or("Expected organization array")?;
        if nodes.len() > 100_000 { return Err("Organization limit exceeded".into()); }
        let mut ids = std::collections::HashSet::new();
        let mut folders = std::collections::HashMap::new();
        for node in nodes {
            let id=node["id"].as_str().filter(|id| !id.is_empty()).ok_or("Missing ID")?;
            if id.len() > 256 { return Err("Item ID too long".into()); }
            if !ids.insert(id) { return Err("Duplicate ID".into()); }
            if node["position"].as_u64().filter(|n| *n <= i64::MAX as u64).is_none() { return Err("Invalid position".into()); }
            if node["kind"] == "folder" {
                let existing_recording: bool = self.connection.query_row("SELECT EXISTS(SELECT 1 FROM transcriptions WHERE id=?1)", [id], |row| row.get(0)).map_err(|error| error.to_string())?;
                if existing_recording { return Err("Folder ID collides with recording".into()); }
                if node["name"].as_str().filter(|s| !s.trim().is_empty() && s.len() <= 1024).is_none() { return Err("Invalid folder name".into()); }
                folders.insert(id, node["parent_id"].as_str());
            } else if node["kind"] != "recording" { return Err("Invalid item kind".into()); }
            if !node["parent_id"].is_null() && !node["parent_id"].is_string() { return Err("Invalid parent".into()); }
        }
        for node in nodes {
            let id=node["id"].as_str().unwrap();
            let mut parent=node["parent_id"].as_str();
            let mut seen=std::collections::HashSet::from([id]);
            while let Some(p)=parent {
                if !seen.insert(p) { return Err("Folder cycle".into()); }
                parent=*folders.get(p).ok_or("Unknown parent")?;
            }
        }
        let tx=self.connection.transaction().map_err(|e|e.to_string())?;
        let count: usize = tx.query_row("SELECT COUNT(*) FROM transcriptions", [], |row| row.get(0)).map_err(|e| e.to_string())?;
        if nodes.iter().filter(|node| node["kind"] == "recording").count() != count {
            return Err("Recording list changed; refresh before changing organization".into());
        }
        tx.execute("DELETE FROM recording_positions", []).map_err(|e|e.to_string())?;
        tx.execute("UPDATE recording_folders SET parent_id=NULL", []).map_err(|e|e.to_string())?;
        tx.execute("DELETE FROM recording_folders", []).map_err(|e|e.to_string())?;
        for node in nodes.iter().filter(|node| node["kind"]=="folder") {
            tx.execute("INSERT INTO recording_folders(id,name,position) VALUES(?1,?2,?3)", rusqlite::params![node["id"].as_str(),node["name"].as_str(),node["position"].as_i64()]).map_err(|e|e.to_string())?;
        }
        for node in nodes {
            if node["kind"]=="folder" {
                tx.execute("UPDATE recording_folders SET parent_id=?2 WHERE id=?1",rusqlite::params![node["id"].as_str(),node["parent_id"].as_str()]).map_err(|e|e.to_string())?;
            } else {
                let exists: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM transcriptions WHERE id=?1)",[node["id"].as_str()],|row|row.get(0)).map_err(|e|e.to_string())?;
                if !exists { return Err("Unknown recording".into()); }
                tx.execute("INSERT INTO recording_positions(recording_id,folder_id,position) VALUES(?1,?2,?3)",rusqlite::params![node["id"].as_str(),node["parent_id"].as_str(),node["position"].as_i64()]).map_err(|e|e.to_string())?;
            }
        }
        tx.commit().map_err(|e|e.to_string())
    }


    fn migrate(&mut self) -> rusqlite::Result<()> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS recording_folders (
                id TEXT PRIMARY KEY,
                parent_id TEXT REFERENCES recording_folders(id),
                name TEXT NOT NULL CHECK(length(trim(name)) > 0),
                position INTEGER NOT NULL CHECK(position >= 0)
             );
             CREATE TABLE IF NOT EXISTS recording_positions (
                recording_id TEXT PRIMARY KEY REFERENCES transcriptions(id) ON DELETE CASCADE,
                folder_id TEXT REFERENCES recording_folders(id),
                position INTEGER NOT NULL CHECK(position >= 0)
             );"
        )?;

        self.connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS transcriptions (
               id TEXT PRIMARY KEY,
               kind TEXT NOT NULL CHECK(kind IN ('file','live')),
               source_filename TEXT NOT NULL,
               source_path TEXT NOT NULL,
               source_size_bytes INTEGER NOT NULL CHECK(source_size_bytes >= 0),
               source_modified_ms INTEGER NOT NULL,
               created_ms INTEGER NOT NULL,
               started_ms INTEGER,
               completed_ms INTEGER,
               status TEXT NOT NULL CHECK(status IN ('queued','preparing','transcribing','completed','cancelled','failed','interrupted')),
               duration_seconds REAL CHECK(duration_seconds IS NULL OR duration_seconds >= 0),
               model_version TEXT NOT NULL,
               error TEXT,
               audio_path TEXT,
               start_requested INTEGER NOT NULL DEFAULT 0 CHECK(start_requested IN (0,1)),
               completed_chunks INTEGER NOT NULL DEFAULT 0 CHECK(completed_chunks >= 0),
               total_chunks INTEGER CHECK(total_chunks IS NULL OR total_chunks >= 0),
               cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)),
               queue_visible INTEGER NOT NULL DEFAULT 1 CHECK(queue_visible IN (0,1))
             );
             CREATE TABLE IF NOT EXISTS transcript_segments (
               id TEXT PRIMARY KEY,
               transcription_id TEXT NOT NULL REFERENCES transcriptions(id) ON DELETE CASCADE,
               segment_index INTEGER NOT NULL CHECK(segment_index >= 0),
               start_seconds REAL NOT NULL CHECK(start_seconds >= 0),
               end_seconds REAL NOT NULL CHECK(end_seconds > start_seconds),
               text TEXT NOT NULL,
               source_track TEXT NOT NULL CHECK(source_track IN ('file','microphone','system')),
               speaker_id TEXT,
               state TEXT NOT NULL CHECK(state IN ('provisional','final')),
               UNIQUE(transcription_id, segment_index)
             );
             CREATE INDEX IF NOT EXISTS idx_transcriptions_queue ON transcriptions(queue_visible, created_ms);
             CREATE INDEX IF NOT EXISTS idx_transcriptions_filename ON transcriptions(source_filename);
             CREATE INDEX IF NOT EXISTS idx_segments_transcription ON transcript_segments(transcription_id, segment_index);",
        )?;
        self.connection.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_single_active_transcription
             ON transcriptions((1)) WHERE status IN ('preparing','transcribing');",
        )?;
        let columns: Vec<String> = self
            .connection
            .prepare("PRAGMA table_info(transcriptions)")?
            .query_map([], |row| row.get(1))?
            .collect::<rusqlite::Result<_>>()?;
        if !columns.iter().any(|column| column == "completed_chunks") {
            self.connection.execute_batch(
                "ALTER TABLE transcriptions ADD COLUMN completed_chunks INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE transcriptions ADD COLUMN total_chunks INTEGER;",
            )?;
        }
        if !columns.iter().any(|column| column == "cancel_requested") {
            self.connection.execute_batch(
                "ALTER TABLE transcriptions ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        if !columns.iter().any(|column| column == "start_requested") {
            self.connection.execute_batch(
                "ALTER TABLE transcriptions ADD COLUMN start_requested INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        if !columns.iter().any(|column| column == "audio_path") {
            self.connection
                .execute_batch("ALTER TABLE transcriptions ADD COLUMN audio_path TEXT;")?;
        }
        // Older live rows predate `audio_path`. Recover the first archived
        // track so playback remains available after upgrading.
        let mut statement = self.connection.prepare(
            "SELECT id FROM transcriptions WHERE kind = 'live' AND (audio_path IS NULL OR audio_path = '')",
        )?;
        let ids: Vec<String> = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        drop(statement);
        if let Some(data_dir) = self
            .connection
            .path()
            .and_then(|path| Path::new(path).parent())
            .map(Path::to_path_buf)
        {
            for id in ids {
                let directory = data_dir.join("live").join(&id);
                let recovered = std::fs::read_dir(directory).ok().and_then(|entries| {
                    entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| {
                            matches!(
                                path.extension().and_then(|extension| extension.to_str()),
                                Some("flac" | "wav" | "mp3")
                            )
                        })
                        .min()
                });
                if let Some(path) = recovered {
                    self.connection.execute(
                        "UPDATE transcriptions SET audio_path = ?2 WHERE id = ?1",
                        params![id, path.to_string_lossy()],
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn enqueue(&mut self, transcription: &Transcription) -> rusqlite::Result<()> {
        insert_transcription(&self.connection, transcription)
    }

    pub fn enqueue_batch(&mut self, transcriptions: &[Transcription]) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        for transcription in transcriptions {
            insert_transcription(&transaction, transcription)?;
        }
        transaction.commit()
    }

    pub fn insert_live_session(&mut self, transcription: &Transcription) -> rusqlite::Result<()> {
        insert_transcription(&self.connection, transcription)
    }

    pub fn claim_next(&mut self, now_ms: i64) -> rusqlite::Result<Option<Transcription>> {
        let transaction = self.connection.transaction()?;
        let active: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM transcriptions WHERE status IN ('preparing','transcribing'))",
            [],
            |row| row.get(0),
        )?;
        if active {
            return Ok(None);
        }
        let id: Option<String> = transaction
            .query_row(
                "SELECT id FROM transcriptions WHERE queue_visible = 1 AND kind = 'file' AND status = 'queued' AND start_requested = 1
                 ORDER BY created_ms, rowid LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        transaction.execute(
            "UPDATE transcriptions SET status = 'preparing', started_ms = ?2,
                    completed_ms = NULL, error = NULL WHERE id = ?1",
            params![id, now_ms],
        )?;
        let claimed = transaction.query_row(
            "SELECT * FROM transcriptions WHERE id = ?1",
            [&id],
            map_transcription,
        )?;
        transaction.commit()?;
        Ok(Some(claimed))
    }

    pub fn is_cancelled(&self, id: &str) -> rusqlite::Result<bool> {
        self.connection.query_row(
            "SELECT status = 'cancelled' OR cancel_requested = 1 FROM transcriptions WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
    }

    pub fn request_cancel(&mut self, id: &str, now_ms: i64) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET
               cancel_requested = CASE WHEN status IN ('preparing','transcribing') THEN 1 ELSE cancel_requested END,
               completed_ms = CASE WHEN status = 'queued' THEN ?2 ELSE completed_ms END,
               status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END
             WHERE id = ?1 AND status IN ('queued','preparing','transcribing')",
            params![id, now_ms],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn request_start(&mut self, id: &str) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET start_requested = 1 WHERE id = ?1 AND status = 'queued'",
            [id],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn acknowledge_cancel(&mut self, id: &str, now_ms: i64) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET status = 'cancelled', completed_ms = ?2,
                    cancel_requested = 0 WHERE id = ?1 AND cancel_requested = 1
                    AND status IN ('preparing','transcribing')",
            params![id, now_ms],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn complete_if_not_cancelled(
        &mut self,
        id: &str,
        duration_seconds: f64,
        now_ms: i64,
    ) -> rusqlite::Result<bool> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET status = 'completed', duration_seconds = ?2,
                    completed_ms = ?3, error = NULL
             WHERE id = ?1 AND status = 'transcribing' AND cancel_requested = 0",
            params![id, duration_seconds, now_ms],
        )?;
        Ok(changed == 1)
    }

    pub fn fail_if_not_cancelled(
        &mut self,
        id: &str,
        error: &str,
        now_ms: i64,
    ) -> rusqlite::Result<bool> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET status = 'failed', completed_ms = ?3, error = ?2
             WHERE id = ?1 AND status = 'transcribing' AND cancel_requested = 0",
            params![id, error, now_ms],
        )?;
        Ok(changed == 1)
    }

    pub fn reset_for_retry(&mut self, id: &str) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        let (status, kind, audio_path): (String, String, Option<String>) = transaction.query_row(
            "SELECT status, kind, audio_path FROM transcriptions WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if !matches!(
            status.as_str(),
            "cancelled" | "failed" | "interrupted" | "completed"
        ) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        // A live session cannot be re-transcribed live afterwards; instead it
        // becomes a regular file job over its archived FLAC so the existing
        // queue/worker/export flow picks it up unchanged.
        if kind == "live" {
            let audio = audio_path
                .filter(|path| !path.is_empty())
                .ok_or(rusqlite::Error::InvalidQuery)?;
            let size = std::fs::metadata(&audio).map(|meta| meta.len() as i64).ok();
            transaction.execute(
                "UPDATE transcriptions SET kind = 'file', source_path = ?2,
                        source_size_bytes = COALESCE(?3, source_size_bytes)
                 WHERE id = ?1",
                rusqlite::params![id, audio, size],
            )?;
        }
        transaction.execute(
            "DELETE FROM transcript_segments WHERE transcription_id = ?1",
            [id],
        )?;
        transaction.execute(
            "UPDATE transcriptions SET status = 'queued', started_ms = NULL, completed_ms = NULL,
                    duration_seconds = NULL, error = NULL, completed_chunks = 0, total_chunks = NULL,
                    cancel_requested = 0, start_requested = 0, queue_visible = 1
             WHERE id = ?1",
            [id],
        )?;
        transaction.commit()
    }

    pub fn set_duration(&mut self, id: &str, duration_seconds: f64) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE transcriptions SET duration_seconds = ?2 WHERE id = ?1",
            params![id, duration_seconds],
        )?;
        Ok(())
    }

    pub fn rename(&mut self, id: &str, name: &str) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET source_filename = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn set_audio_path(&mut self, id: &str, audio_path: Option<&str>) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE transcriptions SET audio_path = ?2 WHERE id = ?1",
            params![id, audio_path],
        )?;
        Ok(())
    }

    pub fn update_segment_text(
        &mut self,
        transcription_id: &str,
        index: usize,
        text: &str,
    ) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcript_segments SET text = ?3
             WHERE transcription_id = ?1 AND segment_index = ?2",
            params![transcription_id, index as i64, text],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn set_total_chunks(&mut self, id: &str, total_chunks: usize) -> rusqlite::Result<()> {
        self.connection.execute(
            "UPDATE transcriptions SET total_chunks = ?2 WHERE id = ?1",
            params![id, total_chunks],
        )?;
        Ok(())
    }
}

fn insert_transcription(
    connection: &Connection,
    transcription: &Transcription,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO transcriptions (
               id, kind, source_filename, source_path, source_size_bytes, source_modified_ms,
               created_ms, started_ms, completed_ms, status, duration_seconds, model_version,
                error, audio_path, queue_visible
              ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            transcription.id,
            transcription.kind.to_string(),
            transcription.source_filename,
            transcription.source_path,
            transcription.source_size_bytes,
            transcription.source_modified_ms,
            transcription.created_ms,
            transcription.started_ms,
            transcription.completed_ms,
            transcription.status.to_string(),
            transcription.duration_seconds,
            transcription.model_version,
            transcription.error,
            transcription.audio_path,
            transcription.queue_visible,
        ],
    )?;
    Ok(())
}

impl Store {
    pub fn transition(
        &mut self,
        id: &str,
        next: TranscriptionStatus,
        now_ms: i64,
        error: Option<&str>,
    ) -> rusqlite::Result<()> {
        let current: String = self.connection.query_row(
            "SELECT status FROM transcriptions WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let current =
            TranscriptionStatus::from_str(&current).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if !can_transition(current, next) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        match next {
            TranscriptionStatus::Queued => {
                self.connection.execute(
                    "UPDATE transcriptions SET status = 'queued', started_ms = NULL,
                            completed_ms = NULL, error = NULL, cancel_requested = 0,
                            queue_visible = 1 WHERE id = ?1",
                    [id],
                )?;
            }
            TranscriptionStatus::Preparing => {
                self.connection.execute(
                    "UPDATE transcriptions SET status = 'preparing', started_ms = ?2,
                            completed_ms = NULL, error = NULL, cancel_requested = 0 WHERE id = ?1",
                    params![id, now_ms],
                )?;
            }
            TranscriptionStatus::Transcribing => {
                self.connection.execute(
                    "UPDATE transcriptions SET status = 'transcribing', error = NULL WHERE id = ?1",
                    [id],
                )?;
            }
            TranscriptionStatus::Completed
            | TranscriptionStatus::Cancelled
            | TranscriptionStatus::Failed
            | TranscriptionStatus::Interrupted => {
                self.connection.execute(
                    "UPDATE transcriptions SET status = ?2, completed_ms = ?3, error = ?4 WHERE id = ?1",
                    params![id, next.to_string(), now_ms, error],
                )?;
            }
        }
        Ok(())
    }

    pub fn list_history(&self) -> rusqlite::Result<Vec<Transcription>> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM transcriptions ORDER BY created_ms DESC")?;
        statement.query_map([], map_transcription)?.collect()
    }

    pub fn list_queue(&self) -> rusqlite::Result<Vec<Transcription>> {
        let mut statement = self.connection.prepare(
            "SELECT * FROM transcriptions WHERE queue_visible = 1 ORDER BY created_ms, rowid",
        )?;
        statement.query_map([], map_transcription)?.collect()
    }

    pub fn has_active(&self) -> rusqlite::Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM transcriptions WHERE status IN ('preparing','transcribing'))",
            [],
            |row| row.get(0),
        )
    }

    pub fn append_segment(&mut self, segment: &TranscriptSegment) -> rusqlite::Result<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO transcript_segments (
               id, transcription_id, segment_index, start_seconds, end_seconds, text,
               source_track, speaker_id, state
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(transcription_id, segment_index) DO UPDATE SET
               id = excluded.id,
               start_seconds = excluded.start_seconds,
               end_seconds = excluded.end_seconds,
               text = excluded.text,
               source_track = excluded.source_track,
               speaker_id = excluded.speaker_id,
               state = excluded.state",
            params![
                segment.id,
                segment.transcription_id,
                segment.index,
                segment.start_seconds,
                segment.end_seconds,
                segment.text,
                source_track_value(segment.source_track),
                segment.speaker_id,
                segment_state_value(segment.state),
            ],
        )?;
        transaction.execute(
            "UPDATE transcriptions SET completed_chunks = MAX(completed_chunks, ?2)
             WHERE id = ?1",
            params![segment.transcription_id, segment.index + 1],
        )?;
        transaction.commit()
    }

    pub fn segments(&self, transcription_id: &str) -> rusqlite::Result<Vec<TranscriptSegment>> {
        let mut statement = self.connection.prepare(
            "SELECT id, transcription_id, segment_index, start_seconds, end_seconds, text,
                    source_track, speaker_id, state
             FROM transcript_segments WHERE transcription_id = ?1 ORDER BY start_seconds, segment_index",
        )?;
        statement
            .query_map([transcription_id], map_segment)?
            .collect()
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Transcription>> {
        self.connection
            .query_row(
                "SELECT * FROM transcriptions WHERE id = ?1",
                [id],
                map_transcription,
            )
            .optional()
    }

    pub fn search(&self, query: &str) -> rusqlite::Result<Vec<Transcription>> {
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        let mut statement = self.connection.prepare(
            "SELECT DISTINCT t.* FROM transcriptions t
             LEFT JOIN transcript_segments s ON s.transcription_id = t.id
             WHERE t.source_filename LIKE ?1 ESCAPE '\\' OR s.text LIKE ?1 ESCAPE '\\'
             ORDER BY t.created_ms DESC",
        )?;
        statement.query_map([pattern], map_transcription)?.collect()
    }

    pub fn recover_interrupted(&mut self, now_ms: i64) -> rusqlite::Result<usize> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS live_finalizations (
                transcription_id TEXT PRIMARY KEY REFERENCES transcriptions(id) ON DELETE CASCADE,
                state TEXT NOT NULL CHECK(state IN ('finalizing','completed','failed')),
                updated_ms INTEGER NOT NULL
            );",
        )?;
        self.connection.execute(
            "UPDATE live_finalizations SET state='failed',updated_ms=?1 WHERE state='finalizing'",
            [now_ms],
        )?;
        self.connection.execute(
            "UPDATE transcriptions SET status = 'interrupted', completed_ms = ?1,
                    error = 'Application stopped during transcription'
             WHERE status IN ('preparing','transcribing')",
            [now_ms],
        )
    }

    pub fn remove_from_queue(&mut self, id: &str, now_ms: i64) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "UPDATE transcriptions SET
               queue_visible = 0,
               completed_ms = CASE WHEN status = 'queued' THEN ?2 ELSE completed_ms END,
               status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END
             WHERE id = ?1 AND status IN ('queued','completed','cancelled','failed','interrupted')",
            params![id, now_ms],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn delete_history(&mut self, id: &str) -> rusqlite::Result<()> {
        let changed = self.connection.execute(
            "DELETE FROM transcriptions WHERE id = ?1
             AND status IN ('completed','cancelled','failed','interrupted')",
            [id],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }
}

pub fn can_transition(from: TranscriptionStatus, to: TranscriptionStatus) -> bool {
    use TranscriptionStatus::*;
    matches!(
        (from, to),
        (Queued, Preparing)
            | (Queued, Cancelled)
            | (Preparing, Transcribing)
            | (Preparing, Cancelled | Failed | Interrupted)
            | (Transcribing, Completed | Cancelled | Failed | Interrupted)
            | (Cancelled | Failed | Interrupted, Queued)
    )
}

fn source_track_value(value: SourceTrack) -> &'static str {
    match value {
        SourceTrack::File => "file",
        SourceTrack::Microphone => "microphone",
        SourceTrack::System => "system",
    }
}

fn segment_state_value(value: SegmentState) -> &'static str {
    match value {
        SegmentState::Provisional => "provisional",
        SegmentState::Final => "final",
    }
}

fn map_transcription(row: &rusqlite::Row<'_>) -> rusqlite::Result<Transcription> {
    let kind: String = row.get("kind")?;
    let status: String = row.get("status")?;
    Ok(Transcription {
        id: row.get("id")?,
        kind: kind.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        source_filename: row.get("source_filename")?,
        source_path: row.get("source_path")?,
        source_size_bytes: row.get("source_size_bytes")?,
        source_modified_ms: row.get("source_modified_ms")?,
        created_ms: row.get("created_ms")?,
        started_ms: row.get("started_ms")?,
        completed_ms: row.get("completed_ms")?,
        status: status.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        duration_seconds: row.get("duration_seconds")?,
        model_version: row.get("model_version")?,
        error: row.get("error")?,
        audio_path: row.get("audio_path")?,
        completed_chunks: row.get("completed_chunks")?,
        total_chunks: row.get("total_chunks")?,
        queue_visible: row.get("queue_visible")?,
    })
}

fn map_segment(row: &rusqlite::Row<'_>) -> rusqlite::Result<TranscriptSegment> {
    let track: String = row.get("source_track")?;
    let state: String = row.get("state")?;
    Ok(TranscriptSegment {
        id: row.get("id")?,
        transcription_id: row.get("transcription_id")?,
        index: row.get("segment_index")?,
        start_seconds: row.get("start_seconds")?,
        end_seconds: row.get("end_seconds")?,
        text: row.get("text")?,
        source_track: match track.as_str() {
            "file" => SourceTrack::File,
            "microphone" => SourceTrack::Microphone,
            "system" => SourceTrack::System,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
        speaker_id: row.get("speaker_id")?,
        state: match state.as_str() {
            "provisional" => SegmentState::Provisional,
            "final" => SegmentState::Final,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
    })
}

#[cfg(test)]
mod tests {
    #[test]fn drive_restore_uses_original_local_folder_mapping(){
        let mut store=super::Store::memory().unwrap();
        store.save_organization(serde_json::json!([{"id":"local-folder","kind":"folder","parent_id":null,"name":"Local folder","position":0}])).unwrap();
        let manifest=serde_json::json!({"device_id":"device","local_folder_mappings":{"cloud-folder":"local-folder"},"original_recording_id":"original","organization":[{"id":"cloud-folder","kind":"folder","parent_id":null,"name":"Remote name","position":0},{"id":"original","kind":"recording","parent_id":"cloud-folder","position":0}]});
        store.restore_snapshot_organized("mapped-remote",&transcription("mapped-local",TranscriptionStatus::Completed),&[],&manifest).unwrap();
        assert_eq!(store.list_folders().unwrap().len(),1);assert_eq!(store.list_folders().unwrap()[0].2,"Local folder");
        let nodes=store.load_organization().unwrap();let record=nodes.as_array().unwrap().iter().find(|node|node["id"]=="mapped-local").unwrap();assert_eq!(record["parent_id"],"local-folder");
    }
    #[test]fn drive_restore_keeps_folder_path_and_rolls_back_cycle(){
        let mut store=super::Store::memory().unwrap();let record=transcription("restored-tree",TranscriptionStatus::Completed);
        let mut manifest=serde_json::json!({"original_recording_id":"original","device_id":"device","revision":"one","organization":[{"id":"folder","kind":"folder","parent_id":"folder","name":"Folder","position":0},{"id":"original","kind":"recording","parent_id":"folder","position":2}]});
        assert!(store.restore_snapshot_organized("remote-tree",&record,&[],&manifest).is_err());assert!(store.get(&record.id).unwrap().is_none());
        manifest["organization"][0]["parent_id"]=serde_json::Value::Null;
        store.restore_snapshot_organized("remote-tree",&record,&[],&manifest).unwrap();
        let nodes=store.load_organization().unwrap();let nodes=nodes.as_array().unwrap();let folder=nodes.iter().find(|node|node["name"]=="Folder").unwrap();let placement=nodes.iter().find(|node|node["id"]==record.id).unwrap();assert_eq!(placement["parent_id"],folder["id"]);assert_eq!(placement["position"],2);
    }
    #[test]fn drive_restore_reuses_snapshot_folder_without_overwriting_local_rename(){
        let mut store=super::Store::memory().unwrap();
        let manifest=serde_json::json!({"original_recording_id":"original","device_id":"device","revision":"one","organization":[{"id":"folder","kind":"folder","parent_id":null,"name":"Folder","position":0},{"id":"original","kind":"recording","parent_id":"folder","position":0}]});
        let first=transcription("restore-first",TranscriptionStatus::Completed);
        store.restore_snapshot_organized("remote-first",&first,&[],&manifest).unwrap();
        store.connection.execute("UPDATE recording_folders SET name='Local rename'",[]).unwrap();
        let second=transcription("restore-second",TranscriptionStatus::Completed);
        let mut next_manifest=manifest.clone();next_manifest["revision"]="two".into();
        store.restore_snapshot_organized("remote-second",&second,&[],&next_manifest).unwrap();
        let nodes=store.load_organization().unwrap();let nodes=nodes.as_array().unwrap();
        let folders:Vec<_>=nodes.iter().filter(|node|node["kind"]=="folder").collect();assert_eq!(folders.len(),1);assert_eq!(folders[0]["name"],"Local rename");
        let placements:Vec<_>=nodes.iter().filter(|node|node["kind"]=="recording").collect();assert_eq!(placements.len(),2);assert_eq!(placements[0]["parent_id"],placements[1]["parent_id"]);
    }
    #[test]fn drive_restore_preserves_empty_nested_folders(){
        let mut store=super::Store::memory().unwrap();let record=transcription("restore-empty",TranscriptionStatus::Completed);
        let manifest=serde_json::json!({"device_id":"device","organization":[{"id":"child","kind":"folder","parent_id":"parent","name":"Empty child","position":0},{"id":"parent","kind":"folder","parent_id":null,"name":"Empty parent","position":0}]});
        store.restore_snapshot_organized("empty-remote",&record,&[],&manifest).unwrap();let nodes=store.load_organization().unwrap();let nodes=nodes.as_array().unwrap();let parent=nodes.iter().find(|node|node["name"]=="Empty parent").unwrap();let child=nodes.iter().find(|node|node["name"]=="Empty child").unwrap();assert_eq!(child["parent_id"],parent["id"]);
    }
    #[test]fn drive_remote_rename_updates_only_unchanged_local_folder(){
        let mut store=super::Store::memory().unwrap();let mut manifest=serde_json::json!({"device_id":"device","organization":[{"id":"folder","kind":"folder","parent_id":null,"name":"Before","position":0}]});
        store.restore_snapshot_organized("rename-one",&transcription("rename-one",TranscriptionStatus::Completed),&[],&manifest).unwrap();
        manifest["organization"][0]["name"]="Remote rename".into();store.restore_snapshot_organized("rename-two",&transcription("rename-two",TranscriptionStatus::Completed),&[],&manifest).unwrap();
        assert_eq!(store.list_folders().unwrap()[0].2,"Remote rename");
        store.connection.execute("UPDATE recording_folders SET name='Local edit'",[]).unwrap();manifest["organization"][0]["name"]="Other remote edit".into();
        store.restore_snapshot_organized("rename-three",&transcription("rename-three",TranscriptionStatus::Completed),&[],&manifest).unwrap();assert_eq!(store.list_folders().unwrap()[0].2,"Local edit");
    }
    #[test]fn drive_remote_move_preserves_concurrent_local_position(){
        let mut store=super::Store::memory().unwrap();let mut manifest=serde_json::json!({"device_id":"device","organization":[{"id":"a","kind":"folder","parent_id":null,"name":"A","position":0},{"id":"b","kind":"folder","parent_id":null,"name":"B","position":1}]});
        store.restore_snapshot_organized("move-one",&transcription("move-one",TranscriptionStatus::Completed),&[],&manifest).unwrap();
        manifest["organization"][1]["parent_id"]="a".into();manifest["organization"][1]["position"]=3.into();
        store.restore_snapshot_organized("move-two",&transcription("move-two",TranscriptionStatus::Completed),&[],&manifest).unwrap();let folders=store.list_folders().unwrap();let b=folders.iter().find(|f|f.2=="B").unwrap();assert!(b.1.is_some());assert_eq!(b.3,3);
        store.connection.execute("UPDATE recording_folders SET parent_id=NULL,position=9 WHERE name='B'",[]).unwrap();manifest["organization"][1]["position"]=4.into();
        store.restore_snapshot_organized("move-three",&transcription("move-three",TranscriptionStatus::Completed),&[],&manifest).unwrap();let folders=store.list_folders().unwrap();let b=folders.iter().find(|f|f.2=="B").unwrap();assert!(b.1.is_none());assert_eq!(b.3,9);
    }
    #[test]fn drive_folder_identity_survives_second_device_roundtrip(){
        let mut store=super::Store::memory().unwrap();let first=serde_json::json!({"device_id":"device-a","organization":[{"id":"folder","kind":"folder","parent_id":null,"name":"Folder","position":0}]});
        store.restore_snapshot_organized("roundtrip-a",&transcription("roundtrip-a",TranscriptionStatus::Completed),&[],&first).unwrap();
        let organization=store.load_organization().unwrap();let second=serde_json::json!({"device_id":"device-b","organization":organization});
        store.restore_snapshot_organized("roundtrip-b",&transcription("roundtrip-b",TranscriptionStatus::Completed),&[],&second).unwrap();assert_eq!(store.list_folders().unwrap().len(),1);
    }
    #[test]fn drive_restore_is_atomic_and_idempotent(){
        let mut store=super::Store::memory().unwrap();let record=transcription("restored",TranscriptionStatus::Completed);
        let segment=TranscriptSegment::new("restored",0,0.0,1.0,"text".into(),SourceTrack::File,SegmentState::Final).unwrap();
        let mut invalid=segment.clone();invalid.transcription_id="wrong".into();
        assert!(store.restore_snapshot("remote",&record,&[invalid]).is_err());assert!(store.get("restored").unwrap().is_none());
        assert_eq!(store.restore_snapshot("remote",&record,&[segment.clone()]).unwrap(),"restored");
        let another=transcription("duplicate",TranscriptionStatus::Completed);
        assert_eq!(store.restore_snapshot("remote",&another,&[]).unwrap(),"restored");assert!(store.get("duplicate").unwrap().is_none());assert_eq!(store.segments("restored").unwrap(),vec![segment]);
    }
    #[test]
    fn clearing_transcript_keeps_recording_and_audio() {
        let mut store = super::Store::memory().unwrap();
        let mut recording = transcription("clear-text", TranscriptionStatus::Completed);
        recording.audio_path = Some("archive.ogg".into());
        store.enqueue(&recording).unwrap();
        store.append_segment(&TranscriptSegment::new("clear-text", 0, 0.0, 1.0, "text".into(), SourceTrack::File, SegmentState::Final).unwrap()).unwrap();
        store.clear_transcript("clear-text").unwrap();
        assert!(store.segments("clear-text").unwrap().is_empty());
        assert_eq!(store.get("clear-text").unwrap().unwrap().audio_path, recording.audio_path);
    }
    #[test]
    fn organization_stale_writer_cannot_overwrite_new_folder() {
        let mut store = super::Store::memory().unwrap();
        let previous = store.load_organization().unwrap();
        let next = serde_json::json!([{"id":"new","kind":"folder","parent_id":null,"name":"New","position":0}]);
        store.save_organization_if_unchanged(previous.clone(), next.clone()).unwrap();
        assert!(store.save_organization_if_unchanged(previous, serde_json::json!([])).is_err());
        assert_eq!(store.load_organization().unwrap(), next);
    }
    #[test]
    fn organization_migration_and_moves_preserve_recordings_segments_and_queue() {
        let mut store = super::Store::memory().unwrap();
        let mut original = transcription("recording", TranscriptionStatus::Queued);
        original.audio_path = Some("preserved.ogg".into());
        store.enqueue(&original).unwrap();
        let segment = TranscriptSegment::new("recording", 0, 0.0, 1.0, "original text".into(), SourceTrack::File, SegmentState::Final).unwrap();
        store.append_segment(&segment).unwrap();
        let queue = store.list_queue().unwrap();
        store.migrate().unwrap();
        let nodes = store.load_organization().unwrap();
        assert_eq!(nodes[0]["id"], "recording");
        assert!(nodes[0]["parent_id"].is_null());
        let organized = serde_json::json!([
            {"id":"folder","kind":"folder","parent_id":null,"name":"Folder","position":0},
            {"id":"recording","kind":"recording","parent_id":"folder","name":"recording.wav","position":0}
        ]);
        store.save_organization(organized).unwrap();
        let saved = store.get("recording").unwrap().unwrap();
        assert_eq!(saved.audio_path, original.audio_path);
        assert_eq!(saved.source_path, original.source_path);
        assert_eq!(store.segments("recording").unwrap(), [segment]);
        assert_eq!(store.list_queue().unwrap().iter().map(|row| &row.id).collect::<Vec<_>>(), queue.iter().map(|row| &row.id).collect::<Vec<_>>());
        // A filtered/stale snapshot must not silently remove this recording's placement.
        assert!(store.save_organization(serde_json::json!([])).is_err());
        assert_eq!(store.load_organization().unwrap()[1]["parent_id"], "folder");
    }
    #[test]
    fn folder_identity_survives_database_reopen_and_repeated_migration() {
        let path = std::env::temp_dir().join(format!("gigaam-organization-{}-{}.sqlite", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            let mut store = super::Store { connection };
            store.migrate().unwrap();
            store.save_organization(serde_json::json!([{"id":"folder","kind":"folder","parent_id":null,"name":"Folder","position":0}])).unwrap();
        }
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            let mut store = super::Store { connection };
            store.migrate().unwrap();
            let nodes = store.load_organization().unwrap();
            assert_eq!(nodes[0]["id"], "folder");
            assert_eq!(nodes[0]["name"], "Folder");
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn organization_rejects_cycles_and_rolls_back_invalid_recordings() {
        let mut store = super::Store::memory().unwrap();
        let valid = serde_json::json!([
            {"id":"parent","kind":"folder","parent_id":null,"name":"Parent","position":0},
            {"id":"child","kind":"folder","parent_id":"parent","name":"Child","position":0}
        ]);
        store.save_organization(valid.clone()).unwrap();
        let saved = store.load_organization().unwrap();
        assert_eq!(saved.as_array().unwrap().len(), 2);
        let cycle = serde_json::json!([
            {"id":"parent","kind":"folder","parent_id":"child","name":"Parent","position":0},
            {"id":"child","kind":"folder","parent_id":"parent","name":"Child","position":0}
        ]);
        assert!(store.save_organization(cycle).is_err());
        let invalid = serde_json::json!([{"id":"missing","kind":"recording","parent_id":null,"name":"Missing","position":0}]);
        assert!(store.save_organization(invalid).is_err());
        assert_eq!(store.load_organization().unwrap(), saved);
    }

    use super::*;
    use crate::domain::TranscriptionKind;

    fn transcription(id: &str, status: TranscriptionStatus) -> Transcription {
        Transcription {
            id: id.into(),
            kind: TranscriptionKind::File,
            source_filename: "recording.wav".into(),
            source_path: "C:\\missing\\recording.wav".into(),
            source_size_bytes: 100,
            source_modified_ms: 1,
            created_ms: 2,
            started_ms: None,
            completed_ms: None,
            status,
            duration_seconds: None,
            model_version: "v3_e2e_ctc.int8".into(),
            error: None,
            audio_path: None,
            completed_chunks: 0,
            total_chunks: None,
            queue_visible: true,
        }
    }

    #[test]
    fn persists_segments_and_searches_without_source_file() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        let segment = TranscriptSegment::new(
            "tx",
            0,
            1.0,
            2.0,
            "важный текст".into(),
            SourceTrack::File,
            SegmentState::Final,
        )
        .unwrap();
        store.append_segment(&segment).unwrap();
        assert_eq!(store.segments("tx").unwrap(), [segment]);
        assert_eq!(store.search("важный").unwrap()[0].id, "tx");
    }

    #[test]
    fn segment_text_updates_in_place() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        let segment = TranscriptSegment::new(
            "tx",
            0,
            1.0,
            2.0,
            "до".into(),
            SourceTrack::File,
            SegmentState::Final,
        )
        .unwrap();
        store.append_segment(&segment).unwrap();
        store.update_segment_text("tx", 0, "после").unwrap();
        assert_eq!(store.segments("tx").unwrap()[0].text, "после");
        assert_eq!(store.search("после").unwrap()[0].id, "tx");
        assert!(store.update_segment_text("tx", 7, "мимо").is_err());
        assert!(store.update_segment_text("нет", 0, "мимо").is_err());
    }

    #[test]
    fn live_archive_metadata_rolls_back_on_injected_write_failure() {
        let mut store = Store::memory().unwrap();
        store.enqueue(&transcription("tx", TranscriptionStatus::Queued)).unwrap();
        store.transition("tx", TranscriptionStatus::Preparing, 1, None).unwrap();
        store.transition("tx", TranscriptionStatus::Transcribing, 2, None).unwrap();
        store.connection.execute_batch(
            "CREATE TRIGGER injected_disk_failure BEFORE UPDATE OF audio_path ON transcriptions
             BEGIN SELECT RAISE(ABORT, 'injected storage failure'); END;"
        ).unwrap();
        assert!(store.complete_live_archive("tx", 12.0, Some("preserved.wav"), 3).is_err());
        let item = store.get("tx").unwrap().unwrap();
        assert_eq!(item.status, TranscriptionStatus::Transcribing);
        assert!(item.audio_path.is_none());
        assert!(item.duration_seconds.is_none());
    }

    #[test]
    fn live_archive_completion_is_atomic_and_honors_cancellation() {
        let mut store = Store::memory().unwrap();
        store.enqueue(&transcription("tx", TranscriptionStatus::Queued)).unwrap();
        store.transition("tx", TranscriptionStatus::Preparing, 1, None).unwrap();
        store.transition("tx", TranscriptionStatus::Transcribing, 2, None).unwrap();
        store.request_cancel("tx", 2).unwrap();
        assert!(store.complete_live_archive("tx", 3.0, Some("audio.wav"), 3).is_err());
        let item = store.get("tx").unwrap().unwrap();
        assert_eq!(item.status, TranscriptionStatus::Transcribing);
        assert!(item.audio_path.is_none());
    }

    #[test]
    fn finalization_state_survives_and_is_failed_on_recovery() {
        let mut store = Store::memory().unwrap();
        store.enqueue(&transcription("tx", TranscriptionStatus::Queued)).unwrap();
        store.begin_live_finalization("tx", 1).unwrap();
        let state: String = store.connection.query_row("SELECT state FROM live_finalizations WHERE transcription_id='tx'", [], |row| row.get(0)).unwrap();
        assert_eq!(state, "finalizing");
        store.recover_interrupted(2).unwrap();
        let state: String = store.connection.query_row("SELECT state FROM live_finalizations WHERE transcription_id='tx'", [], |row| row.get(0)).unwrap();
        assert_eq!(state, "failed");
    }

    #[test]
    fn recovers_interrupted_work_and_allows_retry() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        store
            .transition("tx", TranscriptionStatus::Preparing, 3, None)
            .unwrap();
        store
            .transition("tx", TranscriptionStatus::Transcribing, 4, None)
            .unwrap();
        assert_eq!(store.recover_interrupted(5).unwrap(), 1);
        assert_eq!(
            store.get("tx").unwrap().unwrap().status,
            TranscriptionStatus::Interrupted
        );
        store
            .transition("tx", TranscriptionStatus::Queued, 6, None)
            .unwrap();
        assert_eq!(
            store.get("tx").unwrap().unwrap().status,
            TranscriptionStatus::Queued
        );
    }

    #[test]
    fn retry_from_completed_clears_segments_and_requeues() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        store.request_start("tx").unwrap();
        let job = store.claim_next(3).unwrap().unwrap();
        assert_eq!(job.status, TranscriptionStatus::Preparing);
        store
            .transition(&job.id, TranscriptionStatus::Transcribing, 4, None)
            .unwrap();
        assert!(store.complete_if_not_cancelled(&job.id, 2.0, 5).unwrap());
        store.reset_for_retry(&job.id).unwrap();
        let retried = store.get(&job.id).unwrap().unwrap();
        assert_eq!(retried.status, TranscriptionStatus::Queued);
        assert_eq!(retried.completed_chunks, 0);
        store.request_start(&job.id).unwrap();
        assert_eq!(store.claim_next(6).unwrap().unwrap().id, job.id);
    }

    #[test]
    fn retry_converts_live_session_into_file_job() {
        let mut store = Store::memory().unwrap();
        let dir = std::env::temp_dir().join(format!("gigaam-retry-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("microphone.flac");
        std::fs::write(&audio, b"fake-flac").unwrap();
        let mut tx = transcription("live1", TranscriptionStatus::Failed);
        tx.kind = TranscriptionKind::Live;
        tx.audio_path = Some(audio.to_string_lossy().into_owned());
        store.enqueue(&tx).unwrap();
        store.reset_for_retry("live1").unwrap();
        let retried = store.get("live1").unwrap().unwrap();
        assert_eq!(retried.kind, TranscriptionKind::File);
        assert_eq!(retried.source_path, audio.to_string_lossy());
        assert_eq!(retried.status, TranscriptionStatus::Queued);
        store.request_start("live1").unwrap();
        assert_eq!(store.claim_next(9).unwrap().unwrap().id, "live1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retry_live_without_audio_is_rejected() {
        let mut store = Store::memory().unwrap();
        let mut tx = transcription("live2", TranscriptionStatus::Failed);
        tx.kind = TranscriptionKind::Live;
        tx.audio_path = None;
        store.enqueue(&tx).unwrap();
        assert!(store.reset_for_retry("live2").is_err());
    }

    #[test]
    fn queue_worker_never_claims_live_rows() {
        let mut store = Store::memory().unwrap();
        let mut tx = transcription("live3", TranscriptionStatus::Queued);
        tx.kind = TranscriptionKind::Live;
        store.enqueue(&tx).unwrap();
        store.request_start("live3").unwrap();
        assert!(store.claim_next(9).unwrap().is_none());
    }

    #[test]
    fn rejects_invalid_queue_transition() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        assert!(
            store
                .transition("tx", TranscriptionStatus::Completed, 3, None)
                .is_err()
        );
    }

    #[test]
    fn queue_is_fifo_and_retry_resets_attempt_fields() {
        let mut store = Store::memory().unwrap();
        let mut first = transcription("first", TranscriptionStatus::Queued);
        first.created_ms = 1;
        let mut second = transcription("second", TranscriptionStatus::Queued);
        second.created_ms = 2;
        store.enqueue(&second).unwrap();
        store.enqueue(&first).unwrap();
        store.request_start("first").unwrap();
        assert_eq!(store.claim_next(3).unwrap().unwrap().id, "first");
        store
            .transition(
                "first",
                TranscriptionStatus::Cancelled,
                4,
                Some("cancelled"),
            )
            .unwrap();
        store
            .transition("first", TranscriptionStatus::Queued, 5, None)
            .unwrap();
        let retried = store.get("first").unwrap().unwrap();
        assert_eq!(retried.status, TranscriptionStatus::Queued);
        assert_eq!(retried.started_ms, None);
        assert_eq!(retried.completed_ms, None);
        assert_eq!(retried.error, None);
    }

    #[test]
    fn renames_transcription_without_changing_source_path() {
        let mut store = Store::memory().unwrap();
        let original = transcription("rename-me", TranscriptionStatus::Completed);
        let source_path = original.source_path.clone();
        store.enqueue(&original).unwrap();
        store.rename("rename-me", "Встреча команды").unwrap();
        let renamed = store.get("rename-me").unwrap().unwrap();
        assert_eq!(renamed.source_filename, "Встреча команды");
        assert_eq!(renamed.source_path, source_path);
    }

    #[test]
    fn clearing_queue_preserves_history_and_segments() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        let segment = TranscriptSegment::new(
            "tx",
            0,
            0.0,
            1.0,
            "text".into(),
            SourceTrack::File,
            SegmentState::Final,
        )
        .unwrap();
        store.append_segment(&segment).unwrap();
        store
            .transition("tx", TranscriptionStatus::Cancelled, 3, None)
            .unwrap();
        store.remove_from_queue("tx", 4).unwrap();
        assert!(store.list_queue().unwrap().is_empty());
        assert_eq!(store.list_history().unwrap().len(), 1);
        assert_eq!(store.segments("tx").unwrap(), [segment]);
    }

    #[test]
    fn deleting_history_cascades_segments() {
        let mut store = Store::memory().unwrap();
        store
            .enqueue(&transcription("tx", TranscriptionStatus::Queued))
            .unwrap();
        let segment = TranscriptSegment::new(
            "tx",
            0,
            0.0,
            1.0,
            "text".into(),
            SourceTrack::File,
            SegmentState::Final,
        )
        .unwrap();
        store.append_segment(&segment).unwrap();
        store
            .transition("tx", TranscriptionStatus::Cancelled, 3, None)
            .unwrap();
        store.delete_history("tx").unwrap();
        assert!(store.segments("tx").unwrap().is_empty());
    }
}
