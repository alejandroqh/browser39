use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::{mpsc, oneshot};

use crate::core::config::Config;
use crate::core::page::*;
use crate::core::redaction::Transport;
use crate::service::service::BrowserService;

use super::params::*;

pub enum McpCommand {
    Fetch {
        params: FetchParams,
        tx: oneshot::Sender<Result<PageResult>>,
    },
    Click {
        params: ClickParams,
        tx: oneshot::Sender<Result<PageResult>>,
    },
    Links {
        tx: oneshot::Sender<Result<LinksResult>>,
    },
    Back {
        tx: oneshot::Sender<Result<PageResult>>,
    },
    Forward {
        tx: oneshot::Sender<Result<PageResult>>,
    },
    Info {
        tx: oneshot::Sender<InfoResult>,
    },
    DomQuery {
        params: DomQueryParams,
        tx: oneshot::Sender<Result<serde_json::Value>>,
    },
    Fill {
        params: FillParams,
        tx: oneshot::Sender<Result<FillResult>>,
    },
    Submit {
        params: SubmitParams,
        tx: oneshot::Sender<Result<PageResult>>,
    },
    Cookies {
        params: CookiesParams,
        tx: oneshot::Sender<Result<CookiesResult>>,
    },
    SetCookie {
        params: SetCookieParams,
        tx: oneshot::Sender<Result<SetCookieResult>>,
    },
    DeleteCookie {
        params: DeleteCookieParams,
        tx: oneshot::Sender<Result<DeleteResult>>,
    },
    StorageGet {
        params: StorageGetParams,
        tx: oneshot::Sender<Result<StorageGetResult>>,
    },
    StorageSet {
        params: StorageSetParams,
        tx: oneshot::Sender<Result<StorageGetResult>>,
    },
    StorageDelete {
        params: StorageDeleteParams,
        tx: oneshot::Sender<Result<DeleteResult>>,
    },
    StorageList {
        params: StorageListParams,
        tx: oneshot::Sender<Result<StorageListResult>>,
    },
    StorageClear {
        params: StorageClearParams,
        tx: oneshot::Sender<Result<StorageClearResult>>,
    },
    History {
        params: HistoryParams,
        tx: oneshot::Sender<HistoryResult>,
    },
    Search {
        params: SearchParams,
        tx: oneshot::Sender<Result<PageResult>>,
    },
    // Resources
    GetPageMarkdown {
        tx: oneshot::Sender<Result<String>>,
    },
    GetPageMeta {
        tx: oneshot::Sender<Result<PageMetadata>>,
    },
}

struct BrowserServiceRunner {
    service: BrowserService,
    rx: mpsc::Receiver<McpCommand>,
}

impl BrowserServiceRunner {
    async fn run(mut self) {
        while let Some(cmd) = self.rx.recv().await {
            self.dispatch(cmd).await;
        }
    }

    async fn dispatch(&mut self, cmd: McpCommand) {
        match cmd {
            McpCommand::Fetch { params, tx } => {
                let result = self.dispatch_fetch(params).await;
                let _ = tx.send(self.redact(result, BrowserService::redact_page));
            }
            McpCommand::Click { params, tx } => {
                let result = self.dispatch_click(params).await;
                let _ = tx.send(self.redact(result, BrowserService::redact_page));
            }
            McpCommand::Links { tx } => {
                let _ = tx.send(self.service.links());
            }
            McpCommand::Back { tx } => {
                let result = self.service.back();
                let _ = tx.send(self.redact(result, BrowserService::redact_page));
            }
            McpCommand::Forward { tx } => {
                let result = self.service.forward();
                let _ = tx.send(self.redact(result, BrowserService::redact_page));
            }
            McpCommand::Info { tx } => {
                let _ = tx.send(self.service.info());
            }
            McpCommand::DomQuery { params, tx } => {
                let _ = tx.send(self.dispatch_dom_query(params));
            }
            McpCommand::Fill { params, tx } => {
                let _ = tx.send(self.dispatch_fill(params));
            }
            McpCommand::Submit { params, tx } => {
                let result = self.dispatch_submit(params).await;
                let _ = tx.send(self.redact(result, BrowserService::redact_page));
            }
            McpCommand::Cookies { params, tx } => {
                let result = self.service.cookies(params.domain.as_deref());
                let _ = tx.send(self.redact(result, BrowserService::redact_cookies));
            }
            McpCommand::SetCookie { params, tx } => {
                // Resolve secret handles in value
                let value = self.service.resolve_secrets(&params.value);
                let _ = tx.send(self.service.set_cookie(
                    &params.name,
                    &value,
                    &params.domain,
                    params.path.as_deref().unwrap_or("/"),
                    params.secure,
                    params.http_only,
                    params.max_age_secs,
                ));
            }
            McpCommand::DeleteCookie { params, tx } => {
                let _ = tx.send(self.service.delete_cookie(&params.name, &params.domain));
            }
            McpCommand::StorageGet { params, tx } => {
                let result = self
                    .service
                    .storage_get(&params.key, params.origin.as_deref());
                let _ = tx.send(self.redact(result, BrowserService::redact_storage_get_result));
            }
            McpCommand::StorageSet { params, tx } => {
                // Resolve secret handles in value
                let value = self.service.resolve_secrets(&params.value);
                let _ = tx.send(self.service.storage_set(
                    &params.key,
                    &value,
                    params.origin.as_deref(),
                ));
            }
            McpCommand::StorageDelete { params, tx } => {
                let _ = tx.send(
                    self.service
                        .storage_delete(&params.key, params.origin.as_deref()),
                );
            }
            McpCommand::StorageList { params, tx } => {
                let result = self.service.storage_list(params.origin.as_deref());
                let _ = tx.send(self.redact(result, BrowserService::redact_storage_list_result));
            }
            McpCommand::StorageClear { params, tx } => {
                let _ = tx.send(self.service.storage_clear(params.origin.as_deref()));
            }
            McpCommand::History { params, tx } => {
                let limit = params.limit.unwrap_or(10);
                let _ = tx.send(self.service.history(params.query.as_deref(), limit));
            }
            McpCommand::Search { params, tx } => {
                let result = self.dispatch_search(params).await;
                let _ = tx.send(self.redact(result, BrowserService::redact_page));
            }
            // Resources
            McpCommand::GetPageMarkdown { tx } => {
                let _ = tx.send(
                    self.service
                        .current_page_result()
                        .map(|p| p.markdown.clone()),
                );
            }
            McpCommand::GetPageMeta { tx } => {
                let _ = tx.send(
                    self.service
                        .current_page_result()
                        .map(|p| p.meta.clone()),
                );
            }
        }
    }

    fn redact<T>(
        &mut self,
        result: Result<T>,
        f: impl FnOnce(&mut BrowserService, &mut T),
    ) -> Result<T> {
        result.map(|mut val| {
            f(&mut self.service, &mut val);
            val
        })
    }

    async fn dispatch_fetch(&mut self, params: FetchParams) -> Result<PageResult> {
        let headers = params.headers.unwrap_or_default();
        let mut opts = self.service.default_fetch_options();
        if let Some(mt) = params.max_tokens {
            opts.max_tokens = Some(mt);
        }
        if let Some(sel) = params.selector {
            opts.selector = Some(sel);
        }
        if let Some(off) = params.offset {
            opts.offset = off;
        }
        opts.show_selectors_first = params.show_selectors_first;
        opts.download_path = params.download_path;
        // Resolve secret handles in body
        let body = params
            .body
            .map(|b| self.service.resolve_secrets(&b));
        self.service
            .fetch(
                &params.url,
                &params.method,
                &headers,
                body,
                params.auth_profile.as_deref(),
                &opts,
            )
            .await
    }

    async fn dispatch_click(&mut self, params: ClickParams) -> Result<PageResult> {
        let method = HttpMethod::Get;
        let headers = HashMap::new();
        let mut opts = self.service.default_fetch_options();
        if let Some(mt) = params.max_tokens {
            opts.max_tokens = Some(mt);
        }

        if let Some(index) = params.index {
            self.service
                .fetch_by_index(index, &method, &headers, None, None, &opts)
                .await
        } else if let Some(ref text) = params.text {
            self.service
                .fetch_by_text(text, &method, &headers, None, None, &opts)
                .await
        } else {
            anyhow::bail!("click requires either index or text parameter")
        }
    }

    fn dispatch_dom_query(&mut self, params: DomQueryParams) -> Result<serde_json::Value> {
        if let Some(ref selector) = params.selector {
            let attr = params.attr.as_deref().unwrap_or("textContent");
            let result = self.service.dom_query(selector, attr)?;
            Ok(serde_json::to_value(result)?)
        } else if let Some(ref script) = params.script {
            let result = self.service.dom_script(script)?;
            Ok(serde_json::to_value(result)?)
        } else {
            anyhow::bail!("dom_query requires either selector or script parameter")
        }
    }

    fn dispatch_fill(&mut self, params: FillParams) -> Result<FillResult> {
        let fields: Vec<(String, String)> = if let Some(ref fields_vec) = params.fields {
            fields_vec
                .iter()
                .map(|f| {
                    let value = self.service.resolve_secrets(&f.value);
                    (f.selector.clone(), value)
                })
                .collect()
        } else if let (Some(selector), Some(value)) = (&params.selector, &params.value) {
            let value = self.service.resolve_secrets(value);
            vec![(selector.clone(), value)]
        } else {
            anyhow::bail!("fill requires either selector+value or fields array")
        };
        self.service.fill(&fields)
    }

    async fn dispatch_search(&mut self, params: SearchParams) -> Result<PageResult> {
        let url = self.service.search_url(&params.query);
        let mut opts = self.service.default_fetch_options();
        if let Some(mt) = params.max_tokens {
            opts.max_tokens = Some(mt);
        }
        opts.show_selectors_first = false;
        self.service
            .fetch(
                &url,
                &HttpMethod::Get,
                &HashMap::new(),
                None,
                None,
                &opts,
            )
            .await
    }

    async fn dispatch_submit(&mut self, params: SubmitParams) -> Result<PageResult> {
        let mut opts = self.service.default_fetch_options();
        if let Some(mt) = params.max_tokens {
            opts.max_tokens = Some(mt);
        }
        self.service.submit(&params.selector, &opts).await
    }
}

pub fn spawn_browser_service(
    config: Config,
    store: Box<dyn crate::core::session_store::SessionStore>,
) -> mpsc::Sender<McpCommand> {
    let (tx, rx) = mpsc::channel(32);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime for browser service");
        rt.block_on(async {
            let service = BrowserService::with_transport(config, Transport::Mcp, store)
                .await
                .expect("failed to create BrowserService");
            let runner = BrowserServiceRunner { service, rx };
            runner.run().await;
        });
    });
    tx
}

pub async fn send_cmd<T>(
    tx: &mpsc::Sender<McpCommand>,
    f: impl FnOnce(oneshot::Sender<T>) -> McpCommand,
) -> Result<T, rmcp::ErrorData> {
    let (resp_tx, resp_rx) = oneshot::channel();
    tx.send(f(resp_tx))
        .await
        .map_err(|_| {
            rmcp::ErrorData::internal_error("browser service channel closed", None)
        })?;
    resp_rx.await.map_err(|_| {
        rmcp::ErrorData::internal_error("browser service response dropped", None)
    })
}
