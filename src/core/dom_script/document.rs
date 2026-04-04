//! Document global registration: querySelector, querySelectorAll, getElementById,
//! getElementsByClassName, getElementsByTagName, cookie, body/head/documentElement,
//! createElement, createTextNode.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use scraper::{Node, Selector};

use super::ScriptContext;
use super::convert::{arg_to_string, parse_selector};
use super::element::{ElementCtx, build_lazy_element_getter, make_element};
use crate::core::html_to_md::{SEL_BODY, SEL_HEAD, SEL_HTML, SEL_TITLE};

// ---------------------------------------------------------------------------
// Register document global
// ---------------------------------------------------------------------------

pub fn register_document(
    context: &mut Context,
    ectx: &ElementCtx,
    cookie_ctx: Option<&ScriptContext>,
) -> JsResult<()> {
    let title = {
        let doc = ectx.doc.borrow();
        doc.select(&SEL_TITLE)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default()
    };

    let qs_fn = build_doc_qs(ectx);
    let qsa_fn = build_doc_qsa(ectx);
    let get_by_id_fn = build_get_element_by_id(ectx);
    let get_by_class_fn = build_get_elements_by_class_name(ectx);
    let get_by_tag_fn = build_get_elements_by_tag_name(ectx);
    let get_by_name_fn = build_get_elements_by_name(ectx);
    let create_el_fn = build_create_element(ectx);
    let create_text_fn = build_create_text_node(ectx);

    // Cookie accessors
    let cookie_accessors = cookie_ctx.map(|ctx| build_cookie_accessors(context, ctx));

    // Lazy element getters for body, head, documentElement
    let body_getter = build_lazy_element_getter(context, ectx, &SEL_BODY);
    let head_getter = build_lazy_element_getter(context, ectx, &SEL_HEAD);
    let doc_el_getter = build_lazy_element_getter(context, ectx, &SEL_HTML);

    // Forms and links getters
    let forms_getter = build_collection_getter(context, ectx, "form");
    let links_getter = build_collection_getter(context, ectx, "a[href]");

    // Event methods on document
    let add_event_fn = build_doc_event_method(ectx, "add");
    let remove_event_fn = build_doc_event_method(ectx, "remove");
    let dispatch_event_fn = build_doc_dispatch(ectx);

    let mut builder = ObjectInitializer::new(context);
    builder
        .property(js_string!("title"), js_string!(title), Attribute::READONLY)
        .property(js_string!("nodeType"), 9, Attribute::READONLY)
        .property(
            js_string!("nodeName"),
            js_string!("#document"),
            Attribute::READONLY,
        )
        .function(qs_fn, js_string!("querySelector"), 1)
        .function(qsa_fn, js_string!("querySelectorAll"), 1)
        .function(get_by_id_fn, js_string!("getElementById"), 1)
        .function(get_by_class_fn, js_string!("getElementsByClassName"), 1)
        .function(get_by_tag_fn, js_string!("getElementsByTagName"), 1)
        .function(get_by_name_fn, js_string!("getElementsByName"), 1)
        .function(create_el_fn, js_string!("createElement"), 1)
        .function(create_text_fn, js_string!("createTextNode"), 1)
        // Event stubs
        .function(add_event_fn, js_string!("addEventListener"), 2)
        .function(remove_event_fn, js_string!("removeEventListener"), 2)
        .function(dispatch_event_fn, js_string!("dispatchEvent"), 1);

    if let Some((getter, setter)) = cookie_accessors {
        builder.accessor(
            js_string!("cookie"),
            Some(getter),
            Some(setter),
            Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
        );
    }

    builder.accessor(
        js_string!("body"),
        Some(body_getter),
        None,
        Attribute::CONFIGURABLE,
    );
    builder.accessor(
        js_string!("head"),
        Some(head_getter),
        None,
        Attribute::CONFIGURABLE,
    );
    builder.accessor(
        js_string!("documentElement"),
        Some(doc_el_getter),
        None,
        Attribute::CONFIGURABLE,
    );
    builder.accessor(
        js_string!("forms"),
        Some(forms_getter),
        None,
        Attribute::CONFIGURABLE,
    );
    builder.accessor(
        js_string!("links"),
        Some(links_getter),
        None,
        Attribute::CONFIGURABLE,
    );

    let document = builder.build();
    context.register_global_property(js_string!("document"), document, Attribute::all())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cookie accessors
// ---------------------------------------------------------------------------

fn build_cookie_accessors(context: &mut Context, ctx: &ScriptContext) -> (JsFunction, JsFunction) {
    let jar = Arc::clone(&ctx.cookie_jar);
    let url_str = ctx.current_url.clone();
    let cookie_getter = FunctionObjectBuilder::new(context.realm(), unsafe {
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
    })
    .build();

    let jar = Arc::clone(&ctx.cookie_jar);
    let url_str = ctx.current_url.clone();
    let cookie_setter = FunctionObjectBuilder::new(context.realm(), unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let cookie_str = arg_to_string(args, 0, ctx, "document.cookie setter")?;
            let url: reqwest::Url = match url_str.parse() {
                Ok(u) => u,
                Err(_) => return Ok(JsValue::undefined()),
            };
            jar.add_cookie_str(&cookie_str, &url);
            Ok(JsValue::undefined())
        })
    })
    .build();

    (cookie_getter, cookie_setter)
}

// ---------------------------------------------------------------------------
// querySelector / querySelectorAll
// ---------------------------------------------------------------------------

fn build_doc_qs(ectx: &ElementCtx) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "querySelector")?;
            let selector = parse_selector(&sel_str)?;
            let first_id = {
                let doc = doc.borrow();
                doc.select(&selector).next().map(|el| el.id())
            };
            match first_id {
                Some(id) => Ok(make_element(ctx, &ectx2, id)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    }
}

fn build_doc_qsa(ectx: &ElementCtx) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "querySelectorAll")?;
            let selector = parse_selector(&sel_str)?;
            select_all_to_array(&doc, &ectx2, &selector, ctx)
        })
    }
}

// ---------------------------------------------------------------------------
// getElementById, getElementsByClassName, etc.
// ---------------------------------------------------------------------------

/// Shared helper: select all matching elements and return as JsArray.
fn select_all_to_array(
    doc: &Rc<RefCell<scraper::Html>>,
    ectx: &ElementCtx,
    selector: &Selector,
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let ids: Vec<ego_tree::NodeId> = {
        let doc = doc.borrow();
        doc.select(selector).map(|el| el.id()).collect()
    };
    let elements: Vec<JsValue> = ids
        .into_iter()
        .map(|nid| make_element(ctx, ectx, nid).map(JsValue::from))
        .collect::<JsResult<_>>()?;
    Ok(JsArray::from_iter(elements, ctx).into())
}

fn build_get_element_by_id(ectx: &ElementCtx) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let id = arg_to_string(args, 0, ctx, "getElementById")?;
            let sel_str = format!("[id=\"{}\"]", id.replace('"', r#"\""#));
            let selector = match Selector::parse(&sel_str) {
                Ok(s) => s,
                Err(_) => return Ok(JsValue::null()),
            };
            let first_id = {
                let doc = doc.borrow();
                doc.select(&selector).next().map(|el| el.id())
            };
            match first_id {
                Some(nid) => Ok(make_element(ctx, &ectx2, nid)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    }
}

/// Build a getElementsBy* function that converts user input to a CSS selector string.
fn build_get_elements_by(
    ectx: &ElementCtx,
    fn_name: &'static str,
    to_selector: fn(&str) -> String,
) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let input = arg_to_string(args, 0, ctx, fn_name)?;
            let sel_str = to_selector(&input);
            let selector = match Selector::parse(&sel_str) {
                Ok(s) => s,
                Err(_) => return Ok(JsArray::new(ctx).into()),
            };
            select_all_to_array(&doc, &ectx2, &selector, ctx)
        })
    }
}

fn build_get_elements_by_class_name(ectx: &ElementCtx) -> NativeFunction {
    build_get_elements_by(ectx, "getElementsByClassName", |cls| {
        cls.split_whitespace().map(|c| format!(".{c}")).collect()
    })
}

fn build_get_elements_by_tag_name(ectx: &ElementCtx) -> NativeFunction {
    build_get_elements_by(ectx, "getElementsByTagName", |tag| tag.to_string())
}

fn build_get_elements_by_name(ectx: &ElementCtx) -> NativeFunction {
    build_get_elements_by(ectx, "getElementsByName", |name| {
        format!("[name=\"{}\"]", name.replace('"', r#"\""#))
    })
}

// ---------------------------------------------------------------------------
// createElement / createTextNode
// ---------------------------------------------------------------------------

fn build_create_element(ectx: &ElementCtx) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let tag_name = arg_to_string(args, 0, ctx, "createElement")?;
            let tag_lower = tag_name.to_ascii_lowercase();

            // Create a new element node and append to root (detached holder)
            let new_el = scraper::node::Element::new(
                html5ever::QualName::new(
                    None,
                    html5ever::ns!(html),
                    html5ever::LocalName::from(tag_lower.as_str()),
                ),
                vec![],
            );
            let new_nid = {
                let mut doc = doc.borrow_mut();
                // Append to root as a "detached" element
                let root_id = doc.tree.root().id();
                let mut root = doc.tree.get_mut(root_id).unwrap();
                root.append(Node::Element(new_el)).id()
            };
            mutated.set(true);
            Ok(make_element(ctx, &ectx2, new_nid)?.into())
        })
    }
}

fn build_create_text_node(ectx: &ElementCtx) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let text = arg_to_string(args, 0, ctx, "createTextNode")?;
            let new_nid = {
                let mut doc = doc.borrow_mut();
                let root_id = doc.tree.root().id();
                let mut root = doc.tree.get_mut(root_id).unwrap();
                root.append(Node::Text(scraper::node::Text { text: text.into() }))
                    .id()
            };
            mutated.set(true);
            Ok(super::element::make_text_node_obj(ctx, &ectx2.doc, new_nid)?.into())
        })
    }
}

// ---------------------------------------------------------------------------
// Collection getter (forms, links)
// ---------------------------------------------------------------------------

fn build_collection_getter(ctx: &mut Context, ectx: &ElementCtx, selector_str: &str) -> JsFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    let sel = Selector::parse(selector_str).expect("valid built-in selector");
    FunctionObjectBuilder::new(ctx.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let ids: Vec<ego_tree::NodeId> = {
                let doc = doc.borrow();
                doc.select(&sel).map(|el| el.id()).collect()
            };
            let elements: Vec<JsValue> = ids
                .into_iter()
                .map(|nid| make_element(ctx, &ectx2, nid).map(JsValue::from))
                .collect::<JsResult<_>>()?;
            Ok(JsArray::from_iter(elements, ctx).into())
        })
    })
    .build()
}

// ---------------------------------------------------------------------------
// localStorage
// ---------------------------------------------------------------------------

pub fn register_local_storage(
    context: &mut Context,
    storage: &Rc<RefCell<HashMap<String, String>>>,
) -> JsResult<()> {
    let get_item = {
        let storage = Rc::clone(storage);
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
// Document event stubs
// ---------------------------------------------------------------------------

/// For document.addEventListener / removeEventListener we use a synthetic
/// "document" NodeId — we pick NodeId(0) which is always invalid in ego-tree
/// (since ego-tree uses 1-based indices). We use a fixed raw value of usize::MAX.
const DOC_EVENT_KEY: usize = usize::MAX;

fn doc_event_nid() -> ego_tree::NodeId {
    super::convert::node_id_from_raw(DOC_EVENT_KEY)
}

fn build_doc_event_method(ectx: &ElementCtx, mode: &'static str) -> NativeFunction {
    let store = Rc::clone(&ectx.event_store);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = arg_to_string(args, 0, ctx, "document.addEventListener")?;
            let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());
            if let Some(cb_obj) = callback.as_object() {
                if mode == "add" {
                    store
                        .borrow_mut()
                        .add_listener(doc_event_nid(), event_type, cb_obj.clone());
                } else {
                    store
                        .borrow_mut()
                        .remove_listener(doc_event_nid(), &event_type, cb_obj);
                }
            }
            Ok(JsValue::undefined())
        })
    }
}

fn build_doc_dispatch(ectx: &ElementCtx) -> NativeFunction {
    let store = Rc::clone(&ectx.event_store);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event = args
                .first()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("dispatchEvent requires an event argument")
                })?
                .as_object()
                .ok_or_else(|| {
                    JsNativeError::typ()
                        .with_message("dispatchEvent argument must be an event object")
                })?;
            let event_type = event
                .get(js_string!("type"), ctx)?
                .to_string(ctx)?
                .to_std_string_escaped();
            let listeners = store.borrow().get_listeners(doc_event_nid(), &event_type);
            for listener in listeners {
                let _ = listener.call(&JsValue::undefined(), &[JsValue::from(event.clone())], ctx);
            }
            Ok(JsValue::from(true))
        })
    }
}
