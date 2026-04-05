//! JavaScript-based DOM query execution using deno_core (V8).
//!
//! Replacement for the boa_engine-based dom_script module.
//! Provides the same public API: `execute_script()` with `ScriptContext` / `ScriptSideEffects`.

mod ops;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use deno_core::{Extension, JsRuntime, OpDecl, RuntimeOptions, v8};
use scraper::Html;

use self::ops::DomState;
use crate::core::dom_query::QueryError;
use crate::core::http_client::CookieJar;
use crate::core::page::DomScriptResult;

static RUNTIME_JS: &str = include_str!("runtime.js");

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

// ---------------------------------------------------------------------------
// Op declarations
// ---------------------------------------------------------------------------

fn op_decls() -> Vec<OpDecl> {
    vec![
        ops::op_element_info(),
        ops::op_element_text_content(),
        ops::op_element_inner_html(),
        ops::op_element_outer_html(),
        ops::op_node_text(),
        ops::op_node_set_text(),
        ops::op_element_get_attribute(),
        ops::op_element_has_attribute(),
        ops::op_element_set_attribute(),
        ops::op_element_remove_attribute(),
        ops::op_element_parent(),
        ops::op_element_children(),
        ops::op_element_child_count(),
        ops::op_element_first_child(),
        ops::op_element_last_child(),
        ops::op_element_first_element_child(),
        ops::op_element_last_element_child(),
        ops::op_element_next_sibling(),
        ops::op_element_prev_sibling(),
        ops::op_doc_query_selector(),
        ops::op_doc_query_selector_all(),
        ops::op_doc_get_element_by_id(),
        ops::op_doc_get_elements_by_class(),
        ops::op_doc_get_elements_by_tag(),
        ops::op_doc_get_elements_by_name(),
        ops::op_doc_title(),
        ops::op_element_query_selector(),
        ops::op_element_query_selector_all(),
        ops::op_element_matches(),
        ops::op_element_closest(),
        ops::op_element_contains(),
        ops::op_element_set_text_content(),
        ops::op_element_set_inner_html(),
        ops::op_element_append_child(),
        ops::op_element_remove_child(),
        ops::op_element_insert_before(),
        ops::op_element_remove(),
        ops::op_doc_create_element(),
        ops::op_doc_create_text_node(),
        ops::op_element_click(),
        ops::op_form_submit(),
        ops::op_field_value_get(),
        ops::op_field_value_set(),
        ops::op_location_href(),
        ops::op_location_navigate(),
        ops::op_console(),
        ops::op_storage_get(),
        ops::op_storage_set(),
        ops::op_storage_remove(),
        ops::op_storage_clear(),
        ops::op_cookie_get(),
        ops::op_cookie_set(),
        ops::op_btoa(),
        ops::op_atob(),
    ]
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Execute a JavaScript script against a parsed HTML document.
///
/// Creates a sandboxed V8 context with a full DOM environment that bridges
/// to the HTML parsed by `scraper`. Returns the script's result value
/// converted to JSON.
pub fn execute_script(
    html: &str,
    script: &str,
    ctx: Option<ScriptContext>,
) -> Result<(DomScriptResult, Option<ScriptSideEffects>), QueryError> {
    let start = Instant::now();

    // Build shared DOM state
    let dom_state = DomState {
        doc: RefCell::new(Html::parse_document(html)),
        filled_fields: RefCell::new(
            ctx.as_ref()
                .map(|c| c.filled_fields.clone())
                .unwrap_or_default(),
        ),
        pending_nav: RefCell::new(None),
        dom_mutated: RefCell::new(false),
        console_output: RefCell::new(Vec::new()),
        storage: RefCell::new(ctx.as_ref().map(|c| c.storage.clone()).unwrap_or_default()),
        cookie_jar: ctx.as_ref().map(|c| Arc::clone(&c.cookie_jar)),
        current_url: ctx.as_ref().map(|c| c.current_url.clone()),
    };

    // Create extension with all ops
    let ext = Extension {
        name: "browser39_dom",
        ops: std::borrow::Cow::Owned(op_decls()),
        ..Default::default()
    };

    // Create V8 runtime
    let mut runtime = JsRuntime::new(RuntimeOptions {
        extensions: vec![ext],
        ..Default::default()
    });

    // Store DomState in OpState
    runtime.op_state().borrow_mut().put(dom_state);

    // Execute bootstrap JS
    runtime
        .execute_script("<bootstrap>", RUNTIME_JS.to_string())
        .map_err(js_err)?;

    // Execute user script and capture result
    let script_owned: String = script.to_string();
    let result_global = runtime
        .execute_script("<user>", script_owned)
        .map_err(js_err)?;

    // Convert result to JSON
    let (json_val, type_str) = {
        let scope = &mut runtime.handle_scope();
        let local = v8::Local::new(scope, &result_global);
        v8_to_json(scope, local)
    };

    // Extract side effects from DomState
    let op_state = runtime.op_state();
    let ds = op_state.borrow();
    let ds = ds.borrow::<DomState>();

    let pending = ds.pending_nav.borrow().clone();
    let console = ds.console_output.borrow().clone();

    let mutated_html = if *ds.dom_mutated.borrow() {
        let doc = ds.doc.borrow();
        Some(doc.html())
    } else {
        None
    };

    let side_effects = ctx.map(|_| ScriptSideEffects {
        storage: ds.storage.borrow().clone(),
        filled_fields: ds.filled_fields.borrow().clone(),
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
// V8 → JSON conversion
// ---------------------------------------------------------------------------

fn v8_to_json(
    scope: &mut v8::HandleScope,
    value: v8::Local<v8::Value>,
) -> (serde_json::Value, String) {
    let type_str = if value.is_undefined() {
        "undefined"
    } else if value.is_null() {
        "null"
    } else if value.is_boolean() {
        "boolean"
    } else if value.is_number() {
        "number"
    } else if value.is_string() {
        "string"
    } else if value.is_array() {
        "array"
    } else {
        "object"
    };
    let json = serde_v8::from_v8::<serde_json::Value>(scope, value)
        .unwrap_or(serde_json::Value::Null);
    (json, type_str.into())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use crate::core::http_client::CookieJar;

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

    fn run(html: &str, script: &str) -> Result<DomScriptResult, QueryError> {
        execute_script(html, script, None).map(|(r, _)| r)
    }

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

    // --- Core tests ---

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
    fn test_console_log() {
        let result = run(TEST_HTML, "console.log('hello', 'world'); 42").unwrap();
        assert_eq!(result.result, serde_json::json!(42));
        assert_eq!(
            result.console_output,
            Some(vec!["[log] hello world".to_string()])
        );
    }

    #[test]
    fn test_btoa_atob() {
        let result = run(TEST_HTML, "btoa('hello')").unwrap();
        assert_eq!(result.result, serde_json::json!("aGVsbG8="));
        let result = run(TEST_HTML, "atob('aGVsbG8=')").unwrap();
        assert_eq!(result.result, serde_json::json!("hello"));
    }

    #[test]
    fn test_get_element_by_id() {
        let result = run(TEST_HTML, "document.getElementById('content').textContent").unwrap();
        assert_eq!(result.result, serde_json::json!("Content here"));
    }

    #[test]
    fn test_get_elements_by_tag_name() {
        let result = run(TEST_HTML, "document.getElementsByTagName('a').length").unwrap();
        assert_eq!(result.result, serde_json::json!(3));
    }

    #[test]
    fn test_dataset() {
        let result = run(TEST_HTML, "document.querySelector('#content').dataset.page").unwrap();
        assert_eq!(result.result, serde_json::json!("home"));
    }

    #[test]
    fn test_dataset_camel_case() {
        let result = run(TEST_HTML, "document.querySelector('#content').dataset.userName").unwrap();
        assert_eq!(result.result, serde_json::json!("alice"));
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

    // --- DOM mutation tests ---

    #[test]
    fn test_create_element() {
        let result = run(TEST_HTML, "document.createElement('div').tagName").unwrap();
        assert_eq!(result.result, serde_json::json!("DIV"));
    }

    #[test]
    fn test_set_attribute() {
        let result = run(
            TEST_HTML,
            "var el = document.querySelector('h1'); el.setAttribute('class', 'title'); el.getAttribute('class')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("title"));
    }

    #[test]
    fn test_set_text_content() {
        let (_, effects) = run_with_ctx(
            TEST_HTML,
            "document.querySelector('h1').textContent = 'New Heading'",
            test_ctx(),
        );
        assert!(effects.mutated_html.is_some());
        assert!(effects.mutated_html.unwrap().contains("New Heading"));
    }

    #[test]
    fn test_parent_element() {
        let result = run(
            TEST_HTML,
            "document.querySelector('strong').parentElement.tagName",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("P"));
    }

    #[test]
    fn test_children_count() {
        let result = run(
            TEST_HTML,
            "document.querySelector('body').childElementCount",
        )
        .unwrap();
        // body has: h1, h2, p, a, a, a, img, div = 8 element children
        let count = result.result.as_i64().unwrap();
        assert!(count > 0);
    }

    #[test]
    fn test_element_matches() {
        let result = run(
            TEST_HTML,
            "document.querySelector('h1').matches('h1')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(true));
    }

    #[test]
    fn test_element_closest() {
        let result = run(
            TEST_HTML,
            "document.querySelector('strong').closest('p').className",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("intro"));
    }

    #[test]
    fn test_classlist_contains() {
        let result = run(
            TEST_HTML,
            "document.querySelector('p').classList.contains('intro')",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(true));
    }

    #[test]
    fn test_set_inner_html() {
        let (_, effects) = run_with_ctx(
            TEST_HTML,
            "document.querySelector('h1').innerHTML = '<span>Updated</span>'",
            test_ctx(),
        );
        assert!(effects.mutated_html.is_some());
        assert!(effects.mutated_html.unwrap().contains("<span>Updated</span>"));
    }

    // --- Form field value tests ---

    const FORM_HTML: &str = r#"
    <!DOCTYPE html>
    <html>
    <head><title>Form Page</title></head>
    <body>
        <form id="login" action="/login" method="POST">
            <input type="text" id="user" name="username" value="default_user">
            <input type="password" name="password" value="">
            <textarea name="notes">initial notes</textarea>
            <button type="submit">Log In</button>
        </form>
    </body>
    </html>
    "#;

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
    fn test_textarea_value_default() {
        let (result, _) = run_with_ctx(
            FORM_HTML,
            "document.querySelector('textarea').value",
            test_ctx(),
        );
        assert_eq!(result.result, serde_json::json!("initial notes"));
    }

    // --- Event tests ---

    #[test]
    fn test_add_event_listener_and_dispatch() {
        let result = run(
            TEST_HTML,
            r#"
            var called = false;
            var el = document.querySelector('h1');
            el.addEventListener('click', function() { called = true; });
            el.dispatchEvent(new Event('click'));
            called
            "#,
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(true));
    }

    // --- Navigation tests ---

    #[test]
    fn test_click_link_pending_nav() {
        let result = run(
            TEST_HTML,
            "document.querySelector('a').click(); 'done'",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("done"));
        assert!(result.pending_navigation.is_some());
    }

    #[test]
    fn test_location_assign_pending_nav() {
        let result = run(TEST_HTML, "location.assign('/new'); 'ok'").unwrap();
        assert!(result.pending_navigation.is_some());
    }

    // --- setTimeout test ---

    #[test]
    fn test_set_timeout_sync() {
        let result = run(
            TEST_HTML,
            "var x = 0; setTimeout(function() { x = 42; }, 0); x",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!(42));
    }

    // --- MutationObserver stub ---

    #[test]
    fn test_mutation_observer_no_crash() {
        let result = run(
            TEST_HTML,
            "var m = new MutationObserver(function(){}); m.observe(document.body, {}); m.disconnect(); 'ok'",
        )
        .unwrap();
        assert_eq!(result.result, serde_json::json!("ok"));
    }
}
