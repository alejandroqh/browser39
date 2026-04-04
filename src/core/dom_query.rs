use scraper::{Html, Selector};
use std::time::Instant;

use super::page::DomSelectorResult;

/// Run a CSS selector query against raw HTML and extract attribute values.
///
/// `attr` controls what is extracted from each matching element:
/// - `"textContent"` — concatenated text nodes
/// - `"innerHTML"` — inner HTML markup
/// - any other string — the named HTML attribute (e.g. `"href"`, `"src"`, `"class"`)
///
/// Returns `DomSelectorResult` with the collected values, count, and elapsed time.
pub fn query_selector(
    html: &str,
    selector: &str,
    attr: &str,
) -> Result<DomSelectorResult, QueryError> {
    let start = Instant::now();

    let compiled =
        Selector::parse(selector).map_err(|_| QueryError::InvalidSelector(selector.to_string()))?;

    let document = Html::parse_document(html);
    let mut results: Vec<serde_json::Value> = Vec::new();

    for element in document.select(&compiled) {
        let value = match attr {
            "textContent" => {
                let text: String = element.text().collect();
                serde_json::Value::String(text)
            }
            "innerHTML" => serde_json::Value::String(element.inner_html()),
            other => match element.value().attr(other) {
                Some(v) => serde_json::Value::String(v.to_string()),
                None => serde_json::Value::Null,
            },
        };
        results.push(value);
    }

    let count = results.len();
    Ok(DomSelectorResult {
        results,
        count,
        exec_ms: start.elapsed().as_millis() as u64,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("invalid CSS selector: {0}")]
    InvalidSelector(String),
    #[error("script execution error: {0}")]
    ScriptError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_HTML: &str = r#"
    <!DOCTYPE html>
    <html>
    <head><title>Test Page</title></head>
    <body>
        <h1>Main Heading</h1>
        <h2>Sub Heading</h2>
        <p class="intro">Hello <strong>world</strong></p>
        <a href="https://example.com">Example</a>
        <a href="/about">About Us</a>
        <a href="/contact">Contact</a>
        <img src="/logo.png" alt="Logo">
        <div id="content" data-page="home">Content here</div>
    </body>
    </html>
    "#;

    #[test]
    fn test_text_content() {
        let result = query_selector(TEST_HTML, "h1", "textContent").unwrap();
        assert_eq!(result.count, 1);
        assert_eq!(result.results[0], "Main Heading");
    }

    #[test]
    fn test_multiple_matches() {
        let result = query_selector(TEST_HTML, "a", "textContent").unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(result.results[0], "Example");
        assert_eq!(result.results[1], "About Us");
        assert_eq!(result.results[2], "Contact");
    }

    #[test]
    fn test_href_attribute() {
        let result = query_selector(TEST_HTML, "a", "href").unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(result.results[0], "https://example.com");
        assert_eq!(result.results[1], "/about");
        assert_eq!(result.results[2], "/contact");
    }

    #[test]
    fn test_src_attribute() {
        let result = query_selector(TEST_HTML, "img", "src").unwrap();
        assert_eq!(result.count, 1);
        assert_eq!(result.results[0], "/logo.png");
    }

    #[test]
    fn test_inner_html() {
        let result = query_selector(TEST_HTML, "p.intro", "innerHTML").unwrap();
        assert_eq!(result.count, 1);
        assert_eq!(result.results[0], "Hello <strong>world</strong>");
    }

    #[test]
    fn test_custom_data_attribute() {
        let result = query_selector(TEST_HTML, "#content", "data-page").unwrap();
        assert_eq!(result.count, 1);
        assert_eq!(result.results[0], "home");
    }

    #[test]
    fn test_missing_attribute_returns_null() {
        let result = query_selector(TEST_HTML, "h1", "href").unwrap();
        assert_eq!(result.count, 1);
        assert!(result.results[0].is_null());
    }

    #[test]
    fn test_no_matches_returns_empty() {
        let result = query_selector(TEST_HTML, "table", "textContent").unwrap();
        assert_eq!(result.count, 0);
        assert!(result.results.is_empty());
    }

    #[test]
    fn test_invalid_selector() {
        let err = query_selector(TEST_HTML, "[[[invalid", "textContent").unwrap_err();
        assert!(matches!(err, QueryError::InvalidSelector(_)));
    }

    #[test]
    fn test_exec_ms_populated() {
        let result = query_selector(TEST_HTML, "h1", "textContent").unwrap();
        // exec_ms should be a valid number (may be 0 for fast operations)
        assert!(result.exec_ms < 1000);
    }
}
