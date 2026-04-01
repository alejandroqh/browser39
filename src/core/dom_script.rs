//! JavaScript-based DOM query execution using boa_engine.
//!
//! Bridges scraper's parsed HTML into a minimal JS DOM environment,
//! exposing `document.title`, `document.querySelector()`, and
//! `document.querySelectorAll()` with element properties.
//!
//! When a `ScriptContext` is provided, also registers Web API shims:
//! `localStorage`, `document.cookie`, `element.value`, `form.submit()`,
//! and `element.click()`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{
    js_string, Context, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, Source,
};
use ego_tree::NodeId;
use scraper::{ElementRef, Html, Selector};

use super::dom_query::QueryError;
use super::html_to_md::{SEL_BODY, SEL_HEAD, SEL_HTML, SEL_TITLE};
use super::http_client::CookieJar;
use super::page::{DomScriptResult, PendingNavigation};

/// Session state passed into the JS context for Web API shims.
pub struct ScriptContext {
    /// Current origin's localStorage entries (cloned in, merged back out).
    pub storage: HashMap<String, String>,
    /// Current page origin (scheme://host[:port]). Used by localStorage shims.
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
}

/// Execute a JavaScript script against a parsed HTML document.
///
/// Creates a sandboxed JS context with a `document` global that bridges
/// to the HTML parsed by `scraper`. Returns the script's result value
/// converted to JSON.
///
/// If `ctx` is provided, also registers `localStorage`, `document.cookie`,
/// `element.value`, `form.submit()`, and `element.click()` shims.
pub fn execute_script(
    html: &str,
    script: &str,
    ctx: Option<ScriptContext>,
) -> Result<(DomScriptResult, Option<ScriptSideEffects>), QueryError> {
    let start = Instant::now();
    let doc = Rc::new(Html::parse_document(html));
    let mut context = Context::default();

    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(1_000_000);
    context.runtime_limits_mut().set_recursion_limit(256);

    let storage_cell = Rc::new(RefCell::new(
        ctx.as_ref().map(|c| c.storage.clone()).unwrap_or_default(),
    ));
    let filled_cell = Rc::new(RefCell::new(
        ctx.as_ref().map(|c| c.filled_fields.clone()).unwrap_or_default(),
    ));
    let pending_nav: Rc<RefCell<Option<PendingNavigation>>> = Rc::new(RefCell::new(None));

    if ctx.is_some() {
        register_local_storage(&mut context, &storage_cell)
            .map_err(|e| QueryError::ScriptError(e.to_string()))?;
    }

    register_document(&mut context, &doc, ctx.as_ref(), &filled_cell, &pending_nav)
        .map_err(|e| QueryError::ScriptError(e.to_string()))?;

    register_window(&mut context, &pending_nav)
        .map_err(|e| QueryError::ScriptError(e.to_string()))?;

    let result = context
        .eval(Source::from_bytes(script.as_bytes()))
        .map_err(|e| QueryError::ScriptError(e.to_string()))?;

    let (json_val, type_str) =
        js_value_to_json(&result, &mut context).map_err(QueryError::ScriptError)?;

    let pending = pending_nav.borrow().clone();

    let side_effects = ctx.map(|_| ScriptSideEffects {
        storage: storage_cell.borrow().clone(),
        filled_fields: filled_cell.borrow().clone(),
    });

    Ok((
        DomScriptResult {
            result: json_val,
            result_type: type_str,
            exec_ms: start.elapsed().as_millis() as u64,
            pending_navigation: pending,
        },
        side_effects,
    ))
}

// ---------------------------------------------------------------------------
// Document global
// ---------------------------------------------------------------------------

fn register_document(
    context: &mut Context,
    doc: &Rc<Html>,
    cookie_ctx: Option<&ScriptContext>,
    filled_fields: &Rc<RefCell<HashMap<String, String>>>,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> JsResult<()> {
    let title = doc
        .select(&SEL_TITLE)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .unwrap_or_default();

    let qs_fn = build_doc_qs(doc, filled_fields, pending_nav);
    let qsa_fn = build_doc_qsa(doc, filled_fields, pending_nav);

    // Build cookie accessors before ObjectInitializer borrows context mutably
    let cookie_accessors = cookie_ctx.map(|ctx| {
        let jar = Arc::clone(&ctx.cookie_jar);
        let url_str = ctx.current_url.clone();
        let cookie_getter = FunctionObjectBuilder::new(
            context.realm(),
            // SAFETY: Arc<CookieJar> is 'static.
            unsafe {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let url: reqwest::Url = match url_str.parse() {
                        Ok(u) => u,
                        Err(_) => return Ok(JsValue::from(js_string!(""))),
                    };
                    let domain = url.host_str().unwrap_or_default();
                    let cookies = jar.list_cookies(Some(domain));
                    let cookie_str = cookies
                        .iter()
                        .map(|c| format!("{}={}", c.name, c.value))
                        .collect::<Vec<_>>()
                        .join("; ");
                    Ok(JsValue::from(js_string!(cookie_str)))
                })
            },
        )
        .build();

        let jar = Arc::clone(&ctx.cookie_jar);
        let url_str = ctx.current_url.clone();
        let cookie_setter = FunctionObjectBuilder::new(
            context.realm(),
            unsafe {
                NativeFunction::from_closure(move |_this, args, ctx| {
                    let cookie_str = arg_to_string(args, 0, ctx, "document.cookie setter")?;
                    let url: reqwest::Url = match url_str.parse() {
                        Ok(u) => u,
                        Err(_) => return Ok(JsValue::undefined()),
                    };
                    jar.add_cookie_str(&cookie_str, &url);
                    Ok(JsValue::undefined())
                })
            },
        )
        .build();

        (cookie_getter, cookie_setter)
    });

    // Build document.body, document.head, document.documentElement as lazy accessors.
    // make_element is expensive, so we only construct them when the script accesses the property.
    let body_getter = build_lazy_element_getter(context, doc, &SEL_BODY, filled_fields, pending_nav);
    let head_getter = build_lazy_element_getter(context, doc, &SEL_HEAD, filled_fields, pending_nav);
    let doc_el_getter = build_lazy_element_getter(context, doc, &SEL_HTML, filled_fields, pending_nav);

    let mut builder = ObjectInitializer::new(context);
    builder
        .property(js_string!("title"), js_string!(title), Attribute::READONLY)
        .function(qs_fn, js_string!("querySelector"), 1)
        .function(qsa_fn, js_string!("querySelectorAll"), 1);

    if let Some((getter, setter)) = cookie_accessors {
        builder.accessor(
            js_string!("cookie"),
            Some(getter),
            Some(setter),
            Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
        );
    }

    builder.accessor(js_string!("body"), Some(body_getter), None, Attribute::CONFIGURABLE);
    builder.accessor(js_string!("head"), Some(head_getter), None, Attribute::CONFIGURABLE);
    builder.accessor(
        js_string!("documentElement"),
        Some(doc_el_getter),
        None,
        Attribute::CONFIGURABLE,
    );

    let document = builder.build();
    context.register_global_property(js_string!("document"), document, Attribute::all())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Window / location globals
// ---------------------------------------------------------------------------

fn make_nav_closure(
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
    label: &'static str,
) -> NativeFunction {
    let pn = Rc::clone(pending_nav);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let url = arg_to_string(args, 0, ctx, label)?;
            *pn.borrow_mut() = Some(PendingNavigation::Link { href: url });
            Ok(JsValue::undefined())
        })
    }
}

fn build_location_object(
    context: &mut Context,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> JsResult<JsObject> {
    let href_setter = FunctionObjectBuilder::new(
        context.realm(),
        make_nav_closure(pending_nav, "location.href setter"),
    )
    .build();

    let location = ObjectInitializer::new(context)
        .function(make_nav_closure(pending_nav, "location.replace"), js_string!("replace"), 1)
        .function(make_nav_closure(pending_nav, "location.assign"), js_string!("assign"), 1)
        .build();

    let href_getter = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::from(js_string!("")))),
    )
    .build();
    location.define_property_or_throw(
        js_string!("href"),
        boa_engine::property::PropertyDescriptor::builder()
            .get(href_getter)
            .set(href_setter)
            .enumerable(true)
            .configurable(true)
            .build(),
        context,
    )?;

    Ok(location)
}

fn register_window(
    context: &mut Context,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> JsResult<()> {
    let location = build_location_object(context, pending_nav)?;

    let parent = ObjectInitializer::new(context).build();
    parent.set(js_string!("location"), location.clone(), false, context)?;

    let window = ObjectInitializer::new(context).build();
    window.set(js_string!("location"), location.clone(), false, context)?;
    window.set(js_string!("parent"), parent, false, context)?;

    context.register_global_property(js_string!("window"), window, Attribute::all())?;
    context.register_global_property(js_string!("location"), location, Attribute::all())?;
    Ok(())
}

fn build_doc_qs(
    doc: &Rc<Html>,
    filled_fields: &Rc<RefCell<HashMap<String, String>>>,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> NativeFunction {
    let doc = Rc::clone(doc);
    let ff = Rc::clone(filled_fields);
    let pn = Rc::clone(pending_nav);
    // SAFETY: Closure captures Rc<Html> (ref-counted, 'static) and NodeId (Copy, 'static).
    // The boa Context owns the closure and is dropped within execute_script,
    // ensuring the Rc (and thus the Html) outlives all uses.
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "querySelector")?;
            let selector = parse_selector(&sel_str)?;
            match doc.select(&selector).next() {
                Some(el) => Ok(make_element(ctx, &doc, el.id(), &ff, &pn)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    }
}

fn build_doc_qsa(
    doc: &Rc<Html>,
    filled_fields: &Rc<RefCell<HashMap<String, String>>>,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> NativeFunction {
    let doc = Rc::clone(doc);
    let ff = Rc::clone(filled_fields);
    let pn = Rc::clone(pending_nav);
    // SAFETY: Same invariant as build_doc_qs.
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "querySelectorAll")?;
            let selector = parse_selector(&sel_str)?;
            let ids: Vec<NodeId> = doc.select(&selector).map(|el| el.id()).collect();
            let elements: Vec<JsValue> = ids
                .into_iter()
                .map(|nid| make_element(ctx, &doc, nid, &ff, &pn).map(JsValue::from))
                .collect::<JsResult<_>>()?;
            Ok(JsArray::from_iter(elements, ctx).into())
        })
    }
}

/// Build a lazy getter that constructs an element object only when accessed.
fn build_lazy_element_getter(
    context: &mut Context,
    doc: &Rc<Html>,
    selector: &Selector,
    filled_fields: &Rc<RefCell<HashMap<String, String>>>,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> JsFunction {
    let doc = Rc::clone(doc);
    let ff = Rc::clone(filled_fields);
    let pn = Rc::clone(pending_nav);
    let node_id = doc.select(selector).next().map(|el| el.id());
    FunctionObjectBuilder::new(
        context.realm(),
        // SAFETY: Same invariant as build_doc_qs — Rc<Html> outlives the closure.
        unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| match node_id {
                Some(nid) => Ok(make_element(ctx, &doc, nid, &ff, &pn)?.into()),
                None => Ok(JsValue::null()),
            })
        },
    )
    .build()
}

// ---------------------------------------------------------------------------
// localStorage global
// ---------------------------------------------------------------------------

fn register_local_storage(
    context: &mut Context,
    storage: &Rc<RefCell<HashMap<String, String>>>,
) -> JsResult<()> {
    let get_item = {
        let storage = Rc::clone(storage);
        // SAFETY: Rc<RefCell<...>> is 'static and single-threaded.
        unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let key = arg_to_string(args, 0, ctx, "localStorage.getItem")?;
                match storage.borrow().get(&key) {
                    Some(v) => Ok(JsValue::from(js_string!(v.as_str()))),
                    None => Ok(JsValue::null()),
                }
            })
        }
    };

    let set_item = {
        let storage = Rc::clone(storage);
        unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let key = arg_to_string(args, 0, ctx, "localStorage.setItem")?;
                let value = arg_to_string(args, 1, ctx, "localStorage.setItem")?;
                storage.borrow_mut().insert(key, value);
                Ok(JsValue::undefined())
            })
        }
    };

    let remove_item = {
        let storage = Rc::clone(storage);
        unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let key = arg_to_string(args, 0, ctx, "localStorage.removeItem")?;
                storage.borrow_mut().remove(&key);
                Ok(JsValue::undefined())
            })
        }
    };

    let clear = {
        let storage = Rc::clone(storage);
        unsafe {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                storage.borrow_mut().clear();
                Ok(JsValue::undefined())
            })
        }
    };

    let ls = ObjectInitializer::new(context)
        .function(get_item, js_string!("getItem"), 1)
        .function(set_item, js_string!("setItem"), 2)
        .function(remove_item, js_string!("removeItem"), 1)
        .function(clear, js_string!("clear"), 0)
        .build();

    context.register_global_property(js_string!("localStorage"), ls, Attribute::all())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Element construction
// ---------------------------------------------------------------------------

fn resolve_element(doc: &Html, nid: NodeId) -> ElementRef<'_> {
    let nr = doc.tree.get(nid).expect("valid node_id");
    ElementRef::wrap(nr).expect("element node")
}

fn make_element(
    ctx: &mut Context,
    doc: &Rc<Html>,
    node_id: NodeId,
    filled_fields: &Rc<RefCell<HashMap<String, String>>>,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> JsResult<JsObject> {
    let node_ref = doc
        .tree
        .get(node_id)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid node reference"))?;
    let element = ElementRef::wrap(node_ref)
        .ok_or_else(|| JsNativeError::typ().with_message("Node is not an element"))?;

    let tag_name: String = element.value().name.local.as_ref().to_ascii_uppercase();
    let tag_lower = tag_name.to_ascii_lowercase();
    let id_val = element.value().attr("id").unwrap_or("").to_string();
    let class_val = element.value().attr("class").unwrap_or("");
    let name_attr = element.value().attr("name").map(|s| s.to_string());

    // Standard getters
    let tc_getter = build_element_getter(ctx, doc, node_id, |el| {
        let text: String = el.text().collect();
        JsValue::from(js_string!(text))
    });
    let ih_getter = build_element_getter(ctx, doc, node_id, |el| {
        JsValue::from(js_string!(el.inner_html()))
    });
    let oh_getter = build_element_getter(ctx, doc, node_id, |el| {
        JsValue::from(js_string!(el.html()))
    });
    let href_getter = build_element_getter(ctx, doc, node_id, |el| {
        match el.value().attr("href") {
            Some(v) => JsValue::from(js_string!(v)),
            None => JsValue::undefined(),
        }
    });
    let get_attr_fn = build_get_attribute(doc, node_id);
    let el_qs = build_el_qs(doc, node_id, filled_fields, pending_nav);
    let el_qsa = build_el_qsa(doc, node_id, filled_fields, pending_nav);
    let click_fn = build_click(doc, node_id, pending_nav);

    // Build value getter/setter BEFORE ObjectInitializer borrows ctx mutably
    let is_field = matches!(tag_lower.as_str(), "input" | "textarea" | "select");
    let value_accessors = if is_field {
        let selector = element_selector(&tag_lower, &id_val, name_attr.as_deref());
        let dom_value = match tag_lower.as_str() {
            "textarea" => element.text().collect::<String>(),
            _ => element.value().attr("value").unwrap_or("").to_string(),
        };

        let ff_get = Rc::clone(filled_fields);
        let sel_get = selector.clone();
        let dom_val_get = dom_value.clone();
        let value_getter = FunctionObjectBuilder::new(
            ctx.realm(),
            unsafe {
                NativeFunction::from_closure(move |_this, _args, _ctx| {
                    let val = ff_get
                        .borrow()
                        .get(&sel_get)
                        .cloned()
                        .unwrap_or_else(|| dom_val_get.clone());
                    Ok(JsValue::from(js_string!(val)))
                })
            },
        )
        .build();

        let ff_set = Rc::clone(filled_fields);
        let sel_set = selector;
        let value_setter = FunctionObjectBuilder::new(
            ctx.realm(),
            unsafe {
                NativeFunction::from_closure(move |_this, args, ctx| {
                    let val = arg_to_string(args, 0, ctx, "element.value setter")?;
                    ff_set.borrow_mut().insert(sel_set.clone(), val);
                    Ok(JsValue::undefined())
                })
            },
        )
        .build();

        Some((value_getter, value_setter))
    } else {
        None
    };

    let submit_fn = if tag_lower == "form" {
        Some(build_form_submit(doc, node_id, pending_nav))
    } else {
        None
    };

    let mut builder = ObjectInitializer::new(ctx);
    builder
        .property(
            js_string!("tagName"),
            js_string!(tag_name.clone()),
            Attribute::READONLY,
        )
        .property(
            js_string!("id"),
            js_string!(id_val.clone()),
            Attribute::READONLY,
        )
        .property(
            js_string!("className"),
            js_string!(class_val),
            Attribute::READONLY,
        )
        .accessor(
            js_string!("textContent"),
            Some(tc_getter),
            None,
            Attribute::READONLY,
        )
        .accessor(
            js_string!("innerHTML"),
            Some(ih_getter),
            None,
            Attribute::READONLY,
        )
        .accessor(
            js_string!("outerHTML"),
            Some(oh_getter),
            None,
            Attribute::READONLY,
        )
        .accessor(
            js_string!("href"),
            Some(href_getter),
            None,
            Attribute::READONLY,
        )
        .function(get_attr_fn, js_string!("getAttribute"), 1)
        .function(el_qs, js_string!("querySelector"), 1)
        .function(el_qsa, js_string!("querySelectorAll"), 1)
        .function(click_fn, js_string!("click"), 0);

    if let Some((getter, setter)) = value_accessors {
        builder.accessor(
            js_string!("value"),
            Some(getter),
            Some(setter),
            Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
        );
    }

    if let Some(submit) = submit_fn {
        builder.function(submit, js_string!("submit"), 0);
    }

    Ok(builder.build())
}

/// Generate a CSS selector for a field element (for storing in filled_fields).
fn element_selector(tag: &str, id: &str, name: Option<&str>) -> String {
    if !id.is_empty() {
        return format!("#{id}");
    }
    if let Some(n) = name
        && !n.is_empty()
    {
        return format!("{tag}[name='{n}']");
    }
    // Fallback: just the tag (may not be unique, but best effort)
    tag.to_string()
}

/// Build a form selector string to identify a <form> element by id or position.
fn form_selector_for(element: &ElementRef) -> String {
    if let Some(id) = element.value().attr("id") {
        return format!("form#{id}");
    }
    // Fallback
    "form".to_string()
}

fn build_click(
    doc: &Rc<Html>,
    node_id: NodeId,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> NativeFunction {
    let doc = Rc::clone(doc);
    let pn = Rc::clone(pending_nav);
    unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let el = resolve_element(&doc, node_id);
            let tag = el.value().name.local.as_ref();
            match tag {
                // Link: navigate to href
                "a" => {
                    if let Some(href) = el.value().attr("href") {
                        *pn.borrow_mut() = Some(PendingNavigation::Link {
                            href: href.to_string(),
                        });
                    }
                }
                // Submit/button inside a form: submit the form
                "button" | "input" => {
                    let input_type = el
                        .value()
                        .attr("type")
                        .unwrap_or("submit")
                        .to_ascii_lowercase();
                    if input_type == "submit" || tag == "button" {
                        // Walk up to find parent <form>
                        if let Some(form_el) = find_ancestor_form(&doc, node_id) {
                            let sel = form_selector_for(&form_el);
                            *pn.borrow_mut() =
                                Some(PendingNavigation::FormSubmit { selector: sel });
                        }
                    }
                }
                _ => {} // no-op
            }
            Ok(JsValue::undefined())
        })
    }
}

fn build_form_submit(
    doc: &Rc<Html>,
    node_id: NodeId,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> NativeFunction {
    let doc = Rc::clone(doc);
    let pn = Rc::clone(pending_nav);
    unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let el = resolve_element(&doc, node_id);
            let sel = form_selector_for(&el);
            *pn.borrow_mut() = Some(PendingNavigation::FormSubmit { selector: sel });
            Ok(JsValue::undefined())
        })
    }
}

/// Walk up the tree to find an ancestor <form> element.
fn find_ancestor_form<'a>(doc: &'a Html, node_id: NodeId) -> Option<ElementRef<'a>> {
    let mut current = doc.tree.get(node_id)?;
    loop {
        current = current.parent()?;
        if let Some(el) = ElementRef::wrap(current)
            && el.value().name.local.as_ref() == "form"
        {
            return Some(el);
        }
    }
}

fn build_el_qs(
    doc: &Rc<Html>,
    nid: NodeId,
    filled_fields: &Rc<RefCell<HashMap<String, String>>>,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> NativeFunction {
    let doc = Rc::clone(doc);
    let ff = Rc::clone(filled_fields);
    let pn = Rc::clone(pending_nav);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "querySelector")?;
            let selector = parse_selector(&sel_str)?;
            let el = resolve_element(&doc, nid);
            match el.select(&selector).next() {
                Some(matched) => Ok(make_element(ctx, &doc, matched.id(), &ff, &pn)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    }
}

fn build_el_qsa(
    doc: &Rc<Html>,
    nid: NodeId,
    filled_fields: &Rc<RefCell<HashMap<String, String>>>,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> NativeFunction {
    let doc = Rc::clone(doc);
    let ff = Rc::clone(filled_fields);
    let pn = Rc::clone(pending_nav);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "querySelectorAll")?;
            let selector = parse_selector(&sel_str)?;
            let el = resolve_element(&doc, nid);
            let ids: Vec<NodeId> = el.select(&selector).map(|m| m.id()).collect();
            let elements: Vec<JsValue> = ids
                .into_iter()
                .map(|mid| make_element(ctx, &doc, mid, &ff, &pn).map(JsValue::from))
                .collect::<JsResult<_>>()?;
            Ok(JsArray::from_iter(elements, ctx).into())
        })
    }
}

// ---------------------------------------------------------------------------
// Element property getters
// ---------------------------------------------------------------------------

fn build_element_getter(
    ctx: &mut Context,
    doc: &Rc<Html>,
    nid: NodeId,
    extract: impl Fn(&ElementRef) -> JsValue + 'static,
) -> JsFunction {
    let doc = Rc::clone(doc);
    FunctionObjectBuilder::new(
        ctx.realm(),
        // SAFETY: Same invariant as build_doc_qs.
        unsafe {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                let el = resolve_element(&doc, nid);
                Ok(extract(&el))
            })
        },
    )
    .build()
}

// ---------------------------------------------------------------------------
// Element methods
// ---------------------------------------------------------------------------

fn build_get_attribute(doc: &Rc<Html>, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(doc);
    // SAFETY: Same invariant as build_doc_qs.
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let attr_name = arg_to_string(args, 0, ctx, "getAttribute")?;
            let el = resolve_element(&doc, nid);
            match el.value().attr(&attr_name) {
                Some(v) => Ok(JsValue::from(js_string!(v))),
                None => Ok(JsValue::null()),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn arg_to_string(
    args: &[JsValue],
    index: usize,
    ctx: &mut Context,
    fn_name: &str,
) -> JsResult<String> {
    let val = args.get(index).ok_or_else(|| {
        JsNativeError::typ().with_message(format!("{fn_name} requires 1 argument"))
    })?;
    let js_str = val.to_string(ctx)?;
    Ok(js_str.to_std_string_escaped())
}

fn parse_selector(sel_str: &str) -> JsResult<Selector> {
    Selector::parse(sel_str).map_err(|_| {
        JsNativeError::syntax()
            .with_message(format!("Invalid selector: {sel_str}"))
            .into()
    })
}

// ---------------------------------------------------------------------------
// JSON conversion
// ---------------------------------------------------------------------------

const MAX_ARRAY_LEN: usize = 10_000;

fn js_value_to_json(
    val: &JsValue,
    ctx: &mut Context,
) -> Result<(serde_json::Value, String), String> {
    js_value_to_json_rec(val, ctx, 0)
}

fn js_value_to_json_rec(
    val: &JsValue,
    ctx: &mut Context,
    depth: usize,
) -> Result<(serde_json::Value, String), String> {
    if depth > 10 {
        return Ok((serde_json::Value::Null, "object".into()));
    }

    Ok(match val {
        JsValue::Undefined => (serde_json::Value::Null, "undefined".into()),
        JsValue::Null => (serde_json::Value::Null, "null".into()),
        JsValue::Boolean(b) => ((*b).into(), "boolean".into()),
        JsValue::Integer(n) => ((*n).into(), "number".into()),
        JsValue::Rational(n) => {
            if n.is_finite() {
                (
                    serde_json::Number::from_f64(*n)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                    "number".into(),
                )
            } else {
                (serde_json::Value::Null, "number".into())
            }
        }
        JsValue::String(s) => (s.to_std_string_escaped().into(), "string".into()),
        JsValue::BigInt(n) => (serde_json::Value::String(n.to_string()), "bigint".into()),
        JsValue::Symbol(_) => (serde_json::Value::Null, "symbol".into()),
        JsValue::Object(obj) => {
            if JsArray::from_object(obj.clone()).is_ok() {
                convert_array(obj, ctx, depth)?
            } else {
                match stringify_via_js(val, ctx) {
                    Some(v) => (v, "object".into()),
                    None => (serde_json::Value::Null, "object".into()),
                }
            }
        }
    })
}

fn convert_array(
    obj: &JsObject,
    ctx: &mut Context,
    depth: usize,
) -> Result<(serde_json::Value, String), String> {
    let len = obj
        .get(js_string!("length"), ctx)
        .map_err(|e| e.to_string())?
        .as_number()
        .unwrap_or(0.0) as usize;

    let capped = len.min(MAX_ARRAY_LEN);
    let mut items = Vec::with_capacity(capped);
    for i in 0..capped {
        let item = obj.get(i as u32, ctx).map_err(|e| e.to_string())?;
        let (v, _) = js_value_to_json_rec(&item, ctx, depth + 1)?;
        items.push(v);
    }
    Ok((items.into(), "array".into()))
}

/// Serialize a non-array object to JSON via the built-in `JSON.stringify`.
fn stringify_via_js(val: &JsValue, ctx: &mut Context) -> Option<serde_json::Value> {
    let global = ctx.global_object();
    let json_obj = global.get(js_string!("JSON"), ctx).ok()?.as_object()?.clone();
    let stringify_val = json_obj.get(js_string!("stringify"), ctx).ok()?;
    let stringify_fn = stringify_val.as_object()?;
    let result = stringify_fn
        .call(&JsValue::undefined(), std::slice::from_ref(val), ctx)
        .ok()?;
    let s = result.as_string()?.to_std_string_escaped();
    serde_json::from_str(&s).ok()
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
        <div id="content" data-page="home">Content here</div>
    </body>
    </html>
    "#;

    /// Helper: run script without context, return DomScriptResult.
    fn run(html: &str, script: &str) -> Result<DomScriptResult, QueryError> {
        execute_script(html, script, None).map(|(r, _)| r)
    }

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
        let result = run(
            TEST_HTML,
            "document.querySelector('p.intro').innerHTML",
        )
        .unwrap();
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
    // Step 17: Web API shim tests
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
        let (result, _) = run_with_ctx(
            TEST_HTML,
            "localStorage.getItem('missing')",
            test_ctx(),
        );
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
        let (result, _) = run_with_ctx(
            TEST_HTML,
            "localStorage.getItem('theme')",
            ctx,
        );
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
        // filled_fields should have the value stored by selector
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

    // --- form.submit() tests ---

    // --- document.body / document.head / document.documentElement tests ---

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
}
