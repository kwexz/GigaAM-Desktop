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
    refresh_access(client,"https://oauth2.googleapis.com/token",&refresh,client_secret().as_deref())
}

fn refresh_access(client:&reqwest::blocking::Client,endpoint:&str,refresh:&str,secret:Option<&str>)->Result<String,String>{
    let mut form = vec![("client_id",CLIENT_ID.to_owned()),("grant_type","refresh_token".into()),("refresh_token",refresh.to_owned())];
    if let Some(secret) = secret { form.push(("client_secret",secret.to_owned())); }
    let response = client.post(endpoint).form(&form).send().map_err(|_| "Google token refresh unavailable")?;
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
    upload_snapshot_api(&client,&token,folder,key,name,path,"https://www.googleapis.com")
}

fn upload_snapshot_api(client:&reqwest::blocking::Client,token:&str,folder:&str,key:&str,name:&str,path:&std::path::Path,base:&str)->Result<String,String>{
    let size = std::fs::metadata(path).map_err(|_| "Backup source unavailable")?.len();
    // Reconcile a previous successful PUT whose response was lost before the
    // durable local acknowledgement. Never invent a new snapshot identity.
    if !key.bytes().all(|c|c.is_ascii_alphanumeric()||b":-_".contains(&c)) {return Err("Invalid snapshot identity".into());}
    let mut lookup=url::Url::parse(&format!("{base}/drive/v3/files")).map_err(|_|"Invalid Drive endpoint")?;
    lookup.query_pairs_mut().append_pair("q",&format!("'{folder}' in parents and trashed = false and appProperties has {{ key='gigaamSnapshot' and value='{key}' }}")).append_pair("fields","files(id,size)").append_pair("pageSize","2");
    let previous=client.get(lookup).bearer_auth(&token).send().map_err(|_|"Could not reconcile snapshot retry")?;
    if !previous.status().is_success(){return Err(drive_error(previous.status(),"Snapshot retry discovery failed"));}
    let previous:serde_json::Value=serde_json::from_str(&previous.text().map_err(|_|"Snapshot discovery unavailable")?).map_err(|_|"Invalid snapshot discovery")?;
    if let Some(id)=previous["files"][0]["id"].as_str(){
        // Existing object is not accepted solely by matching metadata. Download
        // it and compare to the immutable local payload before acknowledging.
        let temporary=path.with_extension(format!("verify-{}",crate::new_id()));
        let response=client.get(format!("{base}/drive/v3/files/{id}?alt=media")).bearer_auth(token).send().map_err(|_|"Remote verification unavailable")?;
        if !response.status().is_success(){return Err(drive_error(response.status(),"Remote verification failed"));}
        store_download(response,&temporary,size)?;
        let compare=(||->Result<bool,String>{
            let mut left=std::fs::File::open(path).map_err(|_|"Local payload unavailable")?;let mut right=std::fs::File::open(&temporary).map_err(|_|"Remote payload unavailable")?;
            let mut a=[0u8;65536];let mut b=[0u8;65536];
            loop{let n=left.read(&mut a).map_err(|_|"Local verification failed")?;right.read_exact(&mut b[..n]).map_err(|_|"Remote payload truncated")?;if a[..n]!=b[..n]{return Ok(false);}if n==0{return Ok(right.read(&mut b[..1]).map_err(|_|"Remote verification failed")?==0);}}
        })();let _=std::fs::remove_file(temporary);
        if compare?{return Ok(id.to_owned());}return Err("Snapshot identity conflict; both copies retained".into());
    }
    let metadata = serde_json::json!({"name":name,"parents":[folder],"appProperties":{"gigaamSnapshot":key}});
    let response = initialize_upload(client,&format!("{base}/upload/drive/v3/files?uploadType=resumable&fields=id,size"),token,&metadata,size)?;
    if !response.status().is_success() { return Err(drive_error(response.status(),"Drive rejected upload initialization")); }
    let location = response.headers().get(reqwest::header::LOCATION).and_then(|value| value.to_str().ok()).ok_or("Drive upload URL missing")?;
    let location = if cfg!(test)&&base.starts_with("http://127.0.0.1:"){let url=url::Url::parse(location).map_err(|_|"Invalid test session")?;if url.origin()!=url::Url::parse(base).unwrap().origin(){return Err("Untrusted test session".into());}url}else{validate_upload_destination(location)?};
    let response = put_upload_body(&client,location,&token,path,size)?;
    if !response.status().is_success() { return Err(drive_error(response.status(),"Drive upload failed; local backup retained")); }
    let result: serde_json::Value = serde_json::from_str(&response.text().map_err(|_| "Upload response unavailable")?).map_err(|_| "Invalid upload response")?;
    let id = result["id"].as_str().ok_or("Uploaded file ID missing")?.to_owned();
    // Independent remote readback: count and SHA256 compare without holding audio in RAM.
    let mut local = std::fs::File::open(path).map_err(|_| "Backup source unavailable")?;
    let mut expected = Sha256::new(); let mut buffer = [0u8;65536];
    loop { let n=local.read(&mut buffer).map_err(|_| "Local verification failed")?; if n==0 {break;} expected.update(&buffer[..n]); }
    let mut response = client.get(format!("{base}/drive/v3/files/{id}?alt=media")).bearer_auth(token).send().map_err(|_| "Remote verification unavailable")?;
    if !response.status().is_success() { return Err(drive_error(response.status(),"Remote verification failed")); }
    let mut actual=Sha256::new(); let mut received=0u64;
    loop { let n=response.read(&mut buffer).map_err(|_| "Remote verification interrupted")?; if n==0 {break;} received+=n as u64; if received>size{return Err("Remote file size mismatch".into());} actual.update(&buffer[..n]); }
    if received!=size || actual.finalize()!=expected.finalize() { return Err("Remote integrity mismatch; local data retained".into()); }
    Ok(id)
}

fn put_upload_body(client:&reqwest::blocking::Client,location:url::Url,token:&str,path:&std::path::Path,size:u64)->Result<reqwest::blocking::Response,String>{
    let file=std::fs::File::open(path).map_err(|_|"Backup source unavailable")?;
    client.put(location).bearer_auth(token).header(reqwest::header::CONTENT_TYPE,"application/octet-stream").header(reqwest::header::CONTENT_LENGTH,size).body(file).send().map_err(|_|"Drive upload interrupted; local backup retained".into())
}

fn initialize_upload(client:&reqwest::blocking::Client,endpoint:&str,token:&str,metadata:&serde_json::Value,size:u64)->Result<reqwest::blocking::Response,String>{
    client.post(endpoint).bearer_auth(token).header("Content-Type","application/json").header("X-Upload-Content-Length",size).body(metadata.to_string()).send().map_err(|_|"Drive upload initialization failed".into())
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

fn validate_revision_graph(values:&[(String,serde_json::Value)])->Result<(),String>{
    let mut parents:std::collections::HashMap<&str,Vec<&str>>=Default::default();
    for(_,value)in values{
        let revision=value["revision"].as_str().ok_or("Snapshot revision missing")?;
        let edges:Vec<_>=value["parent_revision"].as_str().into_iter().chain(value["merge_parents"].as_array().into_iter().flatten().filter_map(|v|v.as_str())).collect();
        if let Some(previous)=parents.insert(revision,edges.clone()){if previous!=edges{return Err("Conflicting duplicate snapshot revision".into());}}
    }
    let mut total_visits=0usize;
    for start in parents.keys(){
        let mut stack=vec![(*start,std::collections::HashSet::new())];
        let mut visited=0;
        while let Some((revision,mut path))=stack.pop(){
            visited+=1;total_visits+=1;if visited>100000||total_visits>1000000{return Err("Snapshot ancestry exceeds validation budget".into());}
            if !path.insert(revision){return Err("Cyclic snapshot ancestry".into());}
            if let Some(edges)=parents.get(revision){for parent in edges{stack.push((*parent,path.clone()));}}
        }
    }
    Ok(())
}

fn validate_upload_destination(location:&str)->Result<url::Url,String>{
    let url=url::Url::parse(location).map_err(|_|"Invalid upload URL")?;
    if url.scheme()!="https"||url.host_str()!=Some("www.googleapis.com")||!url.username().is_empty()||url.password().is_some()||url.port_or_known_default()!=Some(443)||!url.path().starts_with("/upload/"){
        return Err("Untrusted Drive upload URL".into());
    }
    Ok(url)
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
            validate_revision_graph(&values)?;
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
    list_snapshot_pages(&client,&token,&folder,"https://www.googleapis.com/drive/v3/files")
}

fn list_snapshot_pages(client:&reqwest::blocking::Client,token:&str,folder:&str,base:&str)->Result<Vec<RemoteFile>,String>{
    let mut files = Vec::new(); let mut next = String::new();
    let mut seen=std::collections::HashSet::new();
    loop {
        let mut endpoint=url::Url::parse(base).map_err(|_|"Invalid discovery endpoint")?;
        endpoint.query_pairs_mut().append_pair("q",&format!("'{folder}' in parents and trashed = false and mimeType != 'application/vnd.google-apps.folder'"))
            .append_pair("fields","nextPageToken,files(id,name)").append_pair("pageSize","100");
        if !next.is_empty(){endpoint.query_pairs_mut().append_pair("pageToken",&next);}
        let response=client.get(endpoint).bearer_auth(&token).send().map_err(|_|"Drive discovery unavailable")?;
        if !response.status().is_success(){return Err(drive_error(response.status(),"Drive discovery rejected"));}
        #[derive(Deserialize)] struct Page { files:Vec<RemoteFile>, #[serde(rename="nextPageToken")] next:Option<String> }
        let page:Page=serde_json::from_str(&response.text().map_err(|_|"Drive discovery unavailable")?).map_err(|_|"Invalid discovery response")?;
        files.extend(page.files.into_iter().filter(|file|file.name.ends_with(".json")));
        if seen.len()>1000{return Err("Drive discovery exceeds page budget".into());}
        if files.len()>10000{return Err("Too many snapshots; discovery limit reached".into());}
        match page.next {Some(value)=>{if !seen.insert(value.clone()){return Err("Repeated Drive page cursor".into());}next=value;},None=>break}
    }
    Ok(files)
}

pub fn download_file(id:&str, destination:&std::path::Path, max_bytes:u64) -> Result<(),String> {
    if id.is_empty() || !id.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_'||c==b'-'){return Err("Invalid Drive file ID".into());}
    let client=reqwest::blocking::Client::builder().timeout(Duration::from_secs(600)).redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"HTTP setup failed")?;
    let token=access_token(&client)?;
    let response=client.get(format!("https://www.googleapis.com/drive/v3/files/{id}?alt=media")).bearer_auth(token).send().map_err(|_|"Drive download unavailable")?;
    if !response.status().is_success(){return Err(drive_error(response.status(),"Drive download rejected"));}
    store_download(response,destination,max_bytes)
}

fn store_download(mut response:impl Read,destination:&std::path::Path,max_bytes:u64)->Result<(),String>{
    let staged=destination.with_extension(format!("{}.download.partial",crate::new_id()));
    let result=(||{
        let mut output=std::fs::File::create(&staged).map_err(|_|"Could not create download")?;
        let mut buffer=[0u8;65536];let mut total=0u64;
        loop{let n=response.read(&mut buffer).map_err(|_|"Download interrupted")?;if n==0{break;}total+=n as u64;if total>max_bytes{return Err("Download exceeds configured size limit".into());}output.write_all(&buffer[..n]).map_err(|_|"Download write failed")?;}
        output.sync_all().map_err(|_|"Download flush failed")?;
        if destination.exists(){return Err("Restore destination already exists".into());}
        // Atomic no-clobber publication even if another restore creates the
        // destination after exists() above. Rename replaces files on Unix.
        std::fs::hard_link(&staged,destination).map_err(|_|"Download publication failed or destination exists")?;
        std::fs::remove_file(&staged).map_err(|_|"Download staging cleanup failed")?;Ok(())
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
    #[test]fn complete_upload_api_reconciles_retry_without_second_creation(){
        use std::io::{Read,Write};let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();let base=format!("http://{address}");let server_base=base.clone();
        let server=std::thread::spawn(move||{for step in 0..6{
            let(mut socket,_)=listener.accept().unwrap();socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();let mut headers=Vec::new();let mut byte=[0u8;1];while !headers.ends_with(b"\r\n\r\n"){socket.read_exact(&mut byte).unwrap();headers.push(byte[0]);}let headers=String::from_utf8(headers).unwrap().to_lowercase();let length:usize=headers.lines().find_map(|line|line.strip_prefix("content-length: ")).unwrap_or("0").trim().parse().unwrap();let mut body=vec![0;length];socket.read_exact(&mut body).unwrap();
            let(body,extra)=match step{
                0=>{assert!(headers.starts_with("get /drive/v3/files?"));("{\"files\":[]}".to_owned(),String::new())},
                1=>{assert!(headers.starts_with("post /upload/"));let metadata:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(metadata["appProperties"]["gigaamSnapshot"],"record:rev:0");(String::new(),format!("Location: {server_base}/upload/session\r\n"))},
                2=>{assert!(headers.starts_with("put /upload/session"));assert_eq!(body,b"audio");("{\"id\":\"remote-id\"}".into(),String::new())},
                3|5=>{assert!(headers.starts_with("get /drive/v3/files/remote-id?alt=media"));("audio".into(),String::new())},
                _=>{assert!(headers.starts_with("get /drive/v3/files?"));("{\"files\":[{\"id\":\"remote-id\",\"size\":\"5\"}]}".into(),String::new())}
            };write!(socket,"HTTP/1.1 200 OK\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }});
        let path=std::env::temp_dir().join(format!("gigaam-api-{}",crate::new_id()));std::fs::write(&path,b"audio").unwrap();let client=reqwest::blocking::Client::new();for _ in 0..2{assert_eq!(super::upload_snapshot_api(&client,"test-token","folder","record:rev:0","audio.ogg",&path,&base).unwrap(),"remote-id");}server.join().unwrap();std::fs::remove_file(path).unwrap();
    }
    #[test]fn http_upload_retry_survives_queue_reopen_and_requires_readback(){
        use std::io::{Read,Write};
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let server=std::thread::spawn(move||{
            for step in 0..3{
                let(mut socket,_)=listener.accept().unwrap();socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut request=Vec::new();let mut byte=[0u8;1];while !request.ends_with(b"\r\n\r\n"){socket.read_exact(&mut byte).unwrap();request.push(byte[0]);}
                let headers=String::from_utf8(request).unwrap().to_lowercase();
                if step<2{
                    assert!(headers.starts_with("put /session"));let length:usize=headers.lines().find_map(|line|line.strip_prefix("content-length: ")).unwrap().trim().parse().unwrap();let mut body=vec![0;length];socket.read_exact(&mut body).unwrap();assert_eq!(body,b"immutable audio");
                    let status=if step==0{"503 Service Unavailable"}else{"200 OK"};write!(socket,"HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                }else{
                    assert!(headers.starts_with("get /media"));socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\nimmutable audio").unwrap();
                }
            }
        });
        let root=std::env::temp_dir().join(format!("gigaam-http-retry-{}",crate::new_id()));std::fs::create_dir_all(&root).unwrap();let source=root.join("source.ogg");std::fs::write(&source,b"immutable audio").unwrap();let queue_path=root.join("queue.sqlite");
        let client=reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(5)).build().unwrap();
        let upload_url=url::Url::parse(&format!("http://{address}/session")).unwrap();
        {let mut queue=crate::drive_outbox::Outbox::open(&queue_path).unwrap();queue.enqueue("revision","immutable").unwrap();queue.run_one(0,|_|{let response=super::put_upload_body(&client,upload_url.clone(),"test-token",&source,15).unwrap();assert_eq!(response.status(),reqwest::StatusCode::SERVICE_UNAVAILABLE);Err(false)}).unwrap();assert_eq!(queue.status().unwrap().synced,0);}
        let mut queue=crate::drive_outbox::Outbox::open(&queue_path).unwrap();
        assert!(!queue.run_one(1,|_|panic!("backoff bypassed")).unwrap());
        queue.run_one(2000,|job|{
            assert_eq!(job.attempts,1);assert_eq!(job.id,"revision");let response=super::put_upload_body(&client,upload_url.clone(),"test-token",&source,15).unwrap();assert!(response.status().is_success());
            let response=client.get(format!("http://{address}/media")).send().unwrap();let restored=root.join("readback.ogg");super::store_download(response,&restored,15).unwrap();assert_eq!(std::fs::read(restored).unwrap(),std::fs::read(&source).unwrap());Ok(())
        }).unwrap();assert_eq!(queue.status().unwrap().synced,1);server.join().unwrap();drop(queue);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]fn cyclic_remote_revision_graph_is_rejected_not_hidden(){
        let cycle=vec![("one".into(),serde_json::json!({"revision":"a","parent_revision":"b"})),("two".into(),serde_json::json!({"revision":"b","parent_revision":"a"}))];assert!(super::validate_revision_graph(&cycle).is_err());
        let valid=vec![("one".into(),serde_json::json!({"revision":"a"})),("two".into(),serde_json::json!({"revision":"b","parent_revision":"a"}))];assert!(super::validate_revision_graph(&valid).is_ok());
    }
    #[test]fn upload_initialization_contract_sends_snapshot_identity_and_size(){
        use std::io::{Read,Write};let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let server=std::thread::spawn(move||{let(mut socket,_)=listener.accept().unwrap();socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();let mut headers=Vec::new();let mut byte=[0u8;1];while !headers.ends_with(b"\r\n\r\n"){socket.read_exact(&mut byte).unwrap();headers.push(byte[0]);}let headers=String::from_utf8(headers).unwrap().to_lowercase();assert!(headers.contains("x-upload-content-length: 5"));assert!(headers.contains("content-type: application/json"));let size:usize=headers.lines().find_map(|s|s.strip_prefix("content-length: ")).unwrap().trim().parse().unwrap();let mut body=vec![0;size];socket.read_exact(&mut body).unwrap();let value:serde_json::Value=serde_json::from_slice(&body).unwrap();assert_eq!(value["appProperties"]["gigaamSnapshot"],"record:revision:0");assert_eq!(value["parents"][0],"folder");socket.write_all(b"HTTP/1.1 200 OK\r\nLocation: https://www.googleapis.com/upload/drive/v3/files?upload_id=test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();});
        let response=super::initialize_upload(&reqwest::blocking::Client::new(),&format!("http://{address}/upload"),"test-token",&serde_json::json!({"name":"audio.ogg","parents":["folder"],"appProperties":{"gigaamSnapshot":"record:revision:0"}}),5).unwrap();assert!(response.status().is_success());assert!(super::validate_upload_destination(response.headers()[reqwest::header::LOCATION].to_str().unwrap()).is_ok());server.join().unwrap();
    }
    #[test]fn token_refresh_http_contract_uses_form_and_redacts_invalid_grant(){
        use std::io::{Read,Write};let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let server=std::thread::spawn(move||{for attempt in 0..2{let(mut socket,_)=listener.accept().unwrap();socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();let mut request=Vec::new();let mut byte=[0u8;1];while !request.ends_with(b"\r\n\r\n"){socket.read_exact(&mut byte).unwrap();request.push(byte[0]);}let headers=String::from_utf8(request).unwrap().to_lowercase();let length:usize=headers.lines().find_map(|line|line.strip_prefix("content-length: ")).unwrap().trim().parse().unwrap();let mut body=vec![0;length];socket.read_exact(&mut body).unwrap();let form:std::collections::HashMap<_,_>=url::form_urlencoded::parse(&body).into_owned().collect();assert_eq!(form["grant_type"],"refresh_token");assert_eq!(form["refresh_token"],"test-refresh");assert_eq!(form["client_secret"],"test-secret");let(status,body)=if attempt==0{("200 OK",r#"{"access_token":"test-access"}"#)}else{("400 Bad Request",r#"{"error":"invalid_grant","error_description":"test-secret"}"#)};write!(socket,"HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();}});
        let client=reqwest::blocking::Client::new();let endpoint=format!("http://{address}/token");assert_eq!(super::refresh_access(&client,&endpoint,"test-refresh",Some("test-secret")).unwrap(),"test-access");let error=super::refresh_access(&client,&endpoint,"test-refresh",Some("test-secret")).unwrap_err();assert!(error.contains("invalid_grant"));assert!(!error.contains("test-secret"));server.join().unwrap();
    }
    #[test]fn discovery_http_contract_follows_next_page_and_filters_audio(){
        use std::io::{Read,Write};let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let server=std::thread::spawn(move||{for index in 0..2{let(mut socket,_)=listener.accept().unwrap();let mut request=Vec::new();let mut byte=[0u8;1];while !request.ends_with(b"\r\n\r\n"){socket.read_exact(&mut byte).unwrap();request.push(byte[0]);}let request=String::from_utf8(request).unwrap();assert!(request.contains("pageSize=100"));if index==1{assert!(request.contains("pageToken=next-page"));}let body=if index==0{r#"{"files":[{"id":"one","name":"one.json"},{"id":"audio","name":"audio.ogg"}],"nextPageToken":"next-page"}"#}else{r#"{"files":[{"id":"two","name":"two.json"}]}"#};write!(socket,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();}});
        let files=super::list_snapshot_pages(&reqwest::blocking::Client::new(),"test-token","folder",&format!("http://{address}/files")).unwrap();server.join().unwrap();assert_eq!(files.iter().map(|file|file.id.as_str()).collect::<Vec<_>>(),vec!["one","two"]);
    }
    #[test]fn resumable_body_contract_sends_exact_length_and_bytes_over_http(){
        use std::io::{Read,Write};
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let server=std::thread::spawn(move||{let(mut socket,_)=listener.accept().unwrap();socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();let mut data=Vec::new();let mut byte=[0u8;1];while !data.ends_with(b"\r\n\r\n"){socket.read_exact(&mut byte).unwrap();data.push(byte[0]);}let headers=String::from_utf8(data).unwrap().to_lowercase();assert!(headers.starts_with("put /upload-session"));assert!(headers.contains("authorization: bearer test-only-token"));assert!(headers.contains("content-length: 5"));let mut body=[0u8;5];socket.read_exact(&mut body).unwrap();assert_eq!(&body,b"audio");socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"id\":\"x\"}").unwrap();});
        let path=std::env::temp_dir().join(format!("gigaam-put-{}",crate::new_id()));std::fs::write(&path,b"audio").unwrap();let client=reqwest::blocking::Client::new();let response=super::put_upload_body(&client,url::Url::parse(&format!("http://{address}/upload-session")).unwrap(),"test-only-token",&path,5).unwrap();assert!(response.status().is_success());server.join().unwrap();std::fs::remove_file(path).unwrap();
    }
    #[test]fn upload_session_never_forwards_bearer_to_untrusted_destination(){
        assert!(super::validate_upload_destination("https://www.googleapis.com/upload/drive/v3/files?upload_id=test").is_ok());
        for url in ["http://www.googleapis.com/upload/drive/v3/files","https://evil.example/upload/drive/v3/files","https://www.googleapis.com:8443/upload/drive/v3/files","https://user@www.googleapis.com/upload/drive/v3/files","https://www.googleapis.com/not-upload"]{assert!(super::validate_upload_destination(url).is_err(),"{url}");}
    }
    #[test]fn all_drive_operations_pause_on_unauthorized_not_on_rate_limit(){
        for operation in ["Snapshot retry discovery failed","Remote verification failed","Drive discovery rejected","Drive download rejected"]{
            let unauthorized=super::drive_error(reqwest::StatusCode::UNAUTHORIZED,operation);assert!(unauthorized.contains("revoked"));assert!(!unauthorized.contains("token="));
            let limited=super::drive_error(reqwest::StatusCode::TOO_MANY_REQUESTS,operation);assert!(limited.contains("429"));assert!(!limited.contains("revoked"));
        }
    }
    #[test]fn download_contract_streams_http_body_and_keeps_existing_destination(){
        use std::io::Write;
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let server=std::thread::spawn(move||{let(mut stream,_)=listener.accept().unwrap();let mut request=[0u8;1024];let _=std::io::Read::read(&mut stream,&mut request);stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\naudio").unwrap();});
        let root=std::env::temp_dir().join(format!("gigaam-http-download-{}",crate::new_id()));std::fs::create_dir_all(&root).unwrap();let path=root.join("audio.ogg");
        let response=reqwest::blocking::get(format!("http://{address}/media")).unwrap();super::store_download(response,&path,5).unwrap();server.join().unwrap();assert_eq!(std::fs::read(&path).unwrap(),b"audio");
        assert!(super::store_download(std::io::Cursor::new(b"other"),&path,5).is_err());assert_eq!(std::fs::read(&path).unwrap(),b"audio");
        assert!(super::store_download(std::io::Cursor::new(b"too large"),&root.join("oversized"),2).is_err());assert!(!root.join("oversized").exists());assert_eq!(std::fs::read_dir(&root).unwrap().count(),1);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]fn concurrent_downloads_publish_exactly_one_complete_payload(){
        let root=std::env::temp_dir().join(format!("gigaam-download-race-{}",crate::new_id()));std::fs::create_dir_all(&root).unwrap();
        let path=root.join("audio.ogg");let barrier=std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles:Vec<_>=[b"left".to_vec(),b"right".to_vec()].into_iter().map(|bytes|{let path=path.clone();let barrier=barrier.clone();std::thread::spawn(move||{barrier.wait();super::store_download(std::io::Cursor::new(bytes),&path,16)})}).collect();
        let results:Vec<_>=handles.into_iter().map(|handle|handle.join().unwrap()).collect();assert_eq!(results.iter().filter(|result|result.is_ok()).count(),1);
        let actual=std::fs::read(&path).unwrap();assert!(actual==b"left"||actual==b"right");assert_eq!(std::fs::read_dir(&root).unwrap().count(),1);std::fs::remove_dir_all(root).unwrap();
    }
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
