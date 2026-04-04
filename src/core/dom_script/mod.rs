//! JavaScript-based DOM query execution using boa_engine.
//!
//! Bridges scraper's parsed HTML into a JS DOM environment with:
//! - DOM traversal (parentElement, children, siblings, closest, matches)
//! - DOM lookup (getElementById, getElementsByClassName, etc.)
//! - DOM mutation (createElement, appendChild, setAttribute, innerHTML setter)
//! - Event stubs (addEventListener, dispatchEvent, Event constructors)
//! - Web API shims (localStorage, document.cookie, console, setTimeout, atob/btoa)

mod convert;
mod document;
mod element;
mod events;
mod window;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use boa_engine::{Context, Source};
use scraper::Html;

use self::convert::js_value_to_json;
use self::document::{register_document, register_local_storage};
use self::element::ElementCtx;
use self::events::{EventStore, register_event_constructors};
use self::window::{
    register_base64, register_console, register_get_computed_style, register_mutation_observer,
    register_timers, register_window,
};
use crate::core::dom_query::QueryError;
use crate::core::http_client::CookieJar;
use crate::core::page::{DomScriptResult, PendingNavigation};

fn js_err(e: impl ToString) -> QueryError {
    QueryError::ScriptError(e.to_string())
}

/// Session state passed into the JS context for Web API shims.
pub struct ScriptContext {
    /// Current origin's localStorage entries (cloned in, merged back out).
    pub storage: HashMap<String, String>,
    /// Current page origin (scheme://host[:port]).
    #[allow(dead_code)]
    pub origin: String,
    /// Cookie jar (shared with HTTP client).
    pub cookie_jar: Arc<CookieJar>,
    /// Current page URL (for cookie domain resolution).
    pub current_url: String,
    /// Filled form fields overlay: CSS selector → value.
    pub filled_fields: HashMap<String, String>,
}

/// Results returned alongside the script result when a ScriptContext was provided.
pub struct ScriptSideEffects {
    /// Updated localStorage entries for the current origin.
    pub storage: HashMap<String, String>,
    /// Updated filled form fields.
    pub filled_fields: HashMap<String, String>,
    /// If the DOM was mutated, the serialized HTML after mutation.
    pub mutated_html: Option<String>,
}

/// Execute a JavaScript script against a parsed HTML document.
///
/// Creates a sandboxed JS context with a full DOM environment that bridges
/// to the HTML parsed by `scraper`. Returns the script's result value
/// converted to JSON.
pub fn execute_script(
    html: &str,
    script: &str,
    ctx: Option<ScriptContext>,
) -> Result<(DomScriptResult, Option<ScriptSideEffects>), QueryError> {
    let start = Instant::now();
    let doc = Rc::new(RefCell::new(Html::parse_document(html)));
    let mut context = Context::default();

    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(1_000_000);
    context.runtime_limits_mut().set_recursion_limit(256);

    let storage_cell = Rc::new(RefCell::new(
        ctx.as_ref().map(|c| c.storage.clone()).unwrap_or_default(),
    ));
    let filled_cell = Rc::new(RefCell::new(
        ctx.as_ref()
            .map(|c| c.filled_fields.clone())
            .unwrap_or_default(),
    ));
    let pending_nav: Rc<RefCell<Option<PendingNavigation>>> = Rc::new(RefCell::new(None));
    let dom_mutated = Rc::new(Cell::new(false));
    let event_store = Rc::new(RefCell::new(EventStore::new()));
    let console_output = Rc::new(RefCell::new(Vec::new()));

    let ectx = ElementCtx {
        doc: Rc::clone(&doc),
        filled_fields: Rc::clone(&filled_cell),
        pending_nav: Rc::clone(&pending_nav),
        dom_mutated: Rc::clone(&dom_mutated),
        event_store: Rc::clone(&event_store),
    };

    // Register localStorage (only with context)
    if ctx.is_some() {
        register_local_storage(&mut context, &storage_cell).map_err(js_err)?;
    }

    // Register document global
    register_document(&mut context, &ectx, ctx.as_ref()).map_err(js_err)?;

    // Register window/location
    register_window(&mut context, &pending_nav).map_err(js_err)?;

    // Register console
    register_console(&mut context, &console_output).map_err(js_err)?;

    // Register event constructors
    register_event_constructors(&mut context).map_err(js_err)?;

    // Register timers
    register_timers(&mut context).map_err(js_err)?;

    // Register base64
    register_base64(&mut context).map_err(js_err)?;

    // Register MutationObserver
    register_mutation_observer(&mut context).map_err(js_err)?;

    // Register getComputedStyle
    register_get_computed_style(&mut context).map_err(js_err)?;

    // Execute the script
    let result = context
        .eval(Source::from_bytes(script.as_bytes()))
        .map_err(js_err)?;

    let (json_val, type_str) =
        js_value_to_json(&result, &mut context).map_err(QueryError::ScriptError)?;

    let pending = pending_nav.borrow().clone();
    let console = console_output.borrow().clone();

    // Serialize mutated HTML if DOM was changed
    let mutated_html = if dom_mutated.get() {
        let doc = doc.borrow();
        Some(doc.html())
    } else {
        None
    };

    let side_effects = ctx.map(|_| ScriptSideEffects {
        storage: storage_cell.borrow().clone(),
        filled_fields: filled_cell.borrow().clone(),
        mutated_html,
    });

    Ok((
        DomScriptResult {
            result: json_val,
            result_type: type_str,
            exec_ms: start.elapsed().as_millis() as u64,
            pending_navigation: pending,
            console_output: if console.is_empty() {
                None
            } else {
                Some(console)
            },
        },
        side_effects,
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
        <div id="content" data-page="home" data-user-name="alice">Content here</div>
    </body>
    </html>
    "#;

    /// Helper: run script without context, return DomScriptResult.
    fn run(html: &str, script: &str) -> Result<DomScriptResult, QueryError> {
        execute_script(html, script, None).map(|(r, _)| r)
    }

    // -----------------------------------------------------------------------
    // Original tests (must all pass)
    // -----------------------------------------------------------------------

    #[test]
    fn test_document_title() {
        let result = run(TEST_HTML, "document.title").unwrap();
        assert_eq!(result.result, serde_json::json!("Test Page"));
        assert_eq!(result.result_type, "string");
    }

    #[test]
    fn test_query_selector_all_length() {
        let result = run(TEST_HTML, "document.querySelectorAll('a').length").unwrap();
        assert_eq!(result.result, serde_json::json!(3));
        assert_eq!(result.result_type, "number");
    }

    #[test]
    fn test_query_selector_text_content() {
        let result = run(TEST_HTML, "document.querySelector('h1').textContent").unwrap();
        assert_eq!(result.result, serde_json::json!("Main Heading"));
        assert_eq!(result.result_type, "string");
    }

    #[test]
    fn test_query_selector_get_attribute() {
        let result = run(
            TEST_HTML,
            "document.querySelector('a').getAttribute('href')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("https://example.com"));
        assert_eq!(result.result_type, "string");
    }

    #[test]
    fn test_query_selector_href_property() {
        let result = run(TEST_HTML, "document.querySelector('a').href").unwrap();
        assert_eq!(result.result, serde_json::json!("https://example.com"));
        assert_eq!(result.result_type, "string");
    }

    #[test]
    fn test_query_selector_inner_html() {
        let result = run(TEST_HTML, "document.querySelector('p.intro').innerHTML").unwrap();
        assert_eq!(
            result.result,
            serde_json::json!("Hello <strong>world</strong>")
        );
        assert_eq!(result.result_type, "string");
    }

    #[test]
    fn test_query_selector_returns_null_when_not_found() {
        let result = run(TEST_HTML, "document.querySelector('table')").unwrap();
        assert_eq!(result.result, serde_json::Value::Null);
        assert_eq!(result.result_type, "null");
    }

    #[test]
    fn test_element_id_and_classname() {
        let result = run(TEST_HTML, "document.querySelector('#content').id").unwrap();
        assert_eq!(result.result, serde_json::json!("content"));
    }

    #[test]
    fn test_element_tag_name() {
        let result = run(TEST_HTML, "document.querySelector('h1').tagName").unwrap();
        assert_eq!(result.result, serde_json::json!("H1"));
    }

    #[test]
    fn test_scoped_query() {
        let result = run(
            TEST_HTML,
            "document.querySelector('body').querySelectorAll('a').length",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(3));
    }

    #[test]
    fn test_complex_script_collect_hrefs() {
        let result = run(
            TEST_HTML,
            r#"
            var links = document.querySelectorAll('a');
            var hrefs = [];
            for (var i = 0; i < links.length; i++) {
                hrefs.push(links[i].href);
            }
            hrefs
            "#,
        )
        .unwrap();
        assert_eq!(
            result.result,
            serde_json::json!(["https://example.com", "/about", "/contact"])
        );
        assert_eq!(result.result_type, "array");
    }

    #[test]
    fn test_script_error_returns_query_error() {
        let err = run(TEST_HTML, "undefined.property").unwrap_err();
        assert!(matches!(err, QueryError::ScriptError(_)));
    }

    #[test]
    fn test_invalid_selector_in_script() {
        let err = run(TEST_HTML, "document.querySelector('[[[invalid')").unwrap_err();
        assert!(matches!(err, QueryError::ScriptError(_)));
    }

    #[test]
    fn test_exec_ms_populated() {
        let result = run(TEST_HTML, "document.title").unwrap();
        assert!(result.exec_ms < 5000);
    }

    #[test]
    fn test_numeric_result() {
        let result = run(TEST_HTML, "1 + 2").unwrap();
        assert_eq!(result.result, serde_json::json!(3));
        assert_eq!(result.result_type, "number");
    }

    #[test]
    fn test_boolean_result() {
        let result = run(TEST_HTML, "true").unwrap();
        assert_eq!(result.result, serde_json::json!(true));
        assert_eq!(result.result_type, "boolean");
    }

    #[test]
    fn test_null_result() {
        let result = run(TEST_HTML, "null").unwrap();
        assert_eq!(result.result, serde_json::Value::Null);
        assert_eq!(result.result_type, "null");
    }

    #[test]
    fn test_get_attribute_missing_returns_null() {
        let result = run(
            TEST_HTML,
            "document.querySelector('h1').getAttribute('href')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::Value::Null);
        assert_eq!(result.result_type, "null");
    }

    // -----------------------------------------------------------------------
    // Web API shim tests
    // -----------------------------------------------------------------------

    const FORM_HTML: &str = r#"
    <!DOCTYPE html>
    <html>
    <head><title>Form Page</title></head>
    <body>
        <form id="login" action="/login" method="POST">
            <input type="text" id="user" name="username" value="default_user">
            <input type="password" name="password" value="">
            <textarea name="notes">initial notes</textarea>
            <select name="role">
                <option value="user">User</option>
                <option value="admin" selected>Admin</option>
            </select>
            <button type="submit">Log In</button>
        </form>
        <a href="/about" id="about-link">About</a>
    </body>
    </html>
    "#;

    fn test_ctx() -> ScriptContext {
        ScriptContext {
            storage: HashMap::new(),
            origin: "https://example.com".into(),
            cookie_jar: Arc::new(CookieJar::new()),
            current_url: "https://example.com/page".into(),
            filled_fields: HashMap::new(),
        }
    }

    fn run_with_ctx(
        html: &str,
        script: &str,
        ctx: ScriptContext,
    ) -> (DomScriptResult, ScriptSideEffects) {
        let (result, effects) = execute_script(html, script, Some(ctx)).unwrap();
        (result, effects.unwrap())
    }

    // --- localStorage tests ---

    #[test]
    fn test_localstorage_set_and_get() {
        let (result, effects) = run_with_ctx(
            TEST_HTML,
            "localStorage.setItem('k', 'v'); localStorage.getItem('k')",
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::json!("v"));
        assert_eq!(effects.storage.get("k").unwrap(), "v");
    }

    #[test]
    fn test_localstorage_get_missing_returns_null() {
        let (result, _) = run_with_ctx(TEST_HTML, "localStorage.getItem('missing')", test_ctx());
        assert_eq!(result.result, serde_json::Value::Null);
    }

    #[test]
    fn test_localstorage_remove() {
        let mut ctx = test_ctx();
        ctx.storage.insert("k".into(), "v".into());
        let (_, effects) = run_with_ctx(
            TEST_HTML,
            "localStorage.removeItem('k'); localStorage.getItem('k')",
            ctx,
        );
        assert!(!effects.storage.contains_key("k"));
    }

    #[test]
    fn test_localstorage_clear() {
        let mut ctx = test_ctx();
        ctx.storage.insert("a".into(), "1".into());
        ctx.storage.insert("b".into(), "2".into());
        let (_, effects) = run_with_ctx(TEST_HTML, "localStorage.clear()", ctx);
        assert!(effects.storage.is_empty());
    }

    #[test]
    fn test_localstorage_preloaded_value() {
        let mut ctx = test_ctx();
        ctx.storage.insert("theme".into(), "dark".into());
        let (result, _) = run_with_ctx(TEST_HTML, "localStorage.getItem('theme')", ctx);
        assert_eq!(result.result, serde_json::json!("dark"));
    }

    // --- document.cookie tests ---

    #[test]
    fn test_document_cookie_empty() {
        let (result, _) = run_with_ctx(TEST_HTML, "document.cookie", test_ctx());
        assert_eq!(result.result, serde_json::json!(""));
    }

    #[test]
    fn test_document_cookie_set_and_read() {
        let ctx = test_ctx();
        let (result, _) = run_with_ctx(
            TEST_HTML,
            "document.cookie = 'foo=bar; Domain=example.com; Path=/'; document.cookie",
            ctx,
        );
        let s = result.result.as_str().unwrap();
        assert!(s.contains("foo=bar"), "expected 'foo=bar' in '{s}'");
    }

    #[test]
    fn test_document_cookie_preloaded() {
        let ctx = test_ctx();
        let url: reqwest::Url = "https://example.com/".parse().unwrap();
        ctx.cookie_jar
            .add_cookie_str("session=abc123; Domain=example.com; Path=/", &url);

        let (result, _) = run_with_ctx(TEST_HTML, "document.cookie", ctx);
        let s = result.result.as_str().unwrap();
        assert!(s.contains("session=abc123"));
    }

    // --- element.value tests ---

    #[test]
    fn test_element_value_getter_default() {
        let (result, _) = run_with_ctx(
            FORM_HTML,
            "document.querySelector('#user').value",
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::json!("default_user"));
    }

    #[test]
    fn test_element_value_setter() {
        let (result, effects) = run_with_ctx(
            FORM_HTML,
            "document.querySelector('#user').value = 'new_user'; document.querySelector('#user').value",
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::json!("new_user"));
        assert!(effects.filled_fields.values().any(|v| v == "new_user"));
    }

    #[test]
    fn test_element_value_textarea() {
        let (result, _) = run_with_ctx(
            FORM_HTML,
            "document.querySelector('textarea[name=\"notes\"]').value",
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::json!("initial notes"));
    }

    #[test]
    fn test_element_value_setter_updates_filled_fields() {
        let (_, effects) = run_with_ctx(
            FORM_HTML,
            "document.querySelector('input[name=\"password\"]').value = 's3cret'",
            test_ctx(),
        );
        assert!(effects.filled_fields.values().any(|v| v == "s3cret"));
    }

    // --- element.click() tests ---

    #[test]
    fn test_click_link_sets_pending_navigation() {
        let (result, _) = run_with_ctx(
            FORM_HTML,
            "document.querySelector('#about-link').click()",
            test_ctx(),
        );
        assert_eq!(
            result.pending_navigation,
            Some(PendingNavigation::Link {
                href: "/about".into()
            })
        );
    }

    #[test]
    fn test_click_submit_button_sets_form_submit() {
        let (result, _) = run_with_ctx(
            FORM_HTML,
            "document.querySelector('button[type=\"submit\"]').click()",
            test_ctx(),
        );
        assert_eq!(
            result.pending_navigation,
            Some(PendingNavigation::FormSubmit {
                selector: "form#login".into()
            })
        );
    }

    #[test]
    fn test_click_non_interactive_is_noop() {
        let (result, _) = run_with_ctx(
            TEST_HTML,
            "document.querySelector('h1').click()",
            test_ctx(),
        );
        assert_eq!(result.pending_navigation, None);
    }

    // --- document.body / document.head / document.documentElement ---

    #[test]
    fn test_document_body_text_content() {
        let result = run(TEST_HTML, "document.body.textContent").unwrap();
        let text = result.result.as_str().unwrap();
        assert!(text.contains("Main Heading"));
        assert!(text.contains("Content here"));
    }

    #[test]
    fn test_document_body_inner_html() {
        let result = run(TEST_HTML, "document.body.innerHTML").unwrap();
        let html = result.result.as_str().unwrap();
        assert!(html.contains("<h1>Main Heading</h1>"));
        assert!(html.contains("<a href=\"https://example.com\">Example</a>"));
    }

    #[test]
    fn test_document_body_outer_html() {
        let result = run(TEST_HTML, "document.body.outerHTML").unwrap();
        let html = result.result.as_str().unwrap();
        assert!(html.starts_with("<body>"));
        assert!(html.contains("Main Heading"));
    }

    #[test]
    fn test_document_head_inner_html() {
        let result = run(TEST_HTML, "document.head.innerHTML").unwrap();
        let html = result.result.as_str().unwrap();
        assert!(html.contains("<title>Test Page</title>"));
    }

    #[test]
    fn test_document_document_element_tag_name() {
        let result = run(TEST_HTML, "document.documentElement.tagName").unwrap();
        assert_eq!(result.result, serde_json::json!("HTML"));
    }

    #[test]
    fn test_document_document_element_outer_html() {
        let result = run(TEST_HTML, "document.documentElement.outerHTML").unwrap();
        let html = result.result.as_str().unwrap();
        assert!(html.starts_with("<html>"));
        assert!(html.contains("<title>Test Page</title>"));
        assert!(html.contains("Main Heading"));
    }

    #[test]
    fn test_document_body_query_selector() {
        let result = run(TEST_HTML, "document.body.querySelector('h1').textContent").unwrap();
        assert_eq!(result.result, serde_json::json!("Main Heading"));
    }

    #[test]
    fn test_element_outer_html() {
        let result = run(TEST_HTML, "document.querySelector('p.intro').outerHTML").unwrap();
        assert_eq!(
            result.result,
            serde_json::json!("<p class=\"intro\">Hello <strong>world</strong></p>")
        );
    }

    #[test]
    fn test_form_submit_sets_pending() {
        let (result, _) = run_with_ctx(
            FORM_HTML,
            "document.querySelector('form#login').submit()",
            test_ctx(),
        );
        assert_eq!(
            result.pending_navigation,
            Some(PendingNavigation::FormSubmit {
                selector: "form#login".into()
            })
        );
    }

    // -----------------------------------------------------------------------
    // Phase 1: DOM traversal tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parent_element() {
        let result = run(
            TEST_HTML,
            "document.querySelector('h1').parentElement.tagName",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("BODY"));
    }

    #[test]
    fn test_children_length() {
        let result = run(TEST_HTML, "document.querySelector('body').children.length").unwrap();
        // body has: h1, h2, p, a, a, a, img, div = 8 elements
        let count = result.result.as_i64().unwrap();
        assert!(count >= 7, "expected at least 7 children, got {count}");
    }

    #[test]
    fn test_first_element_child() {
        let result = run(
            TEST_HTML,
            "document.querySelector('body').firstElementChild.tagName",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("H1"));
    }

    #[test]
    fn test_last_element_child() {
        let result = run(
            TEST_HTML,
            "document.querySelector('body').lastElementChild.tagName",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("DIV"));
    }

    #[test]
    fn test_next_element_sibling() {
        let result = run(
            TEST_HTML,
            "document.querySelector('h1').nextElementSibling.tagName",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("H2"));
    }

    #[test]
    fn test_previous_element_sibling() {
        let result = run(
            TEST_HTML,
            "document.querySelector('h2').previousElementSibling.tagName",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("H1"));
    }

    #[test]
    fn test_child_element_count() {
        let result = run(
            TEST_HTML,
            "document.querySelector('p.intro').childElementCount",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(1)); // <strong>
    }

    #[test]
    fn test_matches() {
        let result = run(TEST_HTML, "document.querySelector('h1').matches('h1')").unwrap();
        assert_eq!(result.result, serde_json::json!(true));
    }

    #[test]
    fn test_matches_false() {
        let result = run(TEST_HTML, "document.querySelector('h1').matches('h2')").unwrap();
        assert_eq!(result.result, serde_json::json!(false));
    }

    #[test]
    fn test_closest() {
        let result = run(
            TEST_HTML,
            "document.querySelector('strong').closest('p').className",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("intro"));
    }

    #[test]
    fn test_has_attribute() {
        let result = run(
            TEST_HTML,
            "document.querySelector('#content').hasAttribute('data-page')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(true));
    }

    #[test]
    fn test_has_attribute_false() {
        let result = run(
            TEST_HTML,
            "document.querySelector('#content').hasAttribute('data-missing')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(false));
    }

    // --- getElementById ---

    #[test]
    fn test_get_element_by_id() {
        let result = run(TEST_HTML, "document.getElementById('content').textContent").unwrap();
        assert_eq!(result.result, serde_json::json!("Content here"));
    }

    #[test]
    fn test_get_element_by_id_missing() {
        let result = run(TEST_HTML, "document.getElementById('nonexistent')").unwrap();
        assert_eq!(result.result, serde_json::Value::Null);
    }

    // --- getElementsByClassName ---

    #[test]
    fn test_get_elements_by_class_name() {
        let result = run(TEST_HTML, "document.getElementsByClassName('intro').length").unwrap();
        assert_eq!(result.result, serde_json::json!(1));
    }

    // --- getElementsByTagName ---

    #[test]
    fn test_get_elements_by_tag_name() {
        let result = run(TEST_HTML, "document.getElementsByTagName('a').length").unwrap();
        assert_eq!(result.result, serde_json::json!(3));
    }

    // --- classList ---

    #[test]
    fn test_class_list_contains() {
        let result = run(
            TEST_HTML,
            "document.querySelector('p').classList.contains('intro')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(true));
    }

    #[test]
    fn test_class_list_contains_false() {
        let result = run(
            TEST_HTML,
            "document.querySelector('p').classList.contains('missing')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(false));
    }

    #[test]
    fn test_class_list_length() {
        let result = run(TEST_HTML, "document.querySelector('p').classList.length").unwrap();
        assert_eq!(result.result, serde_json::json!(1));
    }

    // --- dataset ---

    #[test]
    fn test_dataset() {
        let result = run(TEST_HTML, "document.querySelector('#content').dataset.page").unwrap();
        assert_eq!(result.result, serde_json::json!("home"));
    }

    #[test]
    fn test_dataset_camel_case() {
        let result = run(
            TEST_HTML,
            "document.querySelector('#content').dataset.userName",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("alice"));
    }

    // --- node properties ---

    #[test]
    fn test_node_type() {
        let result = run(TEST_HTML, "document.querySelector('h1').nodeType").unwrap();
        assert_eq!(result.result, serde_json::json!(1));
    }

    #[test]
    fn test_node_name() {
        let result = run(TEST_HTML, "document.querySelector('h1').nodeName").unwrap();
        assert_eq!(result.result, serde_json::json!("H1"));
    }

    // -----------------------------------------------------------------------
    // Phase 2: DOM mutation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_create_element() {
        let result = run(TEST_HTML, "document.createElement('div').tagName").unwrap();
        assert_eq!(result.result, serde_json::json!("DIV"));
    }

    #[test]
    fn test_set_attribute() {
        let (result, _) = run_with_ctx(
            TEST_HTML,
            r#"
            var el = document.querySelector('h1');
            el.setAttribute('class', 'main-title');
            el.getAttribute('class')
            "#,
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::json!("main-title"));
    }

    #[test]
    fn test_remove_attribute() {
        let (result, _) = run_with_ctx(
            TEST_HTML,
            r#"
            var el = document.querySelector('#content');
            el.removeAttribute('data-page');
            el.getAttribute('data-page')
            "#,
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::Value::Null);
    }

    #[test]
    fn test_append_child() {
        let (result, _) = run_with_ctx(
            TEST_HTML,
            r#"
            var parent = document.querySelector('body');
            var child = document.createElement('span');
            parent.appendChild(child);
            parent.lastElementChild.tagName
            "#,
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::json!("SPAN"));
    }

    #[test]
    fn test_remove_child() {
        let (result, _) = run_with_ctx(
            TEST_HTML,
            r#"
            var body = document.querySelector('body');
            var h1 = document.querySelector('h1');
            body.removeChild(h1);
            document.querySelector('h1')
            "#,
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::Value::Null);
        assert_eq!(result.result_type, "null");
    }

    #[test]
    fn test_text_content_setter() {
        let (result, _) = run_with_ctx(
            TEST_HTML,
            r#"
            var el = document.querySelector('#content');
            el.textContent = 'New text';
            el.textContent
            "#,
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::json!("New text"));
    }

    #[test]
    fn test_inner_html_setter() {
        let (result, _) = run_with_ctx(
            TEST_HTML,
            r#"
            var el = document.querySelector('#content');
            el.innerHTML = '<b>Bold</b>';
            el.innerHTML
            "#,
            test_ctx(),
        );
        let html = result.result.as_str().unwrap();
        assert!(html.contains("<b>Bold</b>"), "got: {html}");
    }

    #[test]
    fn test_mutated_html_in_side_effects() {
        let (_, effects) = run_with_ctx(
            TEST_HTML,
            "document.querySelector('h1').setAttribute('class', 'modified')",
            test_ctx(),
        );
        assert!(effects.mutated_html.is_some());
        let html = effects.mutated_html.unwrap();
        assert!(html.contains("modified"));
    }

    // -----------------------------------------------------------------------
    // Phase 3: Event stubs and utilities tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_event_constructor() {
        let result = run(TEST_HTML, "new Event('click').type").unwrap();
        assert_eq!(result.result, serde_json::json!("click"));
    }

    #[test]
    fn test_custom_event_detail() {
        let result = run(TEST_HTML, "new CustomEvent('test', { detail: 42 }).detail").unwrap();
        assert_eq!(result.result, serde_json::json!(42));
    }

    #[test]
    fn test_add_and_dispatch_event() {
        let result = run(
            TEST_HTML,
            r#"
            var result = 0;
            var el = document.querySelector('h1');
            el.addEventListener('click', function(e) { result = 1; });
            el.dispatchEvent(new Event('click'));
            result
            "#,
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(1));
    }

    #[test]
    fn test_console_log() {
        let result = run(TEST_HTML, "console.log('hello', 'world'); 42").unwrap();
        assert_eq!(result.result, serde_json::json!(42));
        let output = result.console_output.unwrap();
        assert_eq!(output.len(), 1);
        assert!(output[0].contains("hello world"));
    }

    #[test]
    fn test_set_timeout_sync() {
        let result = run(
            TEST_HTML,
            r#"
            var x = 0;
            setTimeout(function() { x = 42; }, 1000);
            x
            "#,
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(42));
    }

    #[test]
    fn test_btoa_atob() {
        let result = run(TEST_HTML, "atob(btoa('hello'))").unwrap();
        assert_eq!(result.result, serde_json::json!("hello"));
    }

    #[test]
    fn test_mutation_observer_noop() {
        let result = run(
            TEST_HTML,
            r#"
            var mo = new MutationObserver(function() {});
            mo.observe(document.body, { childList: true });
            mo.disconnect();
            'ok'
            "#,
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("ok"));
    }

    #[test]
    fn test_get_computed_style_stub() {
        let result = run(
            TEST_HTML,
            "getComputedStyle(document.body).getPropertyValue('color')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(""));
    }

    #[test]
    fn test_request_animation_frame() {
        let result = run(
            TEST_HTML,
            r#"
            var x = 0;
            requestAnimationFrame(function() { x = 1; });
            x
            "#,
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(1));
    }

    #[test]
    fn test_document_add_event_listener() {
        let result = run(
            TEST_HTML,
            r#"
            var result = 0;
            document.addEventListener('custom', function(e) { result = 99; });
            document.dispatchEvent(new Event('custom'));
            result
            "#,
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(99));
    }
}
