//! Desktop public-client OAuth. Credentials never cross the IPC boundary.
use std::{io::{Read, Write}, net::TcpListener, time::{Duration, Instant}};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};
use serde::Deserialize;

pub const CLIENT_ID: &str = "591054714419-laosoks26n377qioh8pfr0dirhamocsq.apps.googleusercontent.com";
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
fn random_secret() -> String { let mut bytes = [0u8;32]; rand::rng().fill_bytes(&mut bytes); URL_SAFE_NO_PAD.encode(bytes) }
fn credential() -> Result<keyring::Entry,String> { keyring::Entry::new("GigaAM Desktop", "google-drive-refresh-token").map_err(|_| "Credential store unavailable".into()) }

pub fn connected() -> bool { credential().and_then(|entry| entry.get_password().map_err(|_| "No credential".into())).is_ok() }

fn access_token(client: &reqwest::blocking::Client) -> Result<String,String> {
    let refresh = credential()?.get_password().map_err(|_| "Connect Google Drive first")?;
    let response = client.post("https://oauth2.googleapis.com/token").form(&[("client_id",CLIENT_ID),("grant_type","refresh_token"),("refresh_token",refresh.as_str())]).send().map_err(|_| "Google token refresh unavailable")?;
    if !response.status().is_success() { return Err("Google access expired or was revoked. Reconnect Drive.".into()); }
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
    if !response.status().is_success() { return Err("Could not discover application folder".into()); }
    let result: serde_json::Value = serde_json::from_str(&response.text().map_err(|_| "Drive response unavailable")?).map_err(|_| "Invalid Drive response")?;
    if let Some(id) = result["files"][0]["id"].as_str() { return Ok(id.to_owned()); }
    let body = serde_json::json!({"name":"GigaAM Desktop","mimeType":"application/vnd.google-apps.folder","appProperties":{"gigaam":"root-v1"}});
    let response = client.post("https://www.googleapis.com/drive/v3/files").bearer_auth(&token).header("Content-Type","application/json").body(body.to_string()).send().map_err(|_| "Could not create Drive folder")?;
    if !response.status().is_success() { return Err("Google rejected application folder creation".into()); }
    let result: serde_json::Value = serde_json::from_str(&response.text().map_err(|_| "Drive response unavailable")?).map_err(|_| "Invalid Drive response")?;
    result["id"].as_str().map(str::to_owned).ok_or("Drive folder ID missing".into())
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
                let response = client.post("https://oauth2.googleapis.com/token").form(&[("client_id",CLIENT_ID),("redirect_uri",redirect.as_str()),("grant_type","authorization_code"),("code",code.as_str()),("code_verifier",verifier.as_str())]).send().map_err(|_| "Google token exchange unavailable")?;
                if !response.status().is_success() { return Err("Google rejected token exchange; review Desktop OAuth client configuration".into()); }
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
    #[test] fn pkce_secrets_are_unique_and_valid() { let a=super::random_secret();let b=super::random_secret();assert_ne!(a,b);assert_eq!(a.len(),43);assert!(a.bytes().all(|c| c.is_ascii_alphanumeric() || c==b'-' || c==b'_')); }
}
