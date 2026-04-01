use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use tokio::sync::mpsc;

use super::commands::{McpCommand, send_cmd};
use super::params::*;

pub struct McpServer {
    tool_router: ToolRouter<Self>,
    cmd_tx: mpsc::Sender<McpCommand>,
}

impl McpServer {
    pub fn new(cmd_tx: mpsc::Sender<McpCommand>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            cmd_tx,
        }
    }
}

#[tool_router]
impl McpServer {
    #[tool(description = "Fetch a URL and return the page as token-optimized markdown")]
    async fn browser39_fetch(
        &self,
        Parameters(params): Parameters<FetchParams>,
    ) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Fetch { params, tx }).await
    }

    #[tool(description = "Follow a link on the current page by index number or link text")]
    async fn browser39_click(
        &self,
        Parameters(params): Parameters<ClickParams>,
    ) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Click { params, tx }).await
    }

    #[tool(description = "List all links on the current page")]
    async fn browser39_links(&self) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::Links { tx }).await
    }

    #[tool(description = "Query the DOM with a CSS selector or JavaScript")]
    async fn browser39_dom_query(
        &self,
        Parameters(params): Parameters<DomQueryParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::DomQuery { params, tx }).await
    }

    #[tool(description = "Fill form field(s) by CSS selector")]
    async fn browser39_fill(
        &self,
        Parameters(params): Parameters<FillParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::Fill { params, tx }).await
    }

    #[tool(description = "Submit a form by CSS selector")]
    async fn browser39_submit(
        &self,
        Parameters(params): Parameters<SubmitParams>,
    ) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Submit { params, tx }).await
    }

    #[tool(description = "List cookies for the current session")]
    async fn browser39_cookies(
        &self,
        Parameters(params): Parameters<CookiesParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::Cookies { params, tx }).await
    }

    #[tool(description = "Set a cookie")]
    async fn browser39_set_cookie(
        &self,
        Parameters(params): Parameters<SetCookieParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::SetCookie { params, tx }).await
    }

    #[tool(description = "Delete a cookie")]
    async fn browser39_delete_cookie(
        &self,
        Parameters(params): Parameters<DeleteCookieParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::DeleteCookie { params, tx }).await
    }

    #[tool(description = "Get a localStorage value")]
    async fn browser39_storage_get(
        &self,
        Parameters(params): Parameters<StorageGetParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageGet { params, tx }).await
    }

    #[tool(description = "Set a localStorage value")]
    async fn browser39_storage_set(
        &self,
        Parameters(params): Parameters<StorageSetParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageSet { params, tx }).await
    }

    #[tool(description = "Delete a localStorage key")]
    async fn browser39_storage_delete(
        &self,
        Parameters(params): Parameters<StorageDeleteParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageDelete { params, tx }).await
    }

    #[tool(description = "List localStorage entries")]
    async fn browser39_storage_list(
        &self,
        Parameters(params): Parameters<StorageListParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageList { params, tx }).await
    }

    #[tool(description = "Clear localStorage for an origin")]
    async fn browser39_storage_clear(
        &self,
        Parameters(params): Parameters<StorageClearParams>,
    ) -> Result<String, String> {
        data_cmd(&self.cmd_tx, |tx| McpCommand::StorageClear { params, tx }).await
    }

    #[tool(description = "Search the web using the configured search engine (default: DuckDuckGo)")]
    async fn browser39_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Search { params, tx }).await
    }

    #[tool(description = "Navigate back in history")]
    async fn browser39_back(&self) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Back { tx }).await
    }

    #[tool(description = "Navigate forward in history")]
    async fn browser39_forward(&self) -> Result<String, String> {
        page_cmd(&self.cmd_tx, |tx| McpCommand::Forward { tx }).await
    }

    #[tool(description = "Search or list browsing history. Use query to search by URL/title, or omit to list recent pages.")]
    async fn browser39_history(
        &self,
        Parameters(params): Parameters<HistoryParams>,
    ) -> Result<String, String> {
        let result = send_cmd(&self.cmd_tx, |tx| McpCommand::History { params, tx })
            .await
            .map_err(|e| e.to_string())?;
        to_json(&result)
    }

    #[tool(description = "Get session info and liveness status")]
    async fn browser39_info(&self) -> Result<String, String> {
        let result = send_cmd(&self.cmd_tx, |tx| McpCommand::Info { tx })
            .await
            .map_err(|e| e.to_string())?;
        to_json(&result)
    }
}

/// Helper for tools that return a page (fetch, click, back, forward, submit).
async fn page_cmd(
    tx: &mpsc::Sender<McpCommand>,
    f: impl FnOnce(tokio::sync::oneshot::Sender<anyhow::Result<PageResult>>) -> McpCommand,
) -> Result<String, String> {
    send_cmd(tx, f)
        .await
        .map_err(|e| e.to_string())?
        .map(|page| format_page_result(&page))
        .map_err(format_error)
}

/// Helper for tools that return serializable data.
async fn data_cmd<T: serde::Serialize>(
    tx: &mpsc::Sender<McpCommand>,
    f: impl FnOnce(tokio::sync::oneshot::Sender<anyhow::Result<T>>) -> McpCommand,
) -> Result<String, String> {
    send_cmd(tx, f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(format_error)
        .and_then(|v| to_json(&v))
}

fn to_json(v: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string_pretty(v).map_err(|e| format!("serialization failed: {e}"))
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("browser39", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "browser39 is a headless web browser for AI agents. \
             Use browser39_fetch to load pages as markdown, browser39_links to list links, \
             browser39_click to follow links, and browser39_dom_query for CSS/JS queries.",
        )
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl std::future::Future<
        Output = Result<ListResourcesResult, rmcp::ErrorData>,
    > + Send
           + '_ {
        std::future::ready(Ok(ListResourcesResult::with_all_items(vec![
            make_resource("browser39://page", "Current page", Some("text/markdown"), Some("Current page content as markdown")),
            make_resource("browser39://page/links", "Page links", Some("application/json"), Some("Links on the current page as JSON")),
            make_resource("browser39://page/meta", "Page metadata", Some("application/json"), Some("Current page metadata as JSON")),
            make_resource("browser39://cookies", "Cookies", Some("application/json"), Some("Cookies for the current domain as JSON")),
        ])))
    }

    #[allow(clippy::manual_async_fn)]
    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl std::future::Future<
        Output = Result<ReadResourceResult, rmcp::ErrorData>,
    > + Send
           + '_ {
        async move {
            let uri = &request.uri;
            match uri.as_str() {
                "browser39://page" => {
                    let markdown = send_cmd(&self.cmd_tx, |tx| McpCommand::GetPageMarkdown { tx })
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    Ok(ReadResourceResult::new(vec![ResourceContents::text(
                        markdown, uri,
                    )]))
                }
                "browser39://page/links" => {
                    let links = send_cmd(&self.cmd_tx, |tx| McpCommand::Links { tx })
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    let json = serde_json::to_string_pretty(&links)
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    Ok(ReadResourceResult::new(vec![ResourceContents::text(
                        json, uri,
                    )]))
                }
                "browser39://page/meta" => {
                    let meta = send_cmd(&self.cmd_tx, |tx| McpCommand::GetPageMeta { tx })
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    let json = serde_json::to_string_pretty(&meta)
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    Ok(ReadResourceResult::new(vec![ResourceContents::text(
                        json, uri,
                    )]))
                }
                "browser39://cookies" => {
                    let cookies = send_cmd(
                        &self.cmd_tx,
                        |tx| McpCommand::Cookies {
                            params: super::params::CookiesParams { domain: None },
                            tx,
                        },
                    )
                    .await
                    .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?
                    .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    let json = serde_json::to_string_pretty(&cookies)
                        .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
                    Ok(ReadResourceResult::new(vec![ResourceContents::text(
                        json, uri,
                    )]))
                }
                _ => Err(rmcp::ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("unknown resource: {uri}"),
                    None,
                )),
            }
        }
    }
}

use crate::core::page::PageResult;

fn make_resource(
    uri: &str,
    name: &str,
    mime_type: Option<&str>,
    description: Option<&str>,
) -> Resource {
    Resource::new(
        RawResource {
            uri: uri.into(),
            name: name.into(),
            title: None,
            description: description.map(Into::into),
            mime_type: mime_type.map(Into::into),
            size: None,
            icons: None,
            meta: None,
        },
        None,
    )
}

fn format_page_result(page: &PageResult) -> String {
    let mut out = page.markdown.clone();
    // Append metadata summary
    out.push_str("\n\n---\n");
    if let Some(ref title) = page.title {
        out.push_str(&format!("**Title:** {title}\n"));
    }
    out.push_str(&format!("**URL:** {}\n", page.url));
    out.push_str(&format!("**Status:** {}\n", page.status));
    if let Some(ref links) = page.links {
        out.push_str(&format!("**Links:** {}\n", links.len()));
    }
    out.push_str(&format!(
        "**Tokens (est):** {}\n",
        page.stats.tokens_est
    ));
    if page.truncated {
        if let Some(next) = page.next_offset {
            out.push_str(&format!(
                "**Truncated:** true (use offset={next} to continue)\n"
            ));
        } else {
            out.push_str("**Truncated:** true\n");
        }
    }
    if let Some(ref selectors) = page.content_selectors {
        out.push_str("\n**Content selectors:**\n");
        for cs in selectors {
            if let Some(ref label) = cs.label {
                out.push_str(&format!(
                    "- `{}` — {label} (~{} tokens)\n",
                    cs.selector, cs.tokens_est
                ));
            } else {
                out.push_str(&format!("- `{}` — ~{} tokens\n", cs.selector, cs.tokens_est));
            }
        }
    }
    out
}

fn format_error(err: anyhow::Error) -> String {
    let se = crate::service::service::classify_error(err);
    let retry_hint = if se.code.retryable() {
        " (retryable)"
    } else {
        ""
    };
    format!("[{:?}] {}{retry_hint}", se.code, se.message)
}
