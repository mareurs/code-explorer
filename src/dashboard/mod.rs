#[cfg(feature = "dashboard")]
mod api;
#[cfg(feature = "dashboard")]
mod routes;

use anyhow::Result;
use std::path::PathBuf;

/// Launch the dashboard HTTP server.
///
/// Reads project data from `.codescout/` and serves a web UI.
/// Does NOT start the MCP server, LSP, or tool machinery.
#[cfg(feature = "dashboard")]
pub async fn serve(
    project_root: PathBuf,
    host: String,
    port: u16,
    open_browser: bool,
) -> Result<()> {
    let addr: std::net::SocketAddr = format!("{}:{}", host, port).parse()?;
    tracing::info!("Dashboard server starting at http://{}", addr);

    let router = routes::build_router(&project_root)?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;
    eprintln!("Dashboard: http://{}", actual_addr);

    if open_browser {
        let url = format!("http://{}", actual_addr);
        if let Err(e) = open::that(&url) {
            tracing::warn!("Failed to open browser: {}", e);
        }
    }

    axum::serve(listener, router)
        .with_graceful_shutdown(crate::server::shutdown_signal())
        .await?;

    Ok(())
}
