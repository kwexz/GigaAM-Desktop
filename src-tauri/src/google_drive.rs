//! Desktop public-client OAuth. Credentials never cross the IPC boundary.
use std::{io::{Read, Write}, net::TcpListener, time::{Duration, Instant}};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};
use serde::Deserialize;

pub const CLIENT_ID: &str = "591054714419-laosoks26n377qioh8pfr0dirhamocsq.apps.googleusercontent.com";
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
fn client_secret() -> Option<String> {
    if let Some(secret) = std::env::var("GIGAAM_GOOGLE_DESKTOP_CLIENT_SECRET").ok().filter(|value| !value.trim().is_empty()) { return Some(secret); }
    // Desktop credential downloaded by the owner; never read Web credentials
    // or expose this file through IPC/logging.
    let root = std::env::var_os("APPDATA")?;
    let path = std::path::PathBuf::from(root).join("ru.gigaam.desktop").join(format!("client_secret_{CLIENT_ID}.json"));
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > 16384 { return None; }
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if value["installed"]["client_id"].as_str()? != CLIENT_ID { return None; }
    value["installed"]["client_secret"].as_str().filter(|value| !value.trim().is_empty()).map(str::to_owned)
}
fn token_error(status: reqwest::StatusCode, body: &str) -> String {
    let code = serde_json::from_str::<serde_json::Value>(body).ok().and_then(|value| value["error"].as_str().map(str::to_owned));
    match code.as_deref() {
        Some("invalid_client") => "Google token exchange: invalid_client. Configure the Desktop OAuth client's client_secret locally; do not use Web client credentials.".into(),
        Some("invalid_grant") => "Google token exchange: invalid_grant. Start a new sign-in; the code may have expired or PKCE/redirect did not match.".into(),
        _ => format!("Google token exchange failed (HTTP {}). No authorization code or tokens were logged.", status.as_u16()),
    }
}

fn drive_error(status:reqwest::StatusCode,operation:&str)->String {
    if status==reqwest::StatusCode::UNAUTHORIZED {"Google access expired or was revoked. Reconnect Drive.".into()}
    else {format!("{operation} (HTTP {})",status.as_u16())}
}

pub fn validate_manifest(value:&serde_json::Value)->Result<(),String>{
    if value["version"]!=1{return Err("Unsupported backup version".into());}
    if let Some(parents)=value.get("merge_parents") {
        let parents=parents.as_array().ok_or("Invalid merge parents")?;
        if parents.len()<2||parents.len()>32{return Err("Invalid merge parent count".into());}
        let revision=value["revision"].as_str().ok_or("Merge revision missing")?;
        let mut seen=std::collections::HashSet::new();
        for parent in parents{
            let parent=parent.as_str().filter(|v|!v.is_empty()&&v.len()<=256).ok_or("Invalid merge parent")?;
            if parent==revision||!seen.insert(parent){return Err("Cyclic or duplicate merge parent".into());}
        }
    }
    let audio=value["audio"].as_array().ok_or("Backup audio missing")?;
    if audio.is_empty()||audio.len()>8{return Err("Invalid backup track count".into());}
    let mut names=std::collections::HashSet::new();
    for track in audio{
        let name=track["original_filename"].as_str().ok_or("Track filename missing")?;
        if name.is_empty()||name.len()>240||name.contains(['/', '\\'])||name.contains("..")||!names.insert(name){return Err("Unsafe or duplicate backup filename".into());}
        let hash=track["sha256"].as_str().ok_or("Backup hash missing")?;
        if hash.len()!=64||!hash.bytes().all(|c|c.is_ascii_hexdigit()){return Err("Invalid backup hash".into());}
        if track["bytes"].as_u64().filter(|size|*size<=512*1024*1024).is_none(){return Err("Invalid backup audio size".into());}
        let id=track["file_id"].as_str().ok_or("Track ID missing")?;
        if id.is_empty()||!id.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_'||c==b'-'){return Err("Invalid Drive file ID".into());}
    }
    if let Some(tracks)=value["tracks"].as_array(){for track in tracks{
        let name=track["file"].as_str().ok_or("Offset filename missing")?;
        if !names.contains(name)||track["offset_seconds"].as_f64().filter(|v|v.is_finite()&&*v>=0.0).is_none(){return Err("Invalid track offset".into());}
    }}
    Ok(())
}
fn random_secret() -> String { let mut bytes = [0u8;32]; rand::rng().fill_bytes(&mut bytes); URL_SAFE_NO_PAD.encode(bytes) }
fn credential() -> Result<keyring::Entry,String> { keyring::Entry::new("GigaAM Desktop", "google-drive-refresh-token").map_err(|_| "Credential store unavailable".into()) }

pub fn connected() -> bool { credential().and_then(|entry| entry.get_password().map_err(|_| "No credential".into())).is_ok() }

fn access_token(client: &reqwest::blocking::Client) -> Result<String,String> {
    let refresh = credential()?.get_password().map_err(|_| "Connect Google Drive first")?;
    let mut form = vec![("client_id",CLIENT_ID.to_owned()),("grant_type","refresh_token".into()),("refresh_token",refresh)];
    if let Some(secret) = client_secret() { form.push(("client_secret",secret)); }
    let response = client.post("https://oauth2.googleapis.com/token").form(&form).send().map_err(|_| "Google token refresh unavailable")?;
    if !response.status().is_success() { let status=response.status();let body=response.text().unwrap_or_default();return Err(token_error(status,&body)); }
    #[derive(Deserialize)] struct Access { access_token: String }
    let access: Access = serde_json::from_str(&response.text().map_err(|_| "Token response unavailable")?).map_err(|_| "Invalid token response")?;
    Ok(access.access_token)
}

/// Creates only an application-owned folder; never requests broad Drive access.
pub fn ensure_folder() -> Result<String,String> {
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_| "HTTP setup failed")?;
    let token = access_token(&client)?;
    let mut endpoint = url::Url::parse("https://www.googleapis.com/drive/v3/files").unwrap();
    endpoint.query_pairs_mut().extend_pairs([("q","trashed = false and mimeType = 'application/vnd.google-apps.folder' and appProperties has { key='gigaam' and value='root-v1' }"),("fields","files(id)"),("pageSize","1")]);
    let response = client.get(endpoint)
        .bearer_auth(&token)
        .send().map_err(|_| "Google Drive unavailable")?;
    if !response.status().is_success() { return Err(drive_error(response.status(),"Could not discover application folder")); }
    let result: serde_json::Value = serde_json::from_str(&response.text().map_err(|_| "Drive response unavailable")?).map_err(|_| "Invalid Drive response")?;
    if let Some(id) = result["files"][0]["id"].as_str() { return Ok(id.to_owned()); }
    let body = serde_json::json!({"name":"GigaAM Desktop","mimeType":"application/vnd.google-apps.folder","appProperties":{"gigaam":"root-v1"}});
    let response = client.post("https://www.googleapis.com/drive/v3/files").bearer_auth(&token).header("Content-Type","application/json").body(body.to_string()).send().map_err(|_| "Could not create Drive folder")?;
    if !response.status().is_success() { return Err(drive_error(response.status(),"Google rejected application folder creation")); }
    let result: serde_json::Value = serde_json::from_str(&response.text().map_err(|_| "Drive response unavailable")?).map_err(|_| "Invalid Drive response")?;
    result["id"].as_str().map(str::to_owned).ok_or("Drive folder ID missing".into())
}

/// Resumable streaming upload with content verification. Existing snapshots are
/// immutable: a failed attempt cannot overwrite a previously valid backup.
pub fn upload_snapshot(folder: &str, key: &str, name: &str, path: &std::path::Path) -> Result<String,String> {
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(600)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_| "HTTP setup failed")?;
    let token = access_token(&client)?;
    let size = std::fs::metadata(path).map_err(|_| "Backup source unavailable")?.len();
    // Reconcile a previous successful PUT whose response was lost before the
    // durable local acknowledgement. Never invent a new snapshot identity.
    if !key.bytes().all(|c|c.is_ascii_alphanumeric()||b":-_".contains(&c)) {return Err("Invalid snapshot identity".into());}
    let mut lookup=url::Url::parse("https://www.googleapis.com/drive/v3/files").unwrap();
    lookup.query_pairs_mut().append_pair("q",&format!("'{folder}' in parents and trashed = false and appProperties has {{ key='gigaamSnapshot' and value='{key}' }}")).append_pair("fields","files(id,size)").append_pair("pageSize","2");
    let previous=client.get(lookup).bearer_auth(&token).send().map_err(|_|"Could not reconcile snapshot retry")?;
    if !previous.status().is_success(){return Err("Snapshot retry discovery failed".into());}
    let previous:serde_json::Value=serde_json::from_str(&previous.text().map_err(|_|"Snapshot discovery unavailable")?).map_err(|_|"Invalid snapshot discovery")?;
    if let Some(id)=previous["files"][0]["id"].as_str(){
        // Existing object is not accepted solely by matching metadata. Download
        // it and compare to the immutable local payload before acknowledging.
        let temporary=path.with_extension(format!("verify-{}",crate::new_id()));
        download_file(id,&temporary,size)?;
        let compare=(||->Result<bool,String>{
            let mut left=std::fs::File::open(path).map_err(|_|"Local payload unavailable")?;let mut right=std::fs::File::open(&temporary).map_err(|_|"Remote payload unavailable")?;
            let mut a=[0u8;65536];let mut b=[0u8;65536];
            loop{let n=left.read(&mut a).map_err(|_|"Local verification failed")?;right.read_exact(&mut b[..n]).map_err(|_|"Remote payload truncated")?;if a[..n]!=b[..n]{return Ok(false);}if n==0{return Ok(right.read(&mut b[..1]).map_err(|_|"Remote verification failed")?==0);}}
        })();let _=std::fs::remove_file(temporary);
        if compare?{return Ok(id.to_owned());}return Err("Snapshot identity conflict; both copies retained".into());
    }
    let metadata = serde_json::json!({"name":name,"parents":[folder],"appProperties":{"gigaamSnapshot":key}});
    let response = client.post("https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable&fields=id,size")
        .bearer_auth(&token).header("Content-Type","application/json").header("X-Upload-Content-Length",size).body(metadata.to_string()).send().map_err(|_| "Drive upload initialization failed")?;
    if !response.status().is_success() { return Err(drive_error(response.status(),"Drive rejected upload initialization")); }
    let location = response.headers().get(reqwest::header::LOCATION).and_then(|value| value.to_str().ok()).ok_or("Drive upload URL missing")?;
    let location = url::Url::parse(location).map_err(|_| "Invalid upload URL")?;
    if location.scheme() != "https" || location.host_str() != Some("www.googleapis.com") { return Err("Untrusted Drive upload URL".into()); }
    let file = std::fs::File::open(path).map_err(|_| "Backup source unavailable")?;
    let response = client.put(location).bearer_auth(&token).header(reqwest::header::CONTENT_LENGTH,size).body(file).send().map_err(|_| "Drive upload interrupted; local backup retained")?;
    if !response.status().is_success() { return Err(drive_error(response.status(),"Drive upload failed; local backup retained")); }
    let result: serde_json::Value = serde_json::from_str(&response.text().map_err(|_| "Upload response unavailable")?).map_err(|_| "Invalid upload response")?;
    let id = result["id"].as_str().ok_or("Uploaded file ID missing")?.to_owned();
    // Independent remote readback: count and SHA256 compare without holding audio in RAM.
    let mut local = std::fs::File::open(path).map_err(|_| "Backup source unavailable")?;
    let mut expected = Sha256::new(); let mut buffer = [0u8;65536];
    loop { let n=local.read(&mut buffer).map_err(|_| "Local verification failed")?; if n==0 {break;} expected.update(&buffer[..n]); }
    let mut response = client.get(format!("https://www.googleapis.com/drive/v3/files/{id}?alt=media")).bearer_auth(&token).send().map_err(|_| "Remote verification unavailable")?;
    if !response.status().is_success() { return Err("Remote verification failed".into()); }
    let mut actual=Sha256::new(); let mut received=0u64;
    loop { let n=response.read(&mut buffer).map_err(|_| "Remote verification interrupted")?; if n==0 {break;} received+=n as u64; if received>size{return Err("Remote file size mismatch".into());} actual.update(&buffer[..n]); }
    if received!=size || actual.finalize()!=expected.finalize() { return Err("Remote integrity mismatch; local data retained".into()); }
    Ok(id)
}

#[derive(Deserialize, serde::Serialize, Clone)]
pub struct RemoteFile { pub id: String, pub name: String }

#[derive(serde::Serialize)]
pub struct SnapshotBranch { pub file_id:String,pub original_id:String,pub revision:String,pub concurrent:bool }

fn branches_for_group(original:&str,values:Vec<(String,serde_json::Value)>)->Vec<SnapshotBranch>{
    let manifests:Vec<_>=values.iter().map(|(_,value)|value.clone()).collect();
    let heads=crate::drive_outbox::conflict_heads(&manifests);let concurrent=heads.len()>1;
    values.into_iter().filter_map(|(file_id,value)|{
        let revision=value["revision"].as_str()?;
        heads.iter().any(|head|head==revision).then(||SnapshotBranch{file_id,original_id:original.into(),revision:revision.into(),concurrent})
    }).collect()
}

pub fn discover_branches()->Result<Vec<SnapshotBranch>,String>{
    let files=list_snapshots()?;
    let root=std::env::temp_dir().join(format!("gigaam-drive-discovery-{}",crate::new_id()));
    std::fs::create_dir_all(&root).map_err(|_|"Could not stage snapshot discovery")?;
    let result=(||{
        let mut groups:std::collections::HashMap<String,Vec<(String,serde_json::Value)>>=Default::default();
        let mut manifest_bytes=0u64;
        for file in files{
            let path=root.join(crate::new_id());download_file(&file.id,&path,8*1024*1024)?;
            manifest_bytes=manifest_bytes.saturating_add(std::fs::metadata(&path).map_err(|_|"Discovery manifest unavailable")?.len());
            if manifest_bytes>64*1024*1024{return Err("Snapshot discovery exceeds 64 MiB metadata budget; no partial conflict result was accepted".into());}
            let value:serde_json::Value=serde_json::from_slice(&std::fs::read(&path).map_err(|_|"Discovery manifest unavailable")?).map_err(|_|"Invalid snapshot manifest")?;
            validate_manifest(&value)?;
            let _=std::fs::remove_file(&path);
            let original=value["original_recording_id"].as_str().or_else(||value["recording"]["id"].as_str()).ok_or("Snapshot identity missing")?.to_owned();
            groups.entry(original).or_default().push((file.id,value));
        }
        let mut branches=Vec::new();
        for(original,values)in groups{
            branches.extend(branches_for_group(&original,values));
        }
        Ok(branches)
    })();
    let _=std::fs::remove_dir_all(root);result
}

pub fn list_snapshots() -> Result<Vec<RemoteFile>,String> {
    let folder = ensure_folder()?;
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"HTTP setup failed")?;
    let token = access_token(&client)?;
    let mut files = Vec::new(); let mut next = String::new();
    loop {
        let mut endpoint=url::Url::parse("https://www.googleapis.com/drive/v3/files").unwrap();
        endpoint.query_pairs_mut().append_pair("q",&format!("'{folder}' in parents and trashed = false and appProperties has {{ key='gigaamSnapshot' }}"))
            .append_pair("fields","nextPageToken,files(id,name)").append_pair("pageSize","100");
        if !next.is_empty(){endpoint.query_pairs_mut().append_pair("pageToken",&next);}
        let response=client.get(endpoint).bearer_auth(&token).send().map_err(|_|"Drive discovery unavailable")?;
        if !response.status().is_success(){return Err("Drive discovery rejected".into());}
        #[derive(Deserialize)] struct Page { files:Vec<RemoteFile>, #[serde(rename="nextPageToken")] next:Option<String> }
        let page:Page=serde_json::from_str(&response.text().map_err(|_|"Drive discovery unavailable")?).map_err(|_|"Invalid discovery response")?;
        files.extend(page.files.into_iter().filter(|file|file.name.ends_with(".json")));
        if files.len()>10000{return Err("Too many snapshots; discovery limit reached".into());}
        match page.next {Some(value)=>next=value,None=>break}
    }
    Ok(files)
}

pub fn download_file(id:&str, destination:&std::path::Path, max_bytes:u64) -> Result<(),String> {
    if id.is_empty() || !id.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_'||c==b'-'){return Err("Invalid Drive file ID".into());}
    let client=reqwest::blocking::Client::builder().timeout(Duration::from_secs(600)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"HTTP setup failed")?;
    let token=access_token(&client)?;
    let mut response=client.get(format!("https://www.googleapis.com/drive/v3/files/{id}?alt=media")).bearer_auth(token).send().map_err(|_|"Drive download unavailable")?;
    if !response.status().is_success(){return Err("Drive download rejected".into());}
    let staged=destination.with_extension("download.partial");
    let result=(||{
        let mut output=std::fs::File::create(&staged).map_err(|_|"Could not create download")?;
        let mut buffer=[0u8;65536];let mut total=0u64;
        loop{let n=response.read(&mut buffer).map_err(|_|"Download interrupted")?;if n==0{break;}total+=n as u64;if total>max_bytes{return Err("Download exceeds configured size limit".into());}output.write_all(&buffer[..n]).map_err(|_|"Download write failed")?;}
        output.sync_all().map_err(|_|"Download flush failed")?;
        if destination.exists(){return Err("Restore destination already exists".into());}
        std::fs::rename(&staged,destination).map_err(|_|"Download publication failed")?;Ok(())
    })();
    if result.is_err(){let _=std::fs::remove_file(staged);}result
}

pub fn validate_merge_selection(original:&str,parents:&[String],branches:&[SnapshotBranch])->Result<(),String>{
    let unique:std::collections::HashSet<_>=parents.iter().collect();
    if parents.len()<2||parents.len()>32||unique.len()!=parents.len(){return Err("Select at least two distinct current revisions".into());}
    if parents.iter().any(|revision|!branches.iter().any(|branch|branch.original_id==original&&branch.revision==*revision&&branch.concurrent)){
        return Err("Conflict changed; refresh Drive backups before resolving".into());
    }
    Ok(())
}

pub fn connect(open: impl FnOnce(String) -> Result<(),String>) -> Result<(),String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| "Could not open local callback")?;
    listener.set_nonblocking(true).map_err(|_| "Callback setup failed")?;
    let redirect = format!("http://127.0.0.1:{}/callback", listener.local_addr().map_err(|_| "Callback address unavailable")?.port());
    let state = random_secret(); let verifier = random_secret();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut auth = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
    auth.query_pairs_mut().extend_pairs([("client_id",CLIENT_ID),("redirect_uri",redirect.as_str()),("response_type","code"),("scope",SCOPE),("state",state.as_str()),("code_challenge",challenge.as_str()),("code_challenge_method","S256"),("access_type","offline"),("prompt","consent")]);
    open(auth.into())?;
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut socket,_)) => {
                socket.set_read_timeout(Some(Duration::from_secs(3))).map_err(|_| "Callback timeout setup failed")?;
                let mut bytes = [0u8;8192]; let size = socket.read(&mut bytes).map_err(|_| "Could not read callback")?;
                let request = String::from_utf8_lossy(&bytes[..size]);
                let target = request.lines().next().and_then(|line| line.strip_prefix("GET ")).and_then(|line| line.strip_suffix(" HTTP/1.1")).unwrap_or("");
                let callback = url::Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| "Invalid callback")?;
                let params: std::collections::HashMap<_,_> = callback.query_pairs().into_owned().collect();
                if callback.path() != "/callback" || params.get("state") != Some(&state) {
                    let _ = socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"); continue;
                }
                let code = params.get("code").ok_or("Google authorization declined")?;
                #[derive(Deserialize)] struct Tokens { refresh_token: Option<String> }
                let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_| "HTTP setup failed")?;
                let mut form = vec![("client_id",CLIENT_ID.to_owned()),("redirect_uri",redirect.clone()),("grant_type","authorization_code".into()),("code",code.clone()),("code_verifier",verifier.clone())];
                if let Some(secret) = client_secret() { form.push(("client_secret",secret)); }
                let response = client.post("https://oauth2.googleapis.com/token").form(&form).send().map_err(|_| "Google token exchange unavailable")?;
                if !response.status().is_success() {
                    let status = response.status(); let body = response.text().unwrap_or_default();
                    let message = token_error(status, &body);
                    let page = "Sign-in did not complete. Return to GigaAM Desktop for the error details.";
                    let _ = write!(socket,"HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",page.len(),page);
                    return Err(message);
                }
                let tokens: Tokens = serde_json::from_str(&response.text().map_err(|_| "Token response unavailable")?).map_err(|_| "Invalid token response")?;
                credential()?.set_password(&tokens.refresh_token.ok_or("Google did not grant offline access")?).map_err(|_| "Could not save credential securely")?;
                let body = "Connected. You may close this tab and return to GigaAM Desktop.";
                let _ = write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return Err("Local callback failed".into()),
        }
    }
    Err("Google sign-in timed out".into())
}

pub fn disconnect() -> Result<(),String> {
    let entry = credential()?;
    if let Ok(token) = entry.get_password() {
        let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(15)).build().map_err(|_| "HTTP setup failed")?;
        let response = client.post("https://oauth2.googleapis.com/revoke").form(&[("token",token)]).send().map_err(|_| "Could not revoke Google access; retry or revoke from Google Account")?;
        if !response.status().is_success() { return Err("Google revocation failed".into()); }
        entry.delete_credential().map_err(|_| "Could not delete credential")?;
    }
    Ok(())
}

#[cfg(test)] mod tests {
    #[test]fn merge_selection_rejects_other_recordings_stale_heads_and_duplicates(){
        let branches=vec![super::SnapshotBranch{file_id:"a".into(),original_id:"record".into(),revision:"left".into(),concurrent:true},super::SnapshotBranch{file_id:"b".into(),original_id:"record".into(),revision:"right".into(),concurrent:true}];
        assert!(super::validate_merge_selection("record",&["left".into(),"right".into()],&branches).is_ok());
        assert!(super::validate_merge_selection("other",&["left".into(),"right".into()],&branches).is_err());
        assert!(super::validate_merge_selection("record",&["left".into(),"left".into()],&branches).is_err());
        assert!(super::validate_merge_selection("record",&["left".into(),"stale".into()],&branches).is_err());
    }
    #[test]fn rejects_self_referencing_and_duplicate_merge_parents(){
        let mut value=serde_json::json!({"version":1,"revision":"merged","merge_parents":["left","right"],"audio":[{"original_filename":"audio.ogg","sha256":"a".repeat(64),"bytes":1,"file_id":"id"}]});
        assert!(super::validate_manifest(&value).is_ok());value["merge_parents"]=serde_json::json!(["merged","left"]);assert!(super::validate_manifest(&value).is_err());value["merge_parents"]=serde_json::json!(["left","left"]);assert!(super::validate_manifest(&value).is_err());
    }
    #[test]fn discovery_returns_both_conflicting_restore_targets_but_not_ancestor(){
        let values=vec![("base-file".into(),serde_json::json!({"revision":"base"})),("left-file".into(),serde_json::json!({"revision":"left","parent_revision":"base"})),("right-file".into(),serde_json::json!({"revision":"right","parent_revision":"base"}))];
        let branches=super::branches_for_group("recording",values);assert_eq!(branches.len(),2);assert!(branches.iter().all(|branch|branch.concurrent&&branch.original_id=="recording"));assert_eq!(branches[0].file_id,"left-file");assert_eq!(branches[1].file_id,"right-file");
    }
    #[test]fn rejects_malformed_restore_before_downloading_audio(){
        let mut value=serde_json::json!({"version":1,"audio":[{"original_filename":"microphone.ogg","sha256":"a".repeat(64),"bytes":12,"file_id":"safe-id"}],"tracks":[{"file":"microphone.ogg","offset_seconds":0.5}]});
        assert!(super::validate_manifest(&value).is_ok());
        value["audio"][0]["original_filename"]="../private".into();assert!(super::validate_manifest(&value).is_err());
        value["audio"][0]["original_filename"]="microphone.ogg".into();value["tracks"][0]["offset_seconds"]=(-1).into();assert!(super::validate_manifest(&value).is_err());
    }
    #[test] fn token_error_does_not_expose_response_secrets() {
        let text = super::token_error(reqwest::StatusCode::UNAUTHORIZED, r#"{"error":"invalid_client","error_description":"secret-value","access_token":"private-token"}"#);
        assert!(text.contains("invalid_client")); assert!(!text.contains("secret-value")); assert!(!text.contains("private-token"));
    }
    #[test] fn pkce_secrets_are_unique_and_valid() { let a=super::random_secret();let b=super::random_secret();assert_ne!(a,b);assert_eq!(a.len(),43);assert!(a.bytes().all(|c| c.is_ascii_alphanumeric() || c==b'-' || c==b'_')); }
}
