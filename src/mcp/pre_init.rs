//! Keeps the MCP server alive through traffic that arrives before `initialize`.
//!
//! rmcp's handshake only accepts `initialize` (or `ping`) as the first request and
//! fails on anything else, which ends the stdio process. Clients on newer protocol
//! revisions probe with `server/discover` first and expect `-32601 Method not found`
//! so they can fall back to `initialize`. This wrapper answers such requests itself
//! and only hands rmcp the messages its handshake accepts.

use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, ErrorCode, ErrorData, ServerJsonRpcMessage,
};
use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;

pub struct PreInitGuard<T> {
    inner: T,
    initialized: bool,
}

impl<T> PreInitGuard<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            initialized: false,
        }
    }
}

/// Error reply for a request received before `initialize`.
fn pre_init_error(request: &ClientRequest) -> ErrorData {
    match request {
        ClientRequest::CustomRequest(_) => {
            ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "Method not found", None)
        }
        _ => ErrorData::invalid_request("Server not initialized", None),
    }
}

impl<T: Transport<RoleServer>> Transport<RoleServer> for PreInitGuard<T> {
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        if self.initialized {
            return self.inner.receive().await;
        }
        loop {
            let msg = self.inner.receive().await?;
            // Notifications and stray responses need no reply, and rmcp rejects them pre-init.
            let ClientJsonRpcMessage::Request(req) = &msg else {
                continue;
            };
            match &req.request {
                ClientRequest::InitializeRequest(_) => {
                    self.initialized = true;
                    return Some(msg);
                }
                ClientRequest::PingRequest(_) => return Some(msg),
                other => {
                    let reply =
                        ServerJsonRpcMessage::error(pre_init_error(other), Some(req.id.clone()));
                    self.inner.send(reply).await.ok()?;
                }
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::server::McpServer;
    use rmcp::ServiceExt;
    use rmcp::transport::async_rw::AsyncRwTransport;
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};
    use tokio::sync::mpsc;

    struct Client {
        reader: Lines<BufReader<tokio::io::ReadHalf<DuplexStream>>>,
        writer: tokio::io::WriteHalf<DuplexStream>,
    }

    impl Client {
        async fn send(&mut self, msg: Value) {
            let mut line = msg.to_string();
            line.push('\n');
            self.writer.write_all(line.as_bytes()).await.unwrap();
        }

        async fn recv(&mut self) -> Value {
            let line = tokio::time::timeout(Duration::from_secs(5), self.reader.next_line())
                .await
                .expect("timed out waiting for server reply")
                .unwrap()
                .expect("server closed the connection");
            serde_json::from_str(&line).unwrap()
        }
    }

    fn start_server() -> Client {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server_io);
        let transport = PreInitGuard::new(AsyncRwTransport::new_server(server_read, server_write));
        // The handshake and tools/list never reach the browser service.
        let (cmd_tx, _) = mpsc::channel(1);
        tokio::spawn(async move {
            let handle = McpServer::new(cmd_tx).serve(transport).await.unwrap();
            let _ = handle.waiting().await;
        });
        let (client_read, client_write) = tokio::io::split(client_io);
        Client {
            reader: BufReader::new(client_read).lines(),
            writer: client_write,
        }
    }

    #[tokio::test]
    async fn pre_init_requests_get_errors_and_session_continues() {
        let mut client = start_server();

        // Issue #3: Go SDK >= 1.7 probes with server/discover before initialize.
        client
            .send(json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {}}))
            .await;
        assert_eq!(
            client.recv().await,
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "Method not found"}})
        );

        // A notification gets no reply; a known method is rejected until initialized.
        client
            .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        client
            .send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
            .await;
        let reply = client.recv().await;
        assert_eq!(reply["id"], 2);
        assert_eq!(reply["error"]["code"], -32600);

        client
            .send(json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}))
            .await;
        assert_eq!(
            client.recv().await,
            json!({"jsonrpc": "2.0", "id": 3, "result": {}})
        );

        client
            .send(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0"}
                }
            }))
            .await;
        let reply = client.recv().await;
        assert_eq!(reply["id"], 4);
        assert_eq!(reply["result"]["serverInfo"]["name"], "browser39");

        client
            .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        client
            .send(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/list"}))
            .await;
        let reply = client.recv().await;
        assert_eq!(reply["id"], 5);
        assert!(!reply["result"]["tools"].as_array().unwrap().is_empty());
    }
}
