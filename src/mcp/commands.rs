use std::collections::HashMap;

use anyhow::{Context, Result};
use tokio::sync::{mpsc, oneshot};

use crate::core::config::{
    AuthProfileConfig, Config, CookieConfig, HeaderRuleConfig, PersistenceMode, StorageConfig,
};
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
    // Config management
    ConfigShow {
        params: ConfigShowParams,
        tx: oneshot::Sender<Result<serde_json::Value>>,
    },
    ConfigSet {
        params: ConfigSetParams,
        tx: oneshot::Sender<Result<String>>,
    },
    ConfigAuthSet {
        params: ConfigAuthSetParams,
        tx: oneshot::Sender<Result<String>>,
    },
    ConfigAuthDelete {
        params: ConfigAuthDeleteParams,
        tx: oneshot::Sender<Result<String>>,
    },
    ConfigCookieSet {
        params: ConfigCookieSetParams,
        tx: oneshot::Sender<Result<String>>,
    },
    ConfigCookieDelete {
        params: ConfigCookieDeleteParams,
        tx: oneshot::Sender<Result<String>>,
    },
    ConfigStorageSet {
        params: ConfigStorageSetParams,
        tx: oneshot::Sender<Result<String>>,
    },
    ConfigStorageDelete {
        params: ConfigStorageDeleteParams,
        tx: oneshot::Sender<Result<String>>,
    },
    ConfigHeaderSet {
        params: ConfigHeaderSetParams,
        tx: oneshot::Sender<Result<String>>,
    },
    ConfigHeaderDelete {
        params: ConfigHeaderDeleteParams,
        tx: oneshot::Sender<Result<String>>,
    },
}

const CONFIG_SECTIONS: &[&str] = &[
    "session", "search", "auth", "cookies", "storage", "headers", "security",
];

const CONFIG_KEYS: &[&str] = &[
    "session.start_url",
    "session.user_agent",
    "session.timeout_secs",
    "session.max_redirects",
    "session.persistence",
    "session.defaults.max_tokens",
    "session.defaults.strip_nav",
    "session.defaults.include_links",
    "session.defaults.include_images",
    "search.engine",
];

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
            // Config management
            McpCommand::ConfigShow { params, tx } => {
                let _ = tx.send(self.dispatch_config_show(params));
            }
            McpCommand::ConfigSet { params, tx } => {
                let _ = tx.send(self.dispatch_config_set(params));
            }
            McpCommand::ConfigAuthSet { params, tx } => {
                let _ = tx.send(self.dispatch_config_auth_set(params));
            }
            McpCommand::ConfigAuthDelete { params, tx } => {
                let _ = tx.send(self.dispatch_config_auth_delete(params));
            }
            McpCommand::ConfigCookieSet { params, tx } => {
                let _ = tx.send(self.dispatch_config_cookie_set(params));
            }
            McpCommand::ConfigCookieDelete { params, tx } => {
                let _ = tx.send(self.dispatch_config_cookie_delete(params));
            }
            McpCommand::ConfigStorageSet { params, tx } => {
                let _ = tx.send(self.dispatch_config_storage_set(params));
            }
            McpCommand::ConfigStorageDelete { params, tx } => {
                let _ = tx.send(self.dispatch_config_storage_delete(params));
            }
            McpCommand::ConfigHeaderSet { params, tx } => {
                let _ = tx.send(self.dispatch_config_header_set(params));
            }
            McpCommand::ConfigHeaderDelete { params, tx } => {
                let _ = tx.send(self.dispatch_config_header_delete(params));
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

    // ─── Config dispatch helpers ───────────────────────────────────

    /// Load config from disk, apply a mutation, save, and reload into service.
    fn mutate_config(&mut self, f: impl FnOnce(&mut Config) -> Result<String>) -> Result<String> {
        let mut config = Config::load(None)?;
        let msg = f(&mut config)?;
        config.save(None)?;
        self.service.reload_config(config)?;
        Ok(msg)
    }

    fn dispatch_config_show(&self, params: ConfigShowParams) -> Result<serde_json::Value> {
        let section = params.section.as_deref();
        if let Some(s) = section {
            if !CONFIG_SECTIONS.contains(&s) {
                anyhow::bail!("unknown config section '{s}'. Valid: {}", CONFIG_SECTIONS.join(", "));
            }
        }
        Ok(self.service.config().masked_json(section))
    }

    fn dispatch_config_set(&mut self, params: ConfigSetParams) -> Result<String> {
        self.mutate_config(|config| {
            match params.key.as_str() {
                "session.start_url" => {
                    config.session.start_url = if params.value == "null" || params.value.is_empty() {
                        None
                    } else {
                        Some(params.value.clone())
                    };
                }
                "session.user_agent" => {
                    config.session.user_agent = params.value.clone();
                }
                "session.timeout_secs" => {
                    config.session.timeout_secs = params.value.parse()
                        .context("timeout_secs must be an integer")?;
                }
                "session.max_redirects" => {
                    config.session.max_redirects = params.value.parse()
                        .context("max_redirects must be an integer")?;
                }
                "session.persistence" => {
                    config.session.persistence = match params.value.as_str() {
                        "disk" => PersistenceMode::Disk,
                        "memory" => PersistenceMode::Memory,
                        _ => anyhow::bail!("persistence must be 'disk' or 'memory'"),
                    };
                }
                "session.defaults.max_tokens" => {
                    config.session.defaults.max_tokens = if params.value == "null" || params.value.is_empty() {
                        None
                    } else {
                        Some(params.value.parse().context("max_tokens must be an integer")?)
                    };
                }
                "session.defaults.strip_nav" => {
                    config.session.defaults.strip_nav = params.value.parse()
                        .context("strip_nav must be true or false")?;
                }
                "session.defaults.include_links" => {
                    config.session.defaults.include_links = params.value.parse()
                        .context("include_links must be true or false")?;
                }
                "session.defaults.include_images" => {
                    config.session.defaults.include_images = params.value.parse()
                        .context("include_images must be true or false")?;
                }
                "search.engine" => {
                    config.search.engine = params.value.clone();
                }
                other => {
                    anyhow::bail!(
                        "unknown config key '{other}'. Allowed: {}",
                        CONFIG_KEYS.join(", ")
                    );
                }
            }
            Ok(format!("Set {key} = {value}", key = params.key, value = params.value))
        })
    }

    fn dispatch_config_auth_set(&mut self, params: ConfigAuthSetParams) -> Result<String> {
        self.mutate_config(|config| {
            let name = params.name;
            config.auth.insert(name.clone(), AuthProfileConfig {
                header: params.header,
                value: params.value,
                value_env: params.value_env,
                value_prefix: params.value_prefix,
                domains: params.domains,
                resolved_value: None,
            });
            Ok(format!("Auth profile '{name}' saved"))
        })
    }

    fn dispatch_config_auth_delete(&mut self, params: ConfigAuthDeleteParams) -> Result<String> {
        self.mutate_config(|config| {
            if config.auth.remove(&params.name).is_some() {
                Ok(format!("Auth profile '{}' deleted", params.name))
            } else {
                anyhow::bail!("auth profile '{}' not found", params.name)
            }
        })
    }

    fn dispatch_config_cookie_set(&mut self, params: ConfigCookieSetParams) -> Result<String> {
        let label = format!("{}@{}", params.name, params.domain);
        self.mutate_config(|config| {
            // Remove existing entry with same name+domain
            config.cookies.retain(|c| !(c.name == params.name && c.domain == params.domain));
            config.cookies.push(CookieConfig {
                name: params.name,
                value: params.value,
                value_env: params.value_env,
                domain: params.domain,
                path: params.path.unwrap_or_else(|| "/".into()),
                secure: params.secure,
                http_only: params.http_only,
                sensitive: params.sensitive,
                resolved_value: None,
            });
            Ok(format!("Cookie config '{label}' saved"))
        })
    }

    fn dispatch_config_cookie_delete(&mut self, params: ConfigCookieDeleteParams) -> Result<String> {
        self.mutate_config(|config| {
            let before = config.cookies.len();
            config.cookies.retain(|c| !(c.name == params.name && c.domain == params.domain));
            if config.cookies.len() < before {
                Ok(format!("Cookie config '{}@{}' deleted", params.name, params.domain))
            } else {
                anyhow::bail!("cookie config '{}@{}' not found", params.name, params.domain)
            }
        })
    }

    fn dispatch_config_storage_set(&mut self, params: ConfigStorageSetParams) -> Result<String> {
        let label = format!("{}:{}", params.origin, params.key);
        self.mutate_config(|config| {
            // Remove existing entry with same origin+key
            config.storage.retain(|s| !(s.origin == params.origin && s.key == params.key));
            config.storage.push(StorageConfig {
                origin: params.origin,
                key: params.key,
                value: params.value,
                value_env: params.value_env,
                sensitive: params.sensitive,
                resolved_value: None,
            });
            Ok(format!("Storage config '{label}' saved"))
        })
    }

    fn dispatch_config_storage_delete(&mut self, params: ConfigStorageDeleteParams) -> Result<String> {
        self.mutate_config(|config| {
            let before = config.storage.len();
            config.storage.retain(|s| !(s.origin == params.origin && s.key == params.key));
            if config.storage.len() < before {
                Ok(format!("Storage config '{}:{}' deleted", params.origin, params.key))
            } else {
                anyhow::bail!("storage config '{}:{}' not found", params.origin, params.key)
            }
        })
    }

    fn dispatch_config_header_set(&mut self, params: ConfigHeaderSetParams) -> Result<String> {
        let domains_label = params.domains.join(", ");
        self.mutate_config(|config| {
            // Remove existing rule with same domains
            config.headers.retain(|h| h.domains != params.domains);
            config.headers.push(HeaderRuleConfig {
                domains: params.domains,
                values: params.values,
            });
            Ok(format!("Header rule for [{domains_label}] saved"))
        })
    }

    fn dispatch_config_header_delete(&mut self, params: ConfigHeaderDeleteParams) -> Result<String> {
        self.mutate_config(|config| {
            let before = config.headers.len();
            config.headers.retain(|h| h.domains != params.domains);
            if config.headers.len() < before {
                Ok(format!("Header rule for [{}] deleted", params.domains.join(", ")))
            } else {
                anyhow::bail!("header rule for [{}] not found", params.domains.join(", "))
            }
        })
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
