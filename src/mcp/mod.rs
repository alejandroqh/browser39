mod commands;
mod params;
mod server;

use anyhow::Result;
use rmcp::ServiceExt;

use crate::core::config::Config;
use crate::core::session_store::{self, SessionStore};
use commands::spawn_browser_service;
use server::McpServer;

pub async fn run_mcp_stdio(config: Config, store: Box<dyn SessionStore>) -> Result<()> {
    let cmd_tx = spawn_browser_service(config, store);
    let server = McpServer::new(cmd_tx);
    let transport = rmcp::transport::io::stdio();
    let handle = server.serve(transport).await.map_err(anyhow::Error::msg)?;
    handle.waiting().await?;
    Ok(())
}

pub async fn run_mcp_sse(config: Config, port: u16) -> Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
        session::local::LocalSessionManager,
    };
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    let ct = CancellationToken::new();
    let config = Arc::new(config);

    let config_clone = config.clone();
    let service = StreamableHttpService::new(
        move || {
            let cfg = (*config_clone).clone();
            // SSE sessions are per-connection, always in-memory
            let store: Box<dyn SessionStore> = Box::new(session_store::InMemoryStore);
            let cmd_tx = spawn_browser_service(cfg, store);
            Ok(McpServer::new(cmd_tx))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_cancellation_token(ct.clone()),
    );

    let addr = format!("0.0.0.0:{port}");
    eprintln!("browser39 MCP server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    loop {
        let (stream, _addr) = listener.accept().await?;
        let svc = service.clone();
        tokio::spawn(async move {
            let io = hyper_util::rt::TokioIo::new(stream);
            if let Err(e) = hyper_util::server::conn::auto::Builder::new(
                hyper_util::rt::TokioExecutor::new(),
            )
            .serve_connection(io, hyper::service::service_fn(move |req| {
                let mut svc = svc.clone();
                async move {
                    use tower_service::Service;
                    svc.call(req).await
                }
            }))
            .await
            {
                eprintln!("connection error: {e}");
            }
        });
    }
}
