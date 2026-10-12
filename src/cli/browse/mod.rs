//! `browser39 browse` — read pages in an interactive, elinks-style terminal
//! viewer, or print them as markdown when stdout is not a terminal.

mod clipboard;
mod doc;
mod table;
mod ui;

use std::collections::HashMap;
use std::io::IsTerminal;

use anyhow::{Result, anyhow};

use crate::core::page::{FetchOptions, HttpMethod};
use crate::service::service::BrowserService;
use doc::Doc;

pub struct BrowseArgs {
    /// URL, bare domain, or search terms.
    pub target: String,
    /// Narrow pages to their main content region.
    pub reader: bool,
    /// Open at, or print only, the section whose heading matches.
    pub section: Option<String>,
    /// Print markdown instead of opening the viewer.
    pub print: bool,
}

pub async fn run_browse(service: &mut BrowserService, args: BrowseArgs) -> Result<()> {
    let interactive =
        !args.print && std::io::stdout().is_terminal() && std::io::stdin().is_terminal();
    let url = resolve_target(service, &args.target);
    let mut browser = Browser {
        service,
        reader: args.reader,
    };
    let doc = browser.open(&url).await?;

    if interactive {
        return ui::run(browser, doc, args.section.as_deref()).await;
    }

    let lines = match args.section.as_deref() {
        Some(name) => {
            let h = doc.find_section(name).ok_or_else(|| {
                let names: Vec<&str> = doc.headings.iter().map(|h| h.text.as_str()).collect();
                anyhow!(
                    "no section matching {name:?}. Sections: {}",
                    names.join(" | ")
                )
            })?;
            doc.section_lines(h)
        }
        None => 0..doc.lines.len(),
    };
    println!("{}", doc.markdown_for(lines));
    Ok(())
}

/// Turn user input into a URL: full URLs pass through, bare domains get
/// `https://`, anything else becomes a web search.
fn resolve_target(service: &BrowserService, input: &str) -> String {
    let s = input.trim();
    if s.contains("://") {
        return s.to_string();
    }
    let looks_like_host = !s.contains(char::is_whitespace)
        && (s.contains('.') || s == "localhost" || s.starts_with("localhost:"));
    if looks_like_host {
        format!("https://{s}")
    } else {
        service.search_url(s)
    }
}

/// Navigation on top of `BrowserService`: fetch, history and re-rendering.
struct Browser<'a> {
    service: &'a mut BrowserService,
    reader: bool,
}

impl Browser<'_> {
    /// How the viewer renders pages: complete (no token limit), numbered
    /// links that keep repeated link text, and reader mode when enabled.
    fn options(&self) -> FetchOptions {
        let defaults = self.service.default_fetch_options();
        FetchOptions {
            // `fetch` swaps a `None` limit for the configured default
            max_tokens: Some(u64::MAX),
            strip_nav: self.reader || defaults.strip_nav,
            include_links: true,
            compact_links: true,
            show_selectors_first: false,
            keep_repeated_links: true,
            main_content: self.reader,
            ..defaults
        }
    }

    async fn open(&mut self, url: &str) -> Result<Doc> {
        let opts = self.options();
        let page = self
            .service
            .fetch(url, &HttpMethod::Get, &HashMap::new(), None, None, &opts)
            .await?;
        let markdown = page.markdown;
        Ok(Doc::new(
            page.url,
            page.title,
            &markdown,
            page.links.unwrap_or_default(),
        ))
    }

    /// Re-render the service's current page into a `Doc`.
    fn render(&self) -> Result<Doc> {
        let converted = self.service.render_current(&self.options())?;
        let page = self.service.current_page_result()?;
        Ok(Doc::new(
            page.url.clone(),
            page.title.clone(),
            &converted.markdown,
            converted.links,
        ))
    }

    /// Move back (or forward) in history and render that page.
    fn step(&mut self, back: bool) -> Result<Doc> {
        if back {
            self.service.back()?;
        } else {
            self.service.forward()?;
        }
        self.render()
    }

    fn history_index(&self) -> usize {
        self.service.info().history_index
    }

    fn resolve(&self, input: &str) -> String {
        resolve_target(self.service, input)
    }
}
