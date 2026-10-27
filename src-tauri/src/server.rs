//! Isolated Rust HTTP adapter for the remote/browser contracts.
//!
//! The server is deliberately a library router: deployment owns the listener,
//! bind address, TLS proxy and process limits. No public socket is opened by
//! the desktop binary and no ASR work runs on the async reactor.

use std::{path::PathBuf, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{
        Path as AxumPath, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Serialize;
use tokio::{
    fs,
    fs::OpenOptions,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Semaphore,
};

pub const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct ServerConfig {
    pub bearer_token: Arc<str>,
    pub data_dir: PathBuf,
    pub max_upload_bytes: u64,
    pub max_concurrent_jobs: usize,
}

#[derive(Clone)]
pub struct ServerState {
    pub config: ServerConfig,
    pub jobs: Arc<Semaphore>,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
}

#[derive(Debug, Serialize)]
struct HealthBody {
    status: &'static str,
    model_enabled: bool,
}

#[derive(Debug, Serialize)]
struct UploadBody {
    session_id: String,
    bytes: u64,
    durable: bool,
}

#[derive(Debug, Serialize)]
struct SessionBody {
    session_id: String,
    recording: &'static str,
    upload: &'static str,
    recognition: &'static str,
    sync: &'static str,
}

#[derive(Debug, Serialize)]
struct ModelBody {
    enabled: bool,
    manifest: Option<&'static str>,
}

pub fn router(config: ServerConfig) -> Router {
    let state = ServerState {
        jobs: Arc::new(Semaphore::new(config.max_concurrent_jobs.max(1))),
        config,
    };
    Router::new()
        .route("/health", get(health))
        .route("/v1/model", get(model_status))
        .route("/v1/sessions/{session_id}/live", get(live_socket))
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{session_id}", get(session_status))
        .route(
            "/v1/sessions/{session_id}/recognition",
            put(start_recognition),
        )
        .route(
            "/v1/sessions/{session_id}/audio",
            post(upload_audio).get(download_audio),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

async fn auth(State(state): State<ServerState>, request: Request<Body>, next: Next) -> Response {
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token.as_bytes() == state.config.bearer_token.as_bytes());
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(ErrorBody {
                error: "unauthorized",
            }),
        )
            .into_response();
    }
    next.run(request).await
}

async fn health(State(_state): State<ServerState>) -> impl IntoResponse {
    axum::Json(HealthBody {
        status: "ok",
        model_enabled: false,
    })
}

async fn model_status(State(_state): State<ServerState>) -> impl IntoResponse {
    axum::Json(ModelBody {
        enabled: false,
        manifest: None,
    })
}

async fn live_socket(
    State(state): State<ServerState>,
    AxumPath(session_id): AxumPath<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !valid_session_id(&session_id) {
        return bad_request("invalid session id");
    }
    upgrade
        .on_upgrade(move |socket| handle_live_socket(socket, state, session_id))
        .into_response()
}

async fn handle_live_socket(mut socket: WebSocket, state: ServerState, session_id: String) {
    let directory = state.config.data_dir.join("sessions").join(&session_id);
    if fs::create_dir_all(&directory).await.is_err() {
        return;
    }
    let spool_path = directory.join("live.spool.partial");
    let mut spool = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&spool_path)
        .await
    {
        Ok(file) => file,
        Err(_) => return,
    };
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Text(text) => {
                let ack = serde_json::json!({"type":"ack","session_id":session_id,"durable":false,"message":text.to_string()});
                if socket
                    .send(Message::Text(ack.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Message::Binary(bytes) => {
                if bytes.len() > MAX_REQUEST_BYTES {
                    break;
                }
                if spool
                    .write_all(&(bytes.len() as u32).to_le_bytes())
                    .await
                    .is_err()
                    || spool.write_all(&bytes).await.is_err()
                    || spool.sync_data().await.is_err()
                {
                    break;
                }
                let ack = serde_json::json!({"type":"ack","session_id":session_id,"durable":true,"bytes":bytes.len()});
                if socket
                    .send(Message::Text(ack.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Message::Ping(payload) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Pong(_) => {}
        }
    }
}

async fn list_sessions(State(state): State<ServerState>) -> Response {
    let directory = state.config.data_dir.join("sessions");
    let mut entries = match fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(_) => return axum::Json(Vec::<SessionBody>::new()).into_response(),
    };
    let mut sessions = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(file_type) = entry.file_type().await
            && file_type.is_dir()
            && entry.file_name().to_str().is_some_and(valid_session_id)
        {
            sessions.push(SessionBody {
                session_id: entry.file_name().to_string_lossy().into_owned(),
                recording: "idle",
                upload: "durable",
                recognition: "not_started",
                sync: "disabled",
            });
        }
    }
    axum::Json(sessions).into_response()
}

async fn session_status(
    State(state): State<ServerState>,
    AxumPath(session_id): AxumPath<String>,
) -> Response {
    if !valid_session_id(&session_id) {
        return bad_request("invalid session id");
    }
    let path = state
        .config
        .data_dir
        .join("sessions")
        .join(&session_id)
        .join("audio.bin");
    if fs::metadata(path).await.is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    axum::Json(SessionBody {
        session_id,
        recording: "idle",
        upload: "durable",
        recognition: "not_started",
        sync: "disabled",
    })
    .into_response()
}

async fn start_recognition(
    State(_state): State<ServerState>,
    AxumPath(session_id): AxumPath<String>,
) -> Response {
    if !valid_session_id(&session_id) {
        return bad_request("invalid session id");
    }
    (
        StatusCode::ACCEPTED,
        axum::Json(SessionBody {
            session_id,
            recording: "idle",
            upload: "durable",
            recognition: "queued",
            sync: "disabled",
        }),
    )
        .into_response()
}

async fn upload_audio(
    State(state): State<ServerState>,
    AxumPath(session_id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !valid_session_id(&session_id) {
        return bad_request("invalid session id");
    }
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if declared.is_some_and(|bytes| bytes > state.config.max_upload_bytes) {
        return too_large();
    }
    let permit = match state.jobs.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                axum::Json(ErrorBody {
                    error: "job capacity exceeded",
                }),
            )
                .into_response();
        }
    };
    let directory = state.config.data_dir.join("sessions").join(&session_id);
    if let Err(error) = fs::create_dir_all(&directory).await {
        return internal(error);
    }
    let path = directory.join("upload.partial");
    let mut stream = body.into_data_stream();
    let mut file = match fs::File::create(&path).await {
        Ok(file) => file,
        Err(error) => return internal(error),
    };
    let mut total = 0u64;
    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                let _ = fs::remove_file(&path).await;
                return bad_request("invalid upload stream");
            }
        };
        total = total.saturating_add(chunk.len() as u64);
        if total > state.config.max_upload_bytes {
            drop(permit);
            let _ = fs::remove_file(&path).await;
            return too_large();
        }
        if let Err(error) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            let _ = fs::remove_file(&path).await;
            return internal(error);
        }
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&path).await;
        return internal(error);
    }
    let final_path = directory.join("audio.bin");
    if let Err(error) = fs::rename(&path, &final_path).await {
        let _ = fs::remove_file(&path).await;
        return internal(error);
    }
    drop(permit);
    (
        StatusCode::CREATED,
        axum::Json(UploadBody {
            session_id,
            bytes: total,
            durable: true,
        }),
    )
        .into_response()
}

async fn download_audio(
    State(state): State<ServerState>,
    AxumPath(session_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if !valid_session_id(&session_id) {
        return bad_request("invalid session id");
    }
    let path = state
        .config
        .data_dir
        .join("sessions")
        .join(&session_id)
        .join("audio.bin");
    let mut file = match fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let size = match file.metadata().await {
        Ok(meta) => meta.len(),
        Err(error) => return internal(error),
    };
    let (start, end, partial) = match parse_range(&headers, size) {
        Ok(value) => value,
        Err(status) => return status.into_response(),
    };
    if let Err(error) = file.seek(std::io::SeekFrom::Start(start)).await {
        return internal(error);
    }
    let mut bytes = vec![0u8; (end - start + 1) as usize];
    if let Err(error) = file.read_exact(&mut bytes).await {
        return internal(error);
    }
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&(end - start + 1).to_string()).unwrap(),
    );
    if partial {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")).unwrap(),
        );
    }
    response
}

fn parse_range(headers: &HeaderMap, size: u64) -> Result<(u64, u64, bool), StatusCode> {
    if size == 0 {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    let Some(value) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok((0, size - 1, false));
    };
    let Some(value) = value.strip_prefix("bytes=") else {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    };
    let (start, end) = value
        .split_once('-')
        .ok_or(StatusCode::RANGE_NOT_SATISFIABLE)?;
    let start = start
        .parse::<u64>()
        .map_err(|_| StatusCode::RANGE_NOT_SATISFIABLE)?;
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>()
            .map_err(|_| StatusCode::RANGE_NOT_SATISFIABLE)?
            .min(size - 1)
    };
    if start >= size || start > end {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    Ok((start, end, true))
}

fn valid_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
fn bad_request(error: &'static str) -> Response {
    (StatusCode::BAD_REQUEST, axum::Json(ErrorBody { error })).into_response()
}
fn too_large() -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        axum::Json(ErrorBody {
            error: "upload too large",
        }),
    )
        .into_response()
}
fn internal<E: std::fmt::Display>(error: E) -> Response {
    tracing_free_error(error);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(ErrorBody {
            error: "internal server error",
        }),
    )
        .into_response()
}
fn tracing_free_error<E: std::fmt::Display>(_error: E) {}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use std::path::Path;
    use tower::ServiceExt;

    fn config(directory: &Path) -> ServerConfig {
        ServerConfig {
            bearer_token: Arc::from("secret"),
            data_dir: directory.to_path_buf(),
            max_upload_bytes: 16,
            max_concurrent_jobs: 1,
        }
    }

    #[tokio::test]
    async fn auth_upload_range_and_traversal_are_enforced() {
        let directory = std::env::temp_dir().join(format!("gigaam-server-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory).await;
        let app = router(config(&directory));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/sessions/x/audio")
                    .method("POST")
                    .body(Body::from("abc"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/sessions/../audio")
                    .method("POST")
                    .header("authorization", "Bearer secret")
                    .body(Body::from("abc"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
        ));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/sessions/s1/audio")
                    .method("POST")
                    .header("authorization", "Bearer secret")
                    .body(Body::from("12345678901234567890"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/sessions/s1/audio")
                    .method("POST")
                    .header("authorization", "Bearer secret")
                    .body(Body::from("0123456789"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/sessions/s1/audio")
                    .header("authorization", "Bearer secret")
                    .header("range", "bytes=2-5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 16)
                .await
                .unwrap()
                .as_ref(),
            b"2345"
        );
        let _ = fs::remove_dir_all(directory).await;
    }
}
