use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use gigaam_desktop_lib::server::{ServerConfig, router};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = std::env::var_os("GIGAAM_SERVER_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("server-data"));
    let token = std::env::var("GIGAAM_SERVER_TOKEN").map_err(|_| {
        anyhow::anyhow!("GIGAAM_SERVER_TOKEN is required; refusing anonymous server")
    })?;
    let bind: SocketAddr = std::env::var("GIGAAM_SERVER_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8787".into())
        .parse()?;
    if !bind.ip().is_loopback() && std::env::var_os("GIGAAM_SERVER_ALLOW_NON_LOOPBACK").is_none() {
        anyhow::bail!("refusing non-loopback bind without explicit opt-in");
    }
    let app = router(ServerConfig {
        bearer_token: Arc::from(token),
        data_dir,
        max_upload_bytes: 512 * 1024 * 1024,
        max_concurrent_jobs: 1,
    });
    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
