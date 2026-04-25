// Convert RSS 2.0, RSS 1.0 (RDF), and Atom feeds to token-optimized markdown.
//
// Output shape (per item):
//
//   ## [Item Title](item_link)
//   YYYY-MM-DDTHH:MM:SSZ · Category
//
//   Item description text.
//
// Channel/feed-level title becomes the H1, with an optional italic subtitle.

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::fmt::Write as _;

use super::html_to_md::{
    ConvertResult, collapse_inline_whitespace, decode_entities, truncate_markdown,
};
use super::page::{FetchOptions, Link};
use super::url::resolve_href;

/// Recognize MIME types that should be parsed as a feed.
pub fn is_feed_content_type(mime: &str) -> bool {
    let m = mime.to_ascii_lowercase();
    matches!(
        m.as_str(),
        "application/rss+xml"
            | "application/atom+xml"
            | "application/rdf+xml"
            | "application/feed+xml"
    ) || m == "application/xml"
        || m == "text/xml"
        || m.ends_with("+xml")
}

/// Cheap content sniff: does the body look like an RSS/Atom/RDF feed?
/// We check the first ~512 bytes after any XML prolog.
pub fn looks_like_feed(body: &str) -> bool {
    let head = body.trim_start();
    let head = head.strip_prefix("<?xml").map_or(head, |s| {
        s.find("?>").map(|p| &s[p + 2..]).unwrap_or(s).trim_start()
    });
    let probe = &head[..head.len().min(512)];
    probe.contains("<rss")
        || probe.contains("<feed")
        || probe.contains("<rdf:RDF")
        || probe.contains("<RDF")
}

#[derive(Default)]
struct ChannelInfo {
    title: Option<String>,
    description: Option<String>,
    link: Option<String>,
    updated: Option<String>,
}

#[derive(Default)]
struct Item {
    title: Option<String>,
    link: Option<String>,
    description: Option<String>,
    date: Option<String>,
    category: Option<String>,
}

pub struct ParsedFeed {
    channel: ChannelInfo,
    items: Vec<Item>,
}

impl ParsedFeed {
    pub fn parse(body: &str) -> Self {
        let mut reader = Reader::from_str(body);
        let config = reader.config_mut();
        config.trim_text(true);
        config.expand_empty_elements = false;

        let mut channel = ChannelInfo::default();
        let mut items: Vec<Item> = Vec::new();
        let mut current_item: Option<Item> = None;
        // Stack of local tag names so we know our context.
        let mut stack: Vec<String> = Vec::new();
        // Active text-collecting field on the current item or channel.
        let mut text_buf = String::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let local = local_name(e.name().as_ref());
                    handle_attributes(&local, &e, &stack, &mut channel, current_item.as_mut());
                    if local == "item" || local == "entry" {
                        current_item = Some(Item::default());
                    }
                    stack.push(local);
                    text_buf.clear();
                }
                Ok(Event::Empty(e)) => {
                    let local = local_name(e.name().as_ref());
                    handle_attributes(&local, &e, &stack, &mut channel, current_item.as_mut());
                }
                Ok(Event::End(e)) => {
                    let local = local_name(e.name().as_ref());
                    let text = std::mem::take(&mut text_buf);
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        assign_text(&mut channel, current_item.as_mut(), &stack, &local, trimmed);
                    }
                    if local == "item" || local == "entry" {
                        if let Some(item) = current_item.take() {
                            if item.title.is_some() || item.link.is_some() {
                                items.push(item);
                            }
                        }
                    }
                    stack.pop();
                }
                Ok(Event::Text(t)) => {
                    if let Ok(s) = t.unescape() {
                        text_buf.push_str(s.as_ref());
                    }
                }
                Ok(Event::CData(c)) => {
                    if let Ok(s) = std::str::from_utf8(c.as_ref()) {
                        text_buf.push_str(s);
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break, // Best-effort: stop on parse error, return what we have.
                _ => {}
            }
            buf.clear();
        }

        Self { channel, items }
    }

    pub fn title(&self) -> Option<String> {
        self.channel.title.clone()
    }

    /// Channel-level description (RSS) or subtitle (Atom), if present.
    pub fn description(&self) -> Option<String> {
        self.channel.description.clone()
    }

    #[cfg(test)]
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Render the feed as markdown and (optionally) collect item links.
    pub fn to_markdown_with_options(
        &self,
        options: &FetchOptions,
        base_url: Option<&str>,
    ) -> ConvertResult {
        let base = base_url.and_then(|u| reqwest::Url::parse(u).ok());
        let mut out = String::new();
        let mut links: Vec<Link> = Vec::new();

        // Channel header
        if let Some(t) = &self.channel.title {
            let _ = writeln!(out, "# {}", t);
        }
        if let Some(d) = &self.channel.description {
            let d = collapse_inline_whitespace(d);
            let d = d.trim();
            if !d.is_empty() && Some(d) != self.channel.title.as_deref() {
                let _ = writeln!(out, "*{}*", d);
            }
        }
        if let Some(u) = &self.channel.updated {
            let _ = writeln!(out, "*Updated: {}*", u.trim());
        }
        if !out.is_empty() {
            out.push('\n');
        }

        // Items
        for (idx, item) in self.items.iter().enumerate() {
            let title = item
                .title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("(untitled)");
            let resolved_link = item
                .link
                .as_deref()
                .map(|l| resolve_href(base.as_ref(), l.trim()))
                .unwrap_or_default();

            if options.include_links && !resolved_link.is_empty() {
                let _ = writeln!(out, "## [{}]({})", title, resolved_link);
                links.push(Link {
                    i: idx,
                    text: title.to_string(),
                    href: resolved_link.clone(),
                });
            } else {
                let _ = writeln!(out, "## {}", title);
                if !resolved_link.is_empty() {
                    links.push(Link {
                        i: idx,
                        text: title.to_string(),
                        href: resolved_link,
                    });
                }
            }

            // Meta line: date · category
            let mut meta_parts: Vec<String> = Vec::new();
            if let Some(d) = item.date.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                meta_parts.push(d.to_string());
            }
            if let Some(c) = item
                .category
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                meta_parts.push(c.to_string());
            }
            if !meta_parts.is_empty() {
                let _ = writeln!(out, "{}", meta_parts.join(" · "));
            }

            // Description
            if let Some(desc) = item.description.as_deref() {
                let cleaned = clean_description(desc);
                if !cleaned.is_empty() {
                    out.push('\n');
                    let _ = writeln!(out, "{}", cleaned);
                }
            }

            out.push('\n');
        }

        let markdown = out.trim_end().to_string();
        let mut result = truncate_markdown(&markdown, options.offset, options.max_tokens);
        result.links = links;
        result
    }
}

// --- helpers ---

fn local_name(qname: &[u8]) -> String {
    let s = std::str::from_utf8(qname).unwrap_or("");
    let local = s.rsplit(':').next().unwrap_or(s);
    local.to_ascii_lowercase()
}

fn attr_value(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    for a in e.attributes().with_checks(false).flatten() {
        if a.key.as_ref() == key {
            return std::str::from_utf8(&a.value).ok().map(|s| s.to_string());
        }
    }
    None
}

/// Atom links are routed by `rel`. We prefer `alternate` (the default) and
/// skip `self`, `enclosure`, `edit`, etc.
fn atom_link_href(e: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    let mut href = None;
    let mut rel = None;
    for a in e.attributes().with_checks(false).flatten() {
        match a.key.as_ref() {
            b"href" => href = std::str::from_utf8(&a.value).ok().map(|s| s.to_string()),
            b"rel" => rel = std::str::from_utf8(&a.value).ok().map(|s| s.to_string()),
            _ => {}
        }
    }
    let rel = rel.as_deref().unwrap_or("alternate");
    if rel == "alternate" { href } else { None }
}

/// True if we're inside an Atom-style container where `<link>` carries href in
/// an attribute (rather than as text content like RSS).
fn in_atom_context(stack: &[String]) -> bool {
    stack.iter().any(|t| t == "feed" || t == "entry")
}

/// Pull `href` (Atom links) and `term` (Atom categories) out of a start/empty
/// element. Both event types arrive here so RSS feeds with self-closing
/// elements work the same as Atom feeds with explicit close tags.
fn handle_attributes(
    local: &str,
    e: &quick_xml::events::BytesStart<'_>,
    stack: &[String],
    channel: &mut ChannelInfo,
    item: Option<&mut Item>,
) {
    match local {
        "link" if in_atom_context(stack) => {
            if let Some(href) = atom_link_href(e) {
                assign_link(channel, item, stack, href);
            }
        }
        "category" => {
            if let Some(item) = item
                && item.category.is_none()
                && let Some(term) = attr_value(e, b"term")
            {
                item.category = Some(term);
            }
        }
        _ => {}
    }
}

fn assign_link(
    channel: &mut ChannelInfo,
    item: Option<&mut Item>,
    stack: &[String],
    href: String,
) {
    if let Some(item) = item {
        if item.link.is_none() {
            item.link = Some(href);
        }
    } else if stack.iter().any(|t| t == "channel" || t == "feed") && channel.link.is_none() {
        channel.link = Some(href);
    }
}

fn assign_text(
    channel: &mut ChannelInfo,
    item: Option<&mut Item>,
    stack: &[String],
    local: &str,
    text: &str,
) {
    if let Some(item) = item {
        match local {
            "title" if item.title.is_none() => item.title = Some(text.to_string()),
            // RSS uses <link>text</link>; Atom uses <link href="..."/> handled elsewhere.
            "link" if item.link.is_none() => item.link = Some(text.to_string()),
            "description" | "summary" | "content" if item.description.is_none() => {
                item.description = Some(text.to_string());
            }
            "pubdate" | "date" | "published" | "updated" if item.date.is_none() => {
                item.date = Some(text.to_string());
            }
            "category" | "subject" if item.category.is_none() => {
                item.category = Some(text.to_string());
            }
            _ => {}
        }
    } else if stack.iter().any(|t| t == "channel" || t == "feed") {
        match local {
            "title" if channel.title.is_none() => channel.title = Some(text.to_string()),
            "link" if channel.link.is_none() => channel.link = Some(text.to_string()),
            "description" | "subtitle" if channel.description.is_none() => {
                channel.description = Some(text.to_string());
            }
            "updated" | "date" if channel.updated.is_none() => {
                channel.updated = Some(text.to_string());
            }
            _ => {}
        }
    }
}

/// Remove HTML tags from a description, decode entities, collapse whitespace.
/// RSS descriptions often contain HTML inside CDATA — we strip it down to
/// plain text since the description is just a teaser, not the article body.
fn clean_description(s: &str) -> String {
    let stripped = strip_html_tags(s);
    let decoded = decode_entities(&stripped);
    collapse_inline_whitespace(&decoded).trim().to_string()
}

fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_content_types() {
        assert!(is_feed_content_type("application/rss+xml"));
        assert!(is_feed_content_type("application/atom+xml"));
        assert!(is_feed_content_type("application/rdf+xml"));
        assert!(is_feed_content_type("text/xml"));
        assert!(is_feed_content_type("application/xml"));
        assert!(is_feed_content_type("application/some-vendor+xml"));
        assert!(!is_feed_content_type("text/html"));
        assert!(!is_feed_content_type("application/json"));
    }

    #[test]
    fn looks_like_feed_rss() {
        let body = r#"<?xml version="1.0"?><rss version="2.0"><channel></channel></rss>"#;
        assert!(looks_like_feed(body));
    }

    #[test]
    fn looks_like_feed_atom() {
        let body = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"></feed>"#;
        assert!(looks_like_feed(body));
    }

    #[test]
    fn looks_like_feed_rdf() {
        let body = r#"<?xml version="1.0"?><rdf:RDF xmlns:rdf="..."></rdf:RDF>"#;
        assert!(looks_like_feed(body));
    }

    #[test]
    fn looks_like_feed_negative() {
        assert!(!looks_like_feed("<html><body>not a feed</body></html>"));
    }

    #[test]
    fn parse_rss_2_0() {
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Site News</title>
    <description>Latest updates</description>
    <link>https://example.com/</link>
    <item>
      <title>First Post</title>
      <link>https://example.com/1</link>
      <description>Hello world</description>
      <pubDate>Mon, 01 Jan 2026 12:00:00 GMT</pubDate>
      <category>News</category>
    </item>
    <item>
      <title>Second Post</title>
      <link>https://example.com/2</link>
      <description><![CDATA[A summary with <b>bold</b> text]]></description>
    </item>
  </channel>
</rss>"#;
        let feed = ParsedFeed::parse(xml);
        assert_eq!(feed.title().as_deref(), Some("Site News"));
        assert_eq!(feed.item_count(), 2);
        let md = feed
            .to_markdown_with_options(&FetchOptions::default(), None)
            .markdown;
        assert!(md.contains("# Site News"));
        assert!(md.contains("## [First Post](https://example.com/1)"));
        assert!(md.contains("Mon, 01 Jan 2026 12:00:00 GMT · News"));
        assert!(md.contains("Hello world"));
        assert!(md.contains("## [Second Post](https://example.com/2)"));
        assert!(md.contains("A summary with bold text"));
        assert!(!md.contains("<b>"));
    }

    #[test]
    fn parse_rss_1_0_rdf() {
        let xml = r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns="http://purl.org/rss/1.0/"
         xmlns:dc="http://purl.org/dc/elements/1.1/">
  <channel rdf:about="https://example.com/feed">
    <title>RDF Channel</title>
    <description>Top stories</description>
    <link>https://example.com/</link>
  </channel>
  <item rdf:about="https://example.com/a">
    <title>RDF Item One</title>
    <link>https://example.com/a</link>
    <description>Body text</description>
    <dc:date>2026-04-25T10:00:00Z</dc:date>
    <dc:subject>Politics</dc:subject>
  </item>
</rdf:RDF>"#;
        let feed = ParsedFeed::parse(xml);
        assert_eq!(feed.title().as_deref(), Some("RDF Channel"));
        assert_eq!(feed.item_count(), 1);
        let md = feed
            .to_markdown_with_options(&FetchOptions::default(), None)
            .markdown;
        assert!(md.contains("# RDF Channel"));
        assert!(md.contains("## [RDF Item One](https://example.com/a)"));
        assert!(md.contains("2026-04-25T10:00:00Z · Politics"));
        assert!(md.contains("Body text"));
    }

    #[test]
    fn parse_atom() {
        let xml = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>My Atom Feed</title>
  <subtitle>Daily musings</subtitle>
  <link href="https://example.com/" rel="alternate"/>
  <link href="https://example.com/feed.xml" rel="self"/>
  <updated>2026-04-25T10:00:00Z</updated>
  <entry>
    <title>Atom Entry</title>
    <link href="https://example.com/post/1" rel="alternate"/>
    <link href="https://example.com/post/1/edit" rel="edit"/>
    <published>2026-04-24T08:00:00Z</published>
    <summary>An atom summary</summary>
    <category term="Tech"/>
  </entry>
</feed>"#;
        let feed = ParsedFeed::parse(xml);
        assert_eq!(feed.title().as_deref(), Some("My Atom Feed"));
        assert_eq!(feed.item_count(), 1);
        let md = feed
            .to_markdown_with_options(&FetchOptions::default(), None)
            .markdown;
        assert!(md.contains("# My Atom Feed"));
        assert!(md.contains("*Daily musings*"));
        assert!(md.contains("## [Atom Entry](https://example.com/post/1)"));
        // Should pick the alternate link, not the edit link
        assert!(!md.contains("post/1/edit"));
        assert!(md.contains("2026-04-24T08:00:00Z · Tech"));
        assert!(md.contains("An atom summary"));
    }

    #[test]
    fn relative_links_resolve() {
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Rel</title>
    <item>
      <title>Post</title>
      <link>/posts/1</link>
    </item>
  </channel>
</rss>"#;
        let feed = ParsedFeed::parse(xml);
        // resolve_href shortens to path-only when the resolved URL has the same
        // origin as the base — matches the HTML→MD convention used elsewhere.
        let md = feed
            .to_markdown_with_options(&FetchOptions::default(), Some("https://example.com/feed"))
            .markdown;
        assert!(md.contains("[Post](/posts/1)"), "got: {md}");
    }

    #[test]
    fn include_links_false_omits_url() {
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>X</title><item><title>Post</title><link>https://example.com/1</link></item></channel></rss>"#;
        let feed = ParsedFeed::parse(xml);
        let opts = FetchOptions {
            include_links: false,
            ..Default::default()
        };
        let result = feed.to_markdown_with_options(&opts, None);
        assert!(result.markdown.contains("## Post"));
        assert!(!result.markdown.contains("(https://example.com/1)"));
        // Links collection still populated for the caller.
        assert_eq!(result.links.len(), 1);
        assert_eq!(result.links[0].href, "https://example.com/1");
    }

    #[test]
    fn item_with_no_title_or_link_skipped() {
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>X</title><item><description>orphan</description></item></channel></rss>"#;
        let feed = ParsedFeed::parse(xml);
        assert_eq!(feed.item_count(), 0);
    }

    #[test]
    fn malformed_xml_returns_partial() {
        // Truncated mid-item — we should still get whatever items completed.
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>X</title>
<item><title>Done</title><link>https://example.com/d</link></item>
<item><title>Partial</title><link>https://exa"#;
        let feed = ParsedFeed::parse(xml);
        assert_eq!(feed.title().as_deref(), Some("X"));
        assert_eq!(feed.item_count(), 1);
    }
}
