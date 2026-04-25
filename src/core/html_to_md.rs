use ego_tree::NodeRef;
use scraper::{Html, Node, Selector};
use std::collections::HashSet;
use std::fmt::Write;
use std::mem;
use std::sync::LazyLock;

use super::page::{ContentSelector, FetchOptions, Link, PageMetadata};
use super::url::resolve_href;

// --- Compiled selectors (cached) ---

pub(crate) static SEL_TITLE: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("title").unwrap());
pub(crate) static SEL_HTML: LazyLock<Selector> = LazyLock::new(|| Selector::parse("html").unwrap());
pub(crate) static SEL_BODY: LazyLock<Selector> = LazyLock::new(|| Selector::parse("body").unwrap());
static SEL_META: LazyLock<Selector> = LazyLock::new(|| Selector::parse("meta").unwrap());
static SEL_LINKS: LazyLock<Selector> = LazyLock::new(|| Selector::parse("a[href]").unwrap());
static SEL_IMG: LazyLock<Selector> = LazyLock::new(|| Selector::parse("img").unwrap());

// Content selector candidates for detect_content_selectors
static CONTENT_SELECTORS: LazyLock<Vec<(&str, Selector)>> = LazyLock::new(|| {
    [
        "main",
        "article",
        "[role=\"main\"]",
        "#content",
        "#main-content",
        "#mw-content-text",
        ".post-content",
        ".entry-content",
        ".article-body",
    ]
    .iter()
    .filter_map(|s| Selector::parse(s).ok().map(|sel| (*s, sel)))
    .collect()
});

/// Minimum token count for a selector to be considered meaningful.
const SELECTOR_MIN_TOKENS: u64 = 500;

/// Estimate token count from byte length (4 bytes per token heuristic).
pub(crate) fn estimate_tokens(byte_len: usize) -> u64 {
    (byte_len as u64) / 4
}

/// Given a heading node, return the node from which to walk siblings.
/// If the heading is wrapped (e.g. `<div class="mw-heading"><h2>...</h2></div>`),
/// return the wrapper; otherwise return the heading itself.
fn heading_section_start(heading: NodeRef<'_, Node>) -> NodeRef<'_, Node> {
    if let Some(parent) = heading.parent()
        && let Some(el) = parent.value().as_element()
    {
        // Common wrapper patterns: div.mw-heading, div.mw-heading2, etc.
        if el.name() == "div"
            && el.attr("class").is_some_and(|c| {
                c.split_whitespace()
                    .any(|cls| cls.starts_with("mw-heading"))
            })
        {
            return parent;
        }
    }
    heading
}

/// Check if a node contains a heading of the given level or higher.
fn contains_heading_at_or_above(node: NodeRef<'_, Node>, tag: &str) -> bool {
    if let Some(el) = node.value().as_element() {
        let name = el.name();
        if matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") && name <= tag {
            return true;
        }
    }
    // Check children (for wrapper divs containing headings)
    for child in node.children() {
        if let Some(el) = child.value().as_element() {
            let name = el.name();
            if matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") && name <= tag {
                return true;
            }
        }
    }
    false
}

/// Sum text bytes of siblings after `start` until the next heading of same/higher level.
fn section_content_bytes(start: NodeRef<'_, Node>, heading_tag: &str) -> usize {
    let mut bytes: usize = 0;
    let mut sibling = start.next_sibling();
    while let Some(sib) = sibling {
        if contains_heading_at_or_above(sib, heading_tag) {
            break;
        }
        if let Some(el_ref) = scraper::ElementRef::wrap(sib) {
            bytes += el_ref.text().map(|t| t.len()).sum::<usize>();
        } else if let Some(text) = sib.value().as_text() {
            bytes += text.text.len();
        }
        sibling = sib.next_sibling();
    }
    bytes
}

// --- ParsedHtml ---

#[derive(Default)]
pub struct ConvertResult {
    pub markdown: String,
    pub tokens_est: u64,
    pub truncated: bool,
    pub next_offset: Option<u64>,
    pub links: Vec<Link>,
}

pub struct ParsedHtml {
    document: Html,
}

impl ParsedHtml {
    pub fn parse(html: &str) -> Self {
        Self {
            document: Html::parse_document(html),
        }
    }

    pub fn title(&self) -> Option<String> {
        self.document
            .select(&SEL_TITLE)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn metadata(&self) -> PageMetadata {
        let lang = self
            .document
            .select(&SEL_HTML)
            .next()
            .and_then(|el| el.value().attr("lang"))
            .map(|s| s.to_string());

        let mut description = None;
        let mut content_type = None;

        for el in self.document.select(&SEL_META) {
            let name = el
                .value()
                .attr("name")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let http_equiv = el
                .value()
                .attr("http-equiv")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let content = el.value().attr("content");

            if name == "description" {
                description = content.map(|s| s.to_string());
            }
            if http_equiv == "content-type" {
                content_type = content.map(|s| s.to_string());
            }
            if content_type.is_none()
                && let Some(charset) = el.value().attr("charset")
            {
                content_type = Some(format!("text/html; charset={charset}"));
            }
        }

        PageMetadata {
            lang,
            description,
            content_type,
        }
    }

    pub fn links(&self, base_url: Option<&str>) -> Vec<Link> {
        let base = base_url.and_then(|u| reqwest::Url::parse(u).ok());
        let mut seen = HashSet::new();
        self.document
            .select(&SEL_LINKS)
            .enumerate()
            .filter_map(|(i, el)| {
                let mut text = el.text().collect::<String>().trim().to_string();
                let raw_href = el.value().attr("href").unwrap_or_default();
                // For links with no text, try title/aria-label, then img alt
                if text.is_empty() {
                    if let Some(label) = el
                        .value()
                        .attr("title")
                        .or_else(|| el.value().attr("aria-label"))
                        .filter(|s| !s.trim().is_empty())
                    {
                        text = label.trim().to_string();
                    } else if let Some(img) = el.select(&SEL_IMG).next() {
                        text = img
                            .value()
                            .attr("alt")
                            .filter(|a| !a.trim().is_empty())
                            .map(|a| a.trim().to_string())
                            .unwrap_or_else(|| "img".to_string());
                    }
                }
                // Skip links with no text and no useful href
                if text.is_empty() {
                    return None;
                }
                let href = resolve_href(base.as_ref(), raw_href);
                // Skip duplicate URLs — keep only the first occurrence
                if !href.is_empty() && !seen.insert(href.clone()) {
                    return None;
                }
                Some(Link { i, text, href })
            })
            .collect()
    }

    #[cfg(test)]
    pub fn to_markdown(&self) -> String {
        let mut writer = MarkdownWriter::with_options(false, true, true);
        writer.walk_children(self.document.tree.root());
        writer.finish()
    }

    pub fn to_markdown_with_options(
        &self,
        options: &FetchOptions,
        base_url: Option<&str>,
    ) -> ConvertResult {
        let mut writer = MarkdownWriter::with_options(
            options.strip_nav,
            options.include_links,
            options.include_images,
        );
        writer.base_url = base_url.and_then(|u| reqwest::Url::parse(u).ok());
        writer.compact_links = options.compact_links;

        if let Some(ref sel_str) = options.selector {
            if let Ok(sel) = Selector::parse(sel_str) {
                let mut first = true;
                for el in self.document.select(&sel) {
                    if !first {
                        ensure_double_newline(&mut writer.output);
                    }
                    first = false;

                    let tag = el.value().name();
                    if matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                        // Heading auto-expand: render heading + siblings until next same-level heading.
                        // If wrapped (e.g. div.mw-heading), walk from the wrapper's parent level.
                        let section_start = heading_section_start(*el);
                        // Render the heading (or its wrapper)
                        writer.walk(section_start);
                        // Walk subsequent siblings
                        let mut sibling = section_start.next_sibling();
                        while let Some(sib) = sibling {
                            if contains_heading_at_or_above(sib, tag) {
                                break;
                            }
                            writer.walk(sib);
                            sibling = sib.next_sibling();
                        }
                    } else {
                        writer.walk(*el);
                    }
                }
            }
        } else {
            writer.walk_children(self.document.tree.root());
        }

        let links = mem::take(&mut writer.collected_links);
        let markdown = writer.finish();
        let mut result = truncate_markdown(&markdown, options.offset, options.max_tokens);
        result.links = links;
        result
    }

    /// Scan the DOM for meaningful content selectors and return them with token estimates.
    pub fn detect_content_selectors(&self) -> Vec<ContentSelector> {
        let mut results: Vec<ContentSelector> = Vec::new();
        let mut seen_ids: HashSet<ego_tree::NodeId> = HashSet::new();

        // Check static common selectors
        for (name, sel) in CONTENT_SELECTORS.iter() {
            if let Some(el) = self.document.select(sel).next() {
                let node_id = el.id();
                if seen_ids.contains(&node_id) {
                    continue;
                }
                let tokens = estimate_tokens(el.text().map(|t| t.len()).sum());
                if tokens >= SELECTOR_MIN_TOKENS {
                    seen_ids.insert(node_id);
                    results.push(ContentSelector {
                        selector: name.to_string(),
                        tokens_est: tokens,
                        label: None,
                    });
                }
            }
        }

        // Dynamic scan: body direct children with id attributes
        if let Some(body) = self.document.select(&SEL_BODY).next() {
            for child in (*body).children() {
                if let Some(el) = child.value().as_element()
                    && let Some(id) = el.attr("id")
                {
                    let node_id = child.id();
                    if seen_ids.contains(&node_id) {
                        continue;
                    }
                    if let Some(el_ref) = scraper::ElementRef::wrap(child) {
                        let tokens = estimate_tokens(el_ref.text().map(|t| t.len()).sum());
                        if tokens >= SELECTOR_MIN_TOKENS {
                            seen_ids.insert(node_id);
                            results.push(ContentSelector {
                                selector: format!("#{id}"),
                                tokens_est: tokens,
                                label: None,
                            });
                        }
                    }
                }
            }
        }

        // Scan for section headings (h2/h3 with id attributes)
        let heading_sel = Selector::parse("h2[id], h3[id]").unwrap();
        let mut sections: Vec<ContentSelector> = Vec::new();
        for el in self.document.select(&heading_sel) {
            let tag = el.value().name();
            if let Some(id) = el.value().attr("id") {
                let heading_text: String = el.text().collect::<String>();
                let heading_text = heading_text.trim().to_string();
                if heading_text.is_empty() {
                    continue;
                }
                // Walk siblings to estimate section size.
                // If heading is inside a wrapper (e.g. div.mw-heading), walk from the wrapper.
                let section_start = heading_section_start(*el);
                let section_bytes = section_content_bytes(section_start, tag);
                let tokens = estimate_tokens(section_bytes);
                if tokens >= SELECTOR_MIN_TOKENS {
                    sections.push(ContentSelector {
                        selector: format!("#{id}"),
                        tokens_est: tokens,
                        label: Some(heading_text),
                    });
                }
            }
        }

        // Sort content selectors by token count descending
        results.sort_by(|a, b| b.tokens_est.cmp(&a.tokens_est));
        // Sections keep document order (don't sort)
        results.extend(sections);
        results
    }

    /// Quick token estimate from the body text without full markdown conversion.
    pub fn estimate_page_tokens(&self) -> u64 {
        if let Some(body) = self.document.select(&SEL_BODY).next() {
            estimate_tokens(body.text().map(|t| t.len()).sum())
        } else {
            0
        }
    }
}

// --- MarkdownWriter (recursive DOM walker) ---

struct ListContext {
    ordered: bool,
    counter: u32,
}

struct MarkdownWriter {
    output: String,
    list_stack: Vec<ListContext>,
    in_pre: bool,
    in_link: bool,
    strip_nav: bool,
    include_links: bool,
    include_images: bool,
    compact_links: bool,
    base_url: Option<reqwest::Url>,
    seen_hrefs: HashSet<String>,
    collected_links: Vec<Link>,
}

impl MarkdownWriter {
    fn with_options(strip_nav: bool, include_links: bool, include_images: bool) -> Self {
        Self {
            output: String::new(),
            list_stack: Vec::new(),
            in_pre: false,
            in_link: false,
            strip_nav,
            include_links,
            include_images,
            compact_links: false,
            base_url: None,
            seen_hrefs: HashSet::new(),
            collected_links: Vec::new(),
        }
    }

    fn resolve_href(&self, href: &str) -> String {
        resolve_href(self.base_url.as_ref(), href)
    }

    fn walk(&mut self, node: NodeRef<'_, Node>) {
        match node.value() {
            Node::Text(text) => {
                let s = text.text.as_ref();
                if self.in_pre {
                    self.output.push_str(s);
                } else {
                    // Collapse whitespace in normal flow
                    let collapsed = collapse_inline_whitespace(s);
                    if !collapsed.is_empty() {
                        self.output.push_str(&collapsed);
                    }
                }
            }
            Node::Element(el) => {
                let tag = el.name.local.as_ref();
                match tag {
                    "script" | "style" | "noscript" => {}

                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        if self.in_link {
                            // Skip heading markers inside links — they waste tokens
                            self.walk_children(node);
                        } else {
                            let level = tag.as_bytes()[1] - b'0';
                            ensure_newline(&mut self.output);
                            for _ in 0..level {
                                self.output.push('#');
                            }
                            self.output.push(' ');
                            self.walk_children(node);
                            self.output.push('\n');
                        }
                    }

                    "header" | "footer" | "nav" if self.strip_nav => {}

                    "p" | "div" | "section" | "article" | "main" | "header" | "footer" | "nav" => {
                        ensure_newline(&mut self.output);
                        self.walk_children(node);
                        ensure_double_newline(&mut self.output);
                    }

                    "br" => {
                        self.output.push('\n');
                    }

                    "hr" => {
                        ensure_newline(&mut self.output);
                        self.output.push_str("---\n");
                    }

                    "a" => {
                        if self.include_links {
                            let href = el.attr("href").unwrap_or_default();
                            let resolved = self.resolve_href(href);

                            // Skip links whose URL was already emitted
                            if !resolved.is_empty() && self.seen_hrefs.contains(&resolved) {
                                return;
                            }

                            let was_in_link = self.in_link;
                            self.in_link = true;

                            let before_len = self.output.len();
                            self.output.push('[');
                            self.walk_children(node);
                            // Check if link text is empty or just formatting markers
                            let link_text = &self.output[before_len + 1..];
                            let meaningful = link_text.trim().replace(['*', '_', '`'], "");
                            if meaningful.trim().is_empty() {
                                // Try title/aria-label on the <a> element
                                let label = el
                                    .attr("title")
                                    .or_else(|| el.attr("aria-label"))
                                    .filter(|s| !s.trim().is_empty());
                                if let Some(lbl) = label {
                                    self.output.truncate(before_len);
                                    self.output.push('[');
                                    self.output.push_str(lbl.trim());
                                } else {
                                    // Single pass: find first img, grab alt if available
                                    let first_img = node
                                        .descendants()
                                        .filter_map(|n| {
                                            if let Node::Element(e) = n.value()
                                                && e.name.local.as_ref() == "img"
                                            {
                                                return Some(
                                                    e.attr("alt").filter(|a| !a.trim().is_empty()),
                                                );
                                            }
                                            None
                                        })
                                        .next();
                                    self.output.truncate(before_len);
                                    match first_img {
                                        Some(Some(alt_text)) => {
                                            self.output.push('[');
                                            self.output.push_str(alt_text.trim());
                                        }
                                        Some(None) => {
                                            self.output.push('[');
                                            self.output.push_str("img");
                                        }
                                        // No img, no label — skip the link entirely
                                        None => {
                                            self.in_link = was_in_link;
                                            return;
                                        }
                                    }
                                }
                            }
                            // Collapse link text to single line
                            let link_text = self.output[before_len + 1..].to_string();
                            let flat = link_text.split_whitespace().collect::<Vec<_>>().join(" ");
                            self.output.truncate(before_len + 1);
                            self.output.push_str(&flat);

                            if self.compact_links {
                                let idx = self.collected_links.len();
                                self.collected_links.push(Link {
                                    i: idx,
                                    text: flat,
                                    href: resolved.clone(),
                                });
                                let _ = write!(self.output, "][{idx}]");
                            } else {
                                self.output.push_str("](");
                                self.output.push_str(&resolved);
                                self.output.push(')');
                            }

                            // Record URL as seen only after successful emission
                            if !resolved.is_empty() {
                                self.seen_hrefs.insert(resolved);
                            }

                            self.in_link = was_in_link;
                        } else {
                            self.walk_children(node);
                        }
                    }

                    "strong" | "b" => {
                        let before = self.output.len();
                        self.output.push_str("**");
                        self.walk_children(node);
                        let content = &self.output[before + 2..];
                        if content.trim().is_empty() {
                            // Don't emit markers for empty bold (e.g. icon fonts)
                            self.output.truncate(before);
                        } else {
                            self.output.push_str("**");
                        }
                    }

                    "em" | "i" => {
                        let before = self.output.len();
                        self.output.push('*');
                        self.walk_children(node);
                        let content = &self.output[before + 1..];
                        if content.trim().is_empty() {
                            // Don't emit markers for empty italic (e.g. icon fonts)
                            self.output.truncate(before);
                        } else {
                            self.output.push('*');
                        }
                    }

                    "code" => {
                        if self.in_pre {
                            self.walk_children(node);
                        } else {
                            self.output.push('`');
                            self.walk_children(node);
                            self.output.push('`');
                        }
                    }

                    "pre" => {
                        self.in_pre = true;
                        ensure_newline(&mut self.output);
                        self.output.push_str("```\n");
                        self.walk_children(node);
                        ensure_newline(&mut self.output);
                        self.output.push_str("```\n");
                        self.in_pre = false;
                    }

                    "blockquote" => {
                        ensure_newline(&mut self.output);
                        // Render children to a separate buffer, then prefix each line
                        let mut inner = MarkdownWriter::with_options(
                            self.strip_nav,
                            self.include_links,
                            self.include_images,
                        );
                        inner.in_pre = self.in_pre;
                        inner.in_link = self.in_link;
                        inner.base_url = self.base_url.clone();
                        inner.walk_children(node);
                        let inner_text = inner.output;
                        for line in inner_text.lines() {
                            self.output.push_str("> ");
                            self.output.push_str(line);
                            self.output.push('\n');
                        }
                        if !inner_text.ends_with('\n') && !inner_text.is_empty() {
                            self.output.push('\n');
                        }
                    }

                    "ul" => {
                        self.list_stack.push(ListContext {
                            ordered: false,
                            counter: 0,
                        });
                        ensure_newline(&mut self.output);
                        self.walk_children(node);
                        self.list_stack.pop();
                        ensure_newline(&mut self.output);
                    }

                    "ol" => {
                        self.list_stack.push(ListContext {
                            ordered: true,
                            counter: 0,
                        });
                        ensure_newline(&mut self.output);
                        self.walk_children(node);
                        self.list_stack.pop();
                        ensure_newline(&mut self.output);
                    }

                    "li" => {
                        let before = self.output.len();
                        ensure_newline(&mut self.output);
                        let depth = self.list_stack.len().saturating_sub(1);
                        for _ in 0..depth {
                            self.output.push_str("  ");
                        }
                        if let Some(ctx) = self.list_stack.last_mut() {
                            if ctx.ordered {
                                ctx.counter += 1;
                                let _ = write!(self.output, "{}. ", ctx.counter);
                            } else {
                                self.output.push_str("- ");
                            }
                        } else {
                            self.output.push_str("- ");
                        }
                        let marker_end = self.output.len();
                        self.walk_children(node);
                        // Remove empty list items (e.g. icon-only <li>)
                        let content = &self.output[marker_end..];
                        if content.trim().is_empty() {
                            self.output.truncate(before);
                        }
                    }

                    "img" => {
                        if self.include_images {
                            let alt = el.attr("alt").unwrap_or_default();
                            let src = el.attr("src").unwrap_or_default();
                            if !src.is_empty() {
                                let resolved = self.resolve_href(src);
                                let _ = write!(self.output, "![{alt}]({resolved})");
                            }
                        }
                    }

                    "table" => {
                        ensure_newline(&mut self.output);
                        self.write_table(node);
                        self.output.push('\n');
                    }

                    // Skip table sub-elements at top level -- handled inside write_table
                    "thead" | "tbody" | "tfoot" | "tr" | "th" | "td" | "caption" | "colgroup"
                    | "col" => {
                        self.walk_children(node);
                    }

                    _ => {
                        self.walk_children(node);
                    }
                }
            }
            Node::Document => {
                self.walk_children(node);
            }
            _ => {}
        }
    }

    fn walk_children(&mut self, node: NodeRef<'_, Node>) {
        for child in node.children() {
            self.walk(child);
        }
    }

    fn write_table(&mut self, table_node: NodeRef<'_, Node>) {
        let rows = collect_table_rows(table_node);
        if rows.is_empty() {
            return;
        }

        let col_count = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
        if col_count == 0 {
            return;
        }

        let thead_row_count = rows.iter().filter(|r| r.is_header).count();
        let separator_after = if thead_row_count > 0 {
            thead_row_count
        } else {
            1
        };

        for (i, row) in rows.iter().enumerate() {
            self.output.push('|');
            for col in 0..col_count {
                let cell = row.cells.get(col).map(|s| s.as_str()).unwrap_or("");
                let _ = write!(self.output, " {cell} |");
            }
            self.output.push('\n');

            if i + 1 == separator_after {
                self.output.push('|');
                for _ in 0..col_count {
                    self.output.push_str("---|");
                }
                self.output.push('\n');
            }
        }
    }

    fn finish(self) -> String {
        let decoded = decode_entities(&self.output);
        normalize_whitespace(&decoded)
    }
}

struct TableRow {
    cells: Vec<String>,
    is_header: bool,
}

fn collect_table_rows(node: NodeRef<'_, Node>) -> Vec<TableRow> {
    let mut rows = Vec::new();
    collect_rows_recursive(node, &mut rows, false);
    rows
}

fn collect_rows_recursive(node: NodeRef<'_, Node>, rows: &mut Vec<TableRow>, in_thead: bool) {
    for child in node.children() {
        let child_node: &Node = child.value();
        if let Node::Element(el) = child_node {
            let tag = el.name.local.as_ref();
            match tag {
                "thead" => collect_rows_recursive(child, rows, true),
                "tbody" | "tfoot" => collect_rows_recursive(child, rows, false),
                "tr" => {
                    let mut cells = Vec::new();
                    let mut is_header = in_thead;
                    for cell_node in child.children() {
                        let cell_val: &Node = cell_node.value();
                        if let Node::Element(cell_el) = cell_val {
                            let cell_tag = cell_el.name.local.as_ref();
                            if cell_tag == "th" || cell_tag == "td" {
                                if cell_tag == "th" {
                                    is_header = true;
                                }
                                cells.push(collect_cell_text(cell_node));
                            }
                        }
                    }
                    rows.push(TableRow { cells, is_header });
                }
                _ => collect_rows_recursive(child, rows, in_thead),
            }
        }
    }
}

fn collect_cell_text(cell_node: NodeRef<'_, Node>) -> String {
    let mut text = String::new();
    for n in cell_node.descendants() {
        if let Node::Text(t) = n.value() {
            text.push_str(t.text.as_ref());
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// --- Utility functions (ported from diana86) ---

pub(crate) fn collapse_inline_whitespace(s: &str) -> String {
    let mut out = String::new();
    let mut prev_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_ws && !out.is_empty() {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            prev_ws = false;
            out.push(ch);
        }
    }
    out
}

/// Reject control characters and Unicode bidi overrides that could
/// craft visually misleading output or aid prompt injection.
fn is_dangerous_char(c: char) -> bool {
    matches!(c,
        '\u{0000}'..='\u{0008}'
        | '\u{000B}'..='\u{000C}'
        | '\u{000E}'..='\u{001F}'
        | '\u{007F}'
        | '\u{0080}'..='\u{009F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}'
    )
}

/// Decode common HTML entities. Strips dangerous characters.
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();

    while let Some((i, ch)) = chars.next() {
        if ch == '&' {
            if let Some(semi_offset) = s[i..].find(';') {
                let entity = &s[i + 1..i + semi_offset];
                let decoded = match entity {
                    "amp" => Some("&"),
                    "lt" => Some("<"),
                    "gt" => Some(">"),
                    "quot" => Some("\""),
                    "nbsp" => Some(" "),
                    "apos" => Some("'"),
                    _ => None,
                };

                if let Some(d) = decoded {
                    out.push_str(d);
                    let end = i + semi_offset + 1;
                    while chars.peek().is_some_and(|(idx, _)| *idx < end) {
                        chars.next();
                    }
                    continue;
                }

                if let Some(num_str) = entity.strip_prefix('#') {
                    let code = if let Some(hex) = num_str.strip_prefix('x') {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        num_str.parse::<u32>().ok()
                    };
                    if let Some(c) = code.and_then(char::from_u32) {
                        if !is_dangerous_char(c) {
                            out.push(c);
                        }
                        let end = i + semi_offset + 1;
                        while chars.peek().is_some_and(|(idx, _)| *idx < end) {
                            chars.next();
                        }
                        continue;
                    }
                }
            }
            out.push('&');
        } else {
            out.push(ch);
        }
    }

    out
}

/// Collapse 3+ consecutive newlines to 2, trim trailing whitespace on lines.
fn normalize_whitespace(s: &str) -> String {
    let mut out = String::new();
    let mut blank_count = 0;
    let mut started = false;

    for line in s.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            if started && blank_count < 1 {
                out.push('\n');
            }
            blank_count += 1;
        } else {
            started = true;
            blank_count = 0;
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(trimmed);
            out.push('\n');
        }
    }

    let trimmed_len = out.trim_end_matches('\n').len();
    out.truncate(trimmed_len);
    out
}

fn ensure_newline(s: &mut String) {
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
}

fn ensure_double_newline(s: &mut String) {
    ensure_newline(s);
    if !s.ends_with("\n\n") {
        s.push('\n');
    }
}

/// Snap a byte offset to the nearest valid UTF-8 char boundary at or before `pos`.
fn floor_char_boundary(s: &str, pos: usize) -> usize {
    let pos = pos.min(s.len());
    let mut i = pos;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub(crate) fn truncate_markdown(
    markdown: &str,
    offset: u64,
    max_tokens: Option<u64>,
) -> ConvertResult {
    if offset == 0 && max_tokens.is_none() {
        let tokens = estimate_tokens(markdown.len());
        return ConvertResult {
            markdown: markdown.to_string(),
            tokens_est: tokens,
            ..Default::default()
        };
    }

    let offset_bytes = floor_char_boundary(markdown, (offset as usize) * 4);

    // Snap offset to next line boundary
    let start = if offset_bytes >= markdown.len() {
        return ConvertResult::default();
    } else if offset_bytes == 0 {
        0
    } else {
        markdown[offset_bytes..]
            .find('\n')
            .map(|p| offset_bytes + p + 1)
            .unwrap_or(markdown.len())
    };

    let remaining = &markdown[start..];

    if let Some(max) = max_tokens {
        let max_bytes = floor_char_boundary(remaining, (max as usize) * 4);
        if max_bytes >= remaining.len() {
            let tokens = estimate_tokens(remaining.len());
            return ConvertResult {
                markdown: remaining.to_string(),
                tokens_est: tokens,
                ..Default::default()
            };
        }

        // Snap end to previous line boundary
        let end = remaining[..max_bytes]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(max_bytes);

        let tokens = estimate_tokens(end);
        ConvertResult {
            markdown: remaining[..end].to_string(),
            tokens_est: tokens,
            truncated: true,
            next_offset: Some(offset + tokens),
            ..Default::default()
        }
    } else {
        let tokens = estimate_tokens(remaining.len());
        ConvertResult {
            markdown: remaining.to_string(),
            tokens_est: tokens,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Title extraction ---

    #[test]
    fn test_title_extraction() {
        let p = ParsedHtml::parse("<html><head><title>Hello</title></head></html>");
        assert_eq!(p.title(), Some("Hello".to_string()));
    }

    #[test]
    fn test_title_missing() {
        let p = ParsedHtml::parse("<html><head></head><body>Hi</body></html>");
        assert_eq!(p.title(), None);
    }

    #[test]
    fn test_title_whitespace() {
        let p = ParsedHtml::parse("<title>  Hello World  </title>");
        assert_eq!(p.title(), Some("Hello World".to_string()));
    }

    // --- Metadata extraction ---

    #[test]
    fn test_metadata_lang() {
        let p = ParsedHtml::parse(r#"<html lang="en"><body></body></html>"#);
        assert_eq!(p.metadata().lang, Some("en".to_string()));
    }

    #[test]
    fn test_metadata_description() {
        let p = ParsedHtml::parse(
            r#"<html><head><meta name="description" content="A page"></head></html>"#,
        );
        assert_eq!(p.metadata().description, Some("A page".to_string()));
    }

    #[test]
    fn test_metadata_content_type() {
        let p = ParsedHtml::parse(
            r#"<html><head><meta http-equiv="content-type" content="text/html; charset=utf-8"></head></html>"#,
        );
        assert_eq!(
            p.metadata().content_type,
            Some("text/html; charset=utf-8".to_string())
        );
    }

    #[test]
    fn test_metadata_charset() {
        let p = ParsedHtml::parse(r#"<html><head><meta charset="utf-8"></head></html>"#);
        assert_eq!(
            p.metadata().content_type,
            Some("text/html; charset=utf-8".to_string())
        );
    }

    #[test]
    fn test_metadata_missing() {
        let p = ParsedHtml::parse("<html><body></body></html>");
        let meta = p.metadata();
        assert_eq!(meta.lang, None);
        assert_eq!(meta.description, None);
        assert_eq!(meta.content_type, None);
    }

    // --- Link extraction ---

    #[test]
    fn test_links_basic() {
        let p = ParsedHtml::parse(r#"<a href="https://example.com">Example</a>"#);
        let links = p.links(None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].i, 0);
        assert_eq!(links[0].text, "Example");
        assert_eq!(links[0].href, "https://example.com");
    }

    #[test]
    fn test_links_nested_elements() {
        let p = ParsedHtml::parse(r#"<a href="/page"><span>Click</span> here</a>"#);
        let links = p.links(None);
        assert_eq!(links[0].text, "Click here");
    }

    #[test]
    fn test_links_no_href_skipped() {
        let p = ParsedHtml::parse(r#"<a name="anchor">Not a link</a>"#);
        assert!(p.links(None).is_empty());
    }

    #[test]
    fn test_links_empty() {
        let p = ParsedHtml::parse("<p>No links here</p>");
        assert!(p.links(None).is_empty());
    }

    #[test]
    fn test_links_multiple() {
        let p = ParsedHtml::parse(r#"<a href="/a">A</a><a href="/b">B</a><a href="/c">C</a>"#);
        let links = p.links(None);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].i, 0);
        assert_eq!(links[1].i, 1);
        assert_eq!(links[2].i, 2);
    }

    // --- Markdown conversion ---

    #[test]
    fn test_basic_heading() {
        let p = ParsedHtml::parse("<h1>Title</h1>");
        assert_eq!(p.to_markdown(), "# Title");
    }

    #[test]
    fn test_heading_levels() {
        for level in 1..=6 {
            let html = format!("<h{level}>Heading</h{level}>");
            let p = ParsedHtml::parse(&html);
            let prefix = "#".repeat(level);
            assert_eq!(p.to_markdown(), format!("{prefix} Heading"));
        }
    }

    #[test]
    fn test_paragraph() {
        let p = ParsedHtml::parse("<p>Hello world</p><p>Second para</p>");
        let md = p.to_markdown();
        assert!(md.contains("Hello world"));
        assert!(md.contains("Second para"));
        // Should have blank line between paragraphs
        assert!(md.contains("Hello world\n\nSecond para"));
    }

    #[test]
    fn test_link_markdown() {
        let p = ParsedHtml::parse(r#"<a href="https://example.com">Click here</a>"#);
        assert_eq!(p.to_markdown(), "[Click here](https://example.com)");
    }

    #[test]
    fn test_bold() {
        let p = ParsedHtml::parse("<strong>bold</strong>");
        assert_eq!(p.to_markdown(), "**bold**");
    }

    #[test]
    fn test_italic() {
        let p = ParsedHtml::parse("<em>italic</em>");
        assert_eq!(p.to_markdown(), "*italic*");
    }

    #[test]
    fn test_inline_code() {
        let p = ParsedHtml::parse("<code>x = 1</code>");
        assert_eq!(p.to_markdown(), "`x = 1`");
    }

    #[test]
    fn test_code_block() {
        let p = ParsedHtml::parse("<pre><code>fn main() {}</code></pre>");
        let md = p.to_markdown();
        assert!(md.contains("```\nfn main() {}\n```"));
    }

    #[test]
    fn test_unordered_list() {
        let p = ParsedHtml::parse("<ul><li>One</li><li>Two</li></ul>");
        let md = p.to_markdown();
        assert!(md.contains("- One"));
        assert!(md.contains("- Two"));
    }

    #[test]
    fn test_ordered_list() {
        let p = ParsedHtml::parse("<ol><li>First</li><li>Second</li></ol>");
        let md = p.to_markdown();
        assert!(md.contains("1. First"));
        assert!(md.contains("2. Second"));
    }

    #[test]
    fn test_nested_list() {
        let p = ParsedHtml::parse("<ul><li>A<ul><li>A1</li><li>A2</li></ul></li><li>B</li></ul>");
        let md = p.to_markdown();
        assert!(md.contains("- A"));
        assert!(md.contains("  - A1"));
        assert!(md.contains("  - A2"));
        assert!(md.contains("- B"));
    }

    #[test]
    fn test_blockquote() {
        let p = ParsedHtml::parse("<blockquote>Quote text</blockquote>");
        let md = p.to_markdown();
        assert!(md.contains("> Quote text"));
    }

    #[test]
    fn test_image() {
        let p = ParsedHtml::parse(r#"<img alt="logo" src="logo.png">"#);
        assert_eq!(p.to_markdown(), "![logo](logo.png)");
    }

    #[test]
    fn test_hr() {
        let p = ParsedHtml::parse("<p>Above</p><hr><p>Below</p>");
        let md = p.to_markdown();
        assert!(md.contains("---"));
    }

    #[test]
    fn test_br() {
        let p = ParsedHtml::parse("Line1<br>Line2");
        let md = p.to_markdown();
        assert!(md.contains("Line1\nLine2"));
    }

    #[test]
    fn test_table_basic() {
        let html =
            "<table><tr><th>Name</th><th>Age</th></tr><tr><td>Alice</td><td>30</td></tr></table>";
        let p = ParsedHtml::parse(html);
        let md = p.to_markdown();
        assert!(md.contains("| Name | Age |"));
        assert!(md.contains("|---|---|"));
        assert!(md.contains("| Alice | 30 |"));
    }

    #[test]
    fn test_table_with_thead() {
        let html = "<table><thead><tr><th>H1</th><th>H2</th></tr></thead><tbody><tr><td>A</td><td>B</td></tr></tbody></table>";
        let p = ParsedHtml::parse(html);
        let md = p.to_markdown();
        assert!(md.contains("| H1 | H2 |"));
        assert!(md.contains("|---|---|"));
        assert!(md.contains("| A | B |"));
    }

    #[test]
    fn test_strip_script() {
        let p = ParsedHtml::parse("<p>Hello</p><script>alert('xss');</script><p>World</p>");
        let md = p.to_markdown();
        assert!(!md.contains("alert"));
        assert!(md.contains("Hello"));
        assert!(md.contains("World"));
    }

    #[test]
    fn test_strip_style() {
        let p = ParsedHtml::parse("<style>body { color: red; }</style><p>Content</p>");
        let md = p.to_markdown();
        assert!(!md.contains("color"));
        assert!(md.contains("Content"));
    }

    #[test]
    fn test_unknown_tags_transparent() {
        let p = ParsedHtml::parse("<span>inner text</span>");
        assert_eq!(p.to_markdown(), "inner text");
    }

    #[test]
    fn test_nested_formatting() {
        let p = ParsedHtml::parse("<strong><em>bold italic</em></strong>");
        assert_eq!(p.to_markdown(), "***bold italic***");
    }

    // --- Icon font / empty formatting ---

    #[test]
    fn test_empty_italic_icon_font() {
        // Font Awesome style: <i class="fa fa-icon"></i> with no text
        let p = ParsedHtml::parse(r#"<i class="fa fa-facebook"></i>"#);
        assert_eq!(p.to_markdown(), "");
    }

    #[test]
    fn test_empty_bold_suppressed() {
        let p = ParsedHtml::parse("<strong></strong>");
        assert_eq!(p.to_markdown(), "");
    }

    #[test]
    fn test_link_with_icon_font_skipped() {
        // <a> containing only an empty <i> — no meaningful text, no img, no label
        let p = ParsedHtml::parse(
            r#"<a href="https://facebook.com"><i class="fa fa-facebook"></i></a>"#,
        );
        assert_eq!(p.to_markdown(), "");
    }

    #[test]
    fn test_link_with_aria_label() {
        let p = ParsedHtml::parse(
            r#"<a href="https://facebook.com" aria-label="Facebook"><i class="fa fa-facebook"></i></a>"#,
        );
        assert_eq!(p.to_markdown(), "[Facebook](https://facebook.com)");
    }

    #[test]
    fn test_link_with_title_fallback() {
        let p = ParsedHtml::parse(
            r#"<a href="https://twitter.com" title="Twitter"><i class="icon"></i></a>"#,
        );
        assert_eq!(p.to_markdown(), "[Twitter](https://twitter.com)");
    }

    #[test]
    fn test_links_extraction_aria_label() {
        let p = ParsedHtml::parse(
            r#"<a href="https://fb.com" aria-label="Facebook"><i class="fa"></i></a>"#,
        );
        let links = p.links(None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].text, "Facebook");
    }

    // --- Link dedup & heading-in-link ---

    #[test]
    fn test_duplicate_link_dedup() {
        let p = ParsedHtml::parse(
            r#"<a href="https://example.com">First mention</a> <a href="https://example.com">Second mention</a>"#,
        );
        let md = p.to_markdown();
        assert!(md.contains("[First mention](https://example.com)"));
        assert!(!md.contains("Second mention"));
    }

    #[test]
    fn test_heading_inside_link_no_markers() {
        let p = ParsedHtml::parse(r#"<a href="https://example.com"><h3>Article Title</h3></a>"#);
        let md = p.to_markdown();
        assert_eq!(md, "[Article Title](https://example.com)");
        assert!(!md.contains("###"));
    }

    #[test]
    fn test_different_urls_not_deduped() {
        let p = ParsedHtml::parse(r#"<a href="/a">Link A</a> <a href="/b">Link B</a>"#);
        let md = p.to_markdown();
        assert!(md.contains("[Link A](/a)"));
        assert!(md.contains("[Link B](/b)"));
    }

    #[test]
    fn test_empty_link_not_counted_as_seen() {
        // Empty link (no text, no img, no label) should not mark href as seen
        let p = ParsedHtml::parse(
            r#"<a href="https://example.com"><i class="icon"></i></a><a href="https://example.com">Real text</a>"#,
        );
        let md = p.to_markdown();
        assert!(md.contains("[Real text](https://example.com)"));
    }

    // --- Entity decoding (ported from diana86) ---

    #[test]
    fn test_decode_named_entities() {
        assert_eq!(decode_entities("&amp;"), "&");
        assert_eq!(decode_entities("&lt;"), "<");
        assert_eq!(decode_entities("&gt;"), ">");
        assert_eq!(decode_entities("&quot;"), "\"");
        assert_eq!(decode_entities("&nbsp;"), " ");
        assert_eq!(decode_entities("&apos;"), "'");
    }

    #[test]
    fn test_decode_numeric_decimal() {
        assert_eq!(decode_entities("&#65;"), "A");
    }

    #[test]
    fn test_decode_numeric_hex() {
        assert_eq!(decode_entities("&#x41;"), "A");
    }

    #[test]
    fn test_null_byte_stripped() {
        assert_eq!(decode_entities("before&#0;after"), "beforeafter");
    }

    #[test]
    fn test_bidi_override_stripped() {
        assert_eq!(decode_entities("test&#8238;evil"), "testevil");
    }

    #[test]
    fn test_safe_numeric_entity_kept() {
        assert_eq!(decode_entities("caf&#233;"), "café");
    }

    // --- Integration ---

    #[test]
    fn test_full_page_parse() {
        let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="description" content="Test page">
    <title>Test Page Title</title>
    <style>body { margin: 0; }</style>
    <script>console.log('hi');</script>
</head>
<body>
    <nav><a href="/">Home</a></nav>
    <article>
        <h1>Main Heading</h1>
        <p>First paragraph with <strong>bold</strong> and <em>italic</em>.</p>
        <p>A <a href="https://example.com">link</a> in text.</p>
        <ul>
            <li>Item one</li>
            <li>Item two</li>
        </ul>
        <pre><code>let x = 42;</code></pre>
    </article>
</body>
</html>"#;

        let p = ParsedHtml::parse(html);

        // Title
        assert_eq!(p.title(), Some("Test Page Title".to_string()));

        // Metadata
        let meta = p.metadata();
        assert_eq!(meta.lang, Some("en".to_string()));
        assert_eq!(meta.description, Some("Test page".to_string()));
        assert_eq!(
            meta.content_type,
            Some("text/html; charset=utf-8".to_string())
        );

        // Links
        let links = p.links(None);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].href, "/");
        assert_eq!(links[1].href, "https://example.com");

        // Markdown
        let md = p.to_markdown();
        assert!(md.contains("# Main Heading"));
        assert!(md.contains("**bold**"));
        assert!(md.contains("*italic*"));
        assert!(md.contains("[link](https://example.com)"));
        assert!(md.contains("- Item one"));
        assert!(md.contains("- Item two"));
        assert!(md.contains("```\nlet x = 42;\n```"));
        // No script/style content
        assert!(!md.contains("console.log"));
        assert!(!md.contains("margin"));
    }

    // --- Step 6: Markdown converter with options ---

    fn page_html() -> &'static str {
        r#"<!DOCTYPE html>
<html lang="en">
<head><title>Test</title></head>
<body>
    <nav><a href="/">Home</a> | <a href="/about">About</a></nav>
    <header><h1>Site Header</h1></header>
    <article>
        <h2>Article Title</h2>
        <p>Content with a <a href="https://example.com">link</a> and <img alt="pic" src="pic.png">.</p>
    </article>
    <footer><p>Copyright 2026</p></footer>
</body>
</html>"#
    }

    #[test]
    fn test_strip_nav_true() {
        let p = ParsedHtml::parse(page_html());
        let opts = FetchOptions {
            strip_nav: true,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(!result.markdown.contains("Home"));
        assert!(!result.markdown.contains("About"));
        assert!(!result.markdown.contains("Site Header"));
        assert!(!result.markdown.contains("Copyright"));
        assert!(result.markdown.contains("Article Title"));
        assert!(result.markdown.contains("Content"));
    }

    #[test]
    fn test_strip_nav_false() {
        let p = ParsedHtml::parse(page_html());
        let opts = FetchOptions {
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(result.markdown.contains("Home"));
        assert!(result.markdown.contains("Site Header"));
        assert!(result.markdown.contains("Copyright"));
        assert!(result.markdown.contains("Article Title"));
    }

    #[test]
    fn test_css_selector() {
        let p = ParsedHtml::parse(page_html());
        let opts = FetchOptions {
            selector: Some("article".to_string()),
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(result.markdown.contains("Article Title"));
        assert!(result.markdown.contains("Content"));
        assert!(!result.markdown.contains("Home"));
        assert!(!result.markdown.contains("Copyright"));
    }

    #[test]
    fn test_css_selector_no_match() {
        let p = ParsedHtml::parse(page_html());
        let opts = FetchOptions {
            selector: Some(".nonexistent".to_string()),
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(result.markdown.is_empty());
    }

    #[test]
    fn test_include_links_false() {
        let p = ParsedHtml::parse(page_html());
        let opts = FetchOptions {
            include_links: false,
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        // Link text present but no markdown link syntax
        assert!(result.markdown.contains("Home"));
        assert!(!result.markdown.contains("[Home]"));
        assert!(!result.markdown.contains("]("));
    }

    #[test]
    fn test_include_links_true() {
        let p = ParsedHtml::parse(page_html());
        let opts = FetchOptions {
            include_links: true,
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(result.markdown.contains("[link](https://example.com)"));
    }

    #[test]
    fn test_include_images_false() {
        let p = ParsedHtml::parse(page_html());
        let opts = FetchOptions {
            include_images: false,
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(!result.markdown.contains("![pic]"));
        assert!(!result.markdown.contains("pic.png"));
    }

    #[test]
    fn test_include_images_true() {
        let p = ParsedHtml::parse(page_html());
        let opts = FetchOptions {
            include_images: true,
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(result.markdown.contains("![pic](pic.png)"));
    }

    #[test]
    fn test_truncation_basic() {
        // Build content that's at least 800 chars (200 tokens)
        let mut lines = Vec::new();
        for i in 0..40 {
            lines.push(format!("Line {i}: some padding text here abcdef"));
        }
        let html = format!("<p>{}</p>", lines.join("<br>"));
        let p = ParsedHtml::parse(&html);
        let opts = FetchOptions {
            max_tokens: Some(50),
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(result.truncated);
        assert!(result.next_offset.is_some());
        assert!(result.tokens_est <= 50);
    }

    #[test]
    fn test_no_truncation_needed() {
        let p = ParsedHtml::parse("<p>Short content</p>");
        let opts = FetchOptions {
            max_tokens: Some(10000),
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(!result.truncated);
        assert_eq!(result.next_offset, None);
        assert!(result.markdown.contains("Short content"));
    }

    #[test]
    fn test_offset_pagination() {
        let mut lines = Vec::new();
        for i in 0..40 {
            lines.push(format!("Line {i}: some padding text here abcdef"));
        }
        let html = format!("<p>{}</p>", lines.join("<br>"));
        let p = ParsedHtml::parse(&html);

        // First page
        let opts1 = FetchOptions {
            max_tokens: Some(50),
            offset: 0,
            strip_nav: false,
            ..Default::default()
        };
        let r1 = p.to_markdown_with_options(&opts1, None);
        assert!(r1.truncated);
        let next = r1.next_offset.unwrap();

        // Second page using next_offset
        let opts2 = FetchOptions {
            max_tokens: Some(50),
            offset: next,
            strip_nav: false,
            ..Default::default()
        };
        let r2 = p.to_markdown_with_options(&opts2, None);
        // Second page should have different content
        assert_ne!(r1.markdown, r2.markdown);
    }

    #[test]
    fn test_offset_beyond_content() {
        let p = ParsedHtml::parse("<p>Short</p>");
        let opts = FetchOptions {
            offset: 99999,
            strip_nav: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(result.markdown.is_empty());
        assert!(!result.truncated);
    }

    #[test]
    fn test_default_options_match_to_markdown() {
        let p = ParsedHtml::parse(page_html());
        let plain = p.to_markdown();
        let opts = FetchOptions {
            strip_nav: false,
            include_images: true,
            ..Default::default()
        };
        let with_opts = p.to_markdown_with_options(&opts, None);
        assert_eq!(plain, with_opts.markdown);
    }

    #[test]
    fn test_full_options_integration() {
        let html = r#"<!DOCTYPE html>
<html>
<body>
    <nav><a href="/">Nav Link</a></nav>
    <article>
        <h1>Title</h1>
        <p>Paragraph with <a href="/page">a link</a> and <img alt="img" src="i.png">.</p>
    </article>
    <footer>Footer text</footer>
</body>
</html>"#;
        let p = ParsedHtml::parse(html);
        let opts = FetchOptions {
            selector: Some("article".to_string()),
            strip_nav: true,
            include_links: false,
            include_images: false,
            ..Default::default()
        };
        let result = p.to_markdown_with_options(&opts, None);
        assert!(result.markdown.contains("# Title"));
        assert!(result.markdown.contains("a link"));
        assert!(!result.markdown.contains("[a link]"));
        assert!(!result.markdown.contains("![img]"));
        assert!(!result.markdown.contains("Nav Link"));
        assert!(!result.markdown.contains("Footer text"));
        assert!(!result.truncated);
    }
}
