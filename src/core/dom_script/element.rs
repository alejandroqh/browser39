//! Element construction and DOM property/method implementations.
//!
//! Builds JS element objects that bridge to scraper's parsed HTML tree.
//! Supports read-only properties, DOM traversal, mutation, and event stubs.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use boa_engine::object::builtins::JsArray;
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, js_string};
use ego_tree::NodeId;
use scraper::{ElementRef, Html, Node, Selector};

use super::convert::{arg_to_string, extract_node_id, node_id_to_raw, parse_selector};
use super::events::EventStore;
use crate::core::page::PendingNavigation;

/// Shared state passed to element closures.
pub struct ElementCtx {
    pub doc: Rc<RefCell<Html>>,
    pub filled_fields: Rc<RefCell<HashMap<String, String>>>,
    pub pending_nav: Rc<RefCell<Option<PendingNavigation>>>,
    pub dom_mutated: Rc<Cell<bool>>,
    pub event_store: Rc<RefCell<EventStore>>,
}

impl ElementCtx {
    pub fn clone_refs(&self) -> Self {
        ElementCtx {
            doc: Rc::clone(&self.doc),
            filled_fields: Rc::clone(&self.filled_fields),
            pending_nav: Rc::clone(&self.pending_nav),
            dom_mutated: Rc::clone(&self.dom_mutated),
            event_store: Rc::clone(&self.event_store),
        }
    }
}

// ---------------------------------------------------------------------------
// Element resolution
// ---------------------------------------------------------------------------

pub fn resolve_element(doc: &Html, nid: NodeId) -> ElementRef<'_> {
    let nr = doc.tree.get(nid).expect("valid node_id");
    ElementRef::wrap(nr).expect("element node")
}

// ---------------------------------------------------------------------------
// Main element constructor
// ---------------------------------------------------------------------------

/// Build a JS object representing a DOM element with full API surface.
pub fn make_element(ctx: &mut Context, ectx: &ElementCtx, node_id: NodeId) -> JsResult<JsObject> {
    let doc = ectx.doc.borrow();
    let node_ref = doc
        .tree
        .get(node_id)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid node reference"))?;
    let element = ElementRef::wrap(node_ref)
        .ok_or_else(|| JsNativeError::typ().with_message("Node is not an element"))?;

    let tag_name: String = element.value().name.local.as_ref().to_ascii_uppercase();
    let tag_lower = tag_name.to_ascii_lowercase();
    let id_val = element.value().id().unwrap_or("").to_string();
    let class_val = element.value().attr("class").unwrap_or("").to_string();
    let name_attr = element.value().attr("name").map(|s| s.to_string());

    // Check if this is a form field
    let is_field = matches!(tag_lower.as_str(), "input" | "textarea" | "select");
    let is_form = tag_lower == "form";

    // Pre-compute field selector and DOM value for form fields
    let field_info = if is_field {
        let selector = element_selector(&tag_lower, &id_val, name_attr.as_deref());
        let dom_value = match tag_lower.as_str() {
            "textarea" => element.text().collect::<String>(),
            _ => element.value().attr("value").unwrap_or("").to_string(),
        };
        Some((selector, dom_value))
    } else {
        None
    };

    // Drop the borrow before building closures that may re-borrow
    drop(doc);

    // --- Lazy property getters ---
    let tc_getter = build_element_getter(ctx, &ectx.doc, node_id, |el| {
        let text: String = el.text().collect();
        JsValue::from(js_string!(text))
    });
    let ih_getter = build_element_getter(ctx, &ectx.doc, node_id, |el| {
        JsValue::from(js_string!(el.inner_html()))
    });
    let oh_getter = build_element_getter(ctx, &ectx.doc, node_id, |el| {
        JsValue::from(js_string!(el.html()))
    });
    let href_getter = build_element_getter(ctx, &ectx.doc, node_id, |el| {
        match el.value().attr("href") {
            Some(v) => JsValue::from(js_string!(v)),
            None => JsValue::undefined(),
        }
    });

    // --- Phase 1: DOM traversal getters ---
    let parent_getter = build_parent_element_getter(ctx, ectx, node_id);
    let parent_node_getter = build_parent_element_getter(ctx, ectx, node_id);
    let children_getter = build_children_getter(ctx, ectx, node_id);
    let child_count_getter = build_child_element_count_getter(ctx, &ectx.doc, node_id);
    let first_child_getter = build_first_last_child_getter(ctx, ectx, node_id, true);
    let last_child_getter = build_first_last_child_getter(ctx, ectx, node_id, false);
    let first_el_child_getter = build_first_last_element_child_getter(ctx, ectx, node_id, true);
    let last_el_child_getter = build_first_last_element_child_getter(ctx, ectx, node_id, false);
    let next_sib_getter = build_sibling_getter(ctx, ectx, node_id, true, false);
    let prev_sib_getter = build_sibling_getter(ctx, ectx, node_id, false, false);
    let next_el_sib_getter = build_sibling_getter(ctx, ectx, node_id, true, true);
    let prev_el_sib_getter = build_sibling_getter(ctx, ectx, node_id, false, true);

    // --- Methods ---
    let get_attr_fn = build_get_attribute(&ectx.doc, node_id);
    let has_attr_fn = build_has_attribute(&ectx.doc, node_id);
    let el_qs = build_el_qs(ectx, node_id);
    let el_qsa = build_el_qsa(ectx, node_id);
    let click_fn = build_click(&ectx.doc, node_id, &ectx.pending_nav);
    let matches_fn = build_matches(&ectx.doc, node_id);
    let closest_fn = build_closest(ectx, node_id);
    let contains_fn = build_contains(&ectx.doc, node_id);

    // --- Phase 2: Mutation methods ---
    let set_attr_fn = build_set_attribute(ectx, node_id);
    let remove_attr_fn = build_remove_attribute(ectx, node_id);
    let remove_fn = build_remove(ectx, node_id);
    let append_child_fn = build_append_child(ectx, node_id);
    let remove_child_fn = build_remove_child(ectx, node_id);
    let insert_before_fn = build_insert_before(ectx, node_id);

    // --- Phase 3: Event stubs ---
    let add_event_fn = build_add_event_listener(&ectx.event_store, node_id);
    let remove_event_fn = build_remove_event_listener(&ectx.event_store, node_id);
    let dispatch_event_fn = build_dispatch_event(&ectx.event_store, node_id);

    // --- classList ---
    let class_list = build_class_list(ctx, &ectx.doc, node_id);

    // --- dataset ---
    let dataset = build_dataset(ctx, &ectx.doc, node_id);

    // --- value accessor (form fields) ---
    let value_accessors = if let Some((selector, dom_value)) = field_info {
        let ff_get = Rc::clone(&ectx.filled_fields);
        let sel_get = selector.clone();
        let dom_val_get = dom_value.clone();
        let value_getter = FunctionObjectBuilder::new(ctx.realm(), unsafe {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                let val = ff_get
                    .borrow()
                    .get(&sel_get)
                    .cloned()
                    .unwrap_or_else(|| dom_val_get.clone());
                Ok(JsValue::from(js_string!(val)))
            })
        })
        .build();

        let ff_set = Rc::clone(&ectx.filled_fields);
        let sel_set = selector;
        let value_setter = FunctionObjectBuilder::new(ctx.realm(), unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let val = arg_to_string(args, 0, ctx, "element.value setter")?;
                ff_set.borrow_mut().insert(sel_set.clone(), val);
                Ok(JsValue::undefined())
            })
        })
        .build();

        Some((value_getter, value_setter))
    } else {
        None
    };

    // --- form.submit() ---
    let submit_fn = if is_form {
        Some(build_form_submit(&ectx.doc, node_id, &ectx.pending_nav))
    } else {
        None
    };

    // --- textContent setter (mutation) ---
    let tc_setter_nf = build_text_content_setter_fn(ectx, node_id);
    let tc_setter = FunctionObjectBuilder::new(ctx.realm(), tc_setter_nf).build();

    // --- innerHTML setter (mutation) ---
    let ih_setter_nf = build_inner_html_setter_fn(ectx, node_id);
    let ih_setter = FunctionObjectBuilder::new(ctx.realm(), ih_setter_nf).build();

    // --- Attribute-based boolean/string getters ---
    let disabled_getter = build_attr_bool_getter(ctx, &ectx.doc, node_id, "disabled");
    let checked_getter = build_attr_bool_getter(ctx, &ectx.doc, node_id, "checked");
    let hidden_getter = build_attr_bool_getter(ctx, &ectx.doc, node_id, "hidden");
    let type_getter = build_attr_str_getter(ctx, &ectx.doc, node_id, "type");
    let name_getter = build_attr_str_getter(ctx, &ectx.doc, node_id, "name");
    let src_getter = build_attr_str_getter(ctx, &ectx.doc, node_id, "src");
    let alt_getter = build_attr_str_getter(ctx, &ectx.doc, node_id, "alt");
    let placeholder_getter = build_attr_str_getter(ctx, &ectx.doc, node_id, "placeholder");

    // --- Build the JS object ---
    let nid_raw = node_id_to_raw(node_id) as u32;
    let mut builder = ObjectInitializer::new(ctx);
    builder
        // Hidden node ID for mutation methods
        .property(js_string!("__node_id__"), nid_raw, Attribute::empty())
        // Standard properties
        .property(
            js_string!("tagName"),
            js_string!(tag_name.clone()),
            Attribute::READONLY,
        )
        .property(
            js_string!("nodeName"),
            js_string!(tag_name),
            Attribute::READONLY,
        )
        .property(js_string!("nodeType"), 1, Attribute::READONLY)
        .property(js_string!("id"), js_string!(id_val), Attribute::READONLY)
        .property(
            js_string!("className"),
            js_string!(class_val),
            Attribute::READONLY,
        )
        // Lazy computed getters
        .accessor(
            js_string!("textContent"),
            Some(tc_getter),
            Some(tc_setter),
            Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
        )
        .accessor(
            js_string!("innerHTML"),
            Some(ih_getter),
            Some(ih_setter),
            Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
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
        // DOM traversal
        .accessor(
            js_string!("parentElement"),
            Some(parent_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("parentNode"),
            Some(parent_node_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("children"),
            Some(children_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("childElementCount"),
            Some(child_count_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("firstChild"),
            Some(first_child_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("lastChild"),
            Some(last_child_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("firstElementChild"),
            Some(first_el_child_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("lastElementChild"),
            Some(last_el_child_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("nextSibling"),
            Some(next_sib_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("previousSibling"),
            Some(prev_sib_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("nextElementSibling"),
            Some(next_el_sib_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("previousElementSibling"),
            Some(prev_el_sib_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        // Attribute getters
        .accessor(
            js_string!("disabled"),
            Some(disabled_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("checked"),
            Some(checked_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("hidden"),
            Some(hidden_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        // Attribute string getters
        .accessor(
            js_string!("type"),
            Some(type_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("name"),
            Some(name_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("src"),
            Some(src_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("alt"),
            Some(alt_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        .accessor(
            js_string!("placeholder"),
            Some(placeholder_getter),
            None,
            Attribute::CONFIGURABLE,
        )
        // Sub-objects
        .property(js_string!("classList"), class_list, Attribute::READONLY)
        .property(js_string!("dataset"), dataset, Attribute::READONLY)
        // Standard methods
        .function(get_attr_fn, js_string!("getAttribute"), 1)
        .function(has_attr_fn, js_string!("hasAttribute"), 1)
        .function(el_qs, js_string!("querySelector"), 1)
        .function(el_qsa, js_string!("querySelectorAll"), 1)
        .function(click_fn, js_string!("click"), 0)
        .function(matches_fn, js_string!("matches"), 1)
        .function(closest_fn, js_string!("closest"), 1)
        .function(contains_fn, js_string!("contains"), 1)
        // Mutation methods
        .function(set_attr_fn, js_string!("setAttribute"), 2)
        .function(remove_attr_fn, js_string!("removeAttribute"), 1)
        .function(remove_fn, js_string!("remove"), 0)
        .function(append_child_fn, js_string!("appendChild"), 1)
        .function(remove_child_fn, js_string!("removeChild"), 1)
        .function(insert_before_fn, js_string!("insertBefore"), 2)
        // Event stubs
        .function(add_event_fn, js_string!("addEventListener"), 2)
        .function(remove_event_fn, js_string!("removeEventListener"), 2)
        .function(dispatch_event_fn, js_string!("dispatchEvent"), 1);

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

// ---------------------------------------------------------------------------
// Element selector generation (for filled_fields)
// ---------------------------------------------------------------------------

pub fn element_selector(tag: &str, id: &str, name: Option<&str>) -> String {
    if !id.is_empty() {
        return format!("#{id}");
    }
    if let Some(n) = name
        && !n.is_empty()
    {
        return format!("{tag}[name='{n}']");
    }
    tag.to_string()
}

pub fn form_selector_for(element: &ElementRef) -> String {
    if let Some(id) = element.value().id() {
        return format!("form#{id}");
    }
    "form".to_string()
}

// ---------------------------------------------------------------------------
// Lazy property getters
// ---------------------------------------------------------------------------

fn build_element_getter(
    ctx: &mut Context,
    doc: &Rc<RefCell<Html>>,
    nid: NodeId,
    extract: impl Fn(&ElementRef) -> JsValue + 'static,
) -> boa_engine::object::builtins::JsFunction {
    let doc = Rc::clone(doc);
    FunctionObjectBuilder::new(ctx.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let doc = doc.borrow();
            if let Some(nr) = doc.tree.get(nid)
                && let Some(el) = ElementRef::wrap(nr)
            {
                return Ok(extract(&el));
            }
            Ok(JsValue::undefined())
        })
    })
    .build()
}

/// Build a getter that returns a boolean based on the presence of an attribute.
fn build_attr_bool_getter(
    ctx: &mut Context,
    doc: &Rc<RefCell<Html>>,
    nid: NodeId,
    attr_name: &'static str,
) -> boa_engine::object::builtins::JsFunction {
    build_element_getter(ctx, doc, nid, move |el| {
        JsValue::from(el.value().attr(attr_name).is_some())
    })
}

/// Build a getter that returns a string attribute value or empty string.
fn build_attr_str_getter(
    ctx: &mut Context,
    doc: &Rc<RefCell<Html>>,
    nid: NodeId,
    attr_name: &'static str,
) -> boa_engine::object::builtins::JsFunction {
    build_element_getter(ctx, doc, nid, move |el| match el.value().attr(attr_name) {
        Some(v) => JsValue::from(js_string!(v)),
        None => JsValue::from(js_string!("")),
    })
}

// ---------------------------------------------------------------------------
// Standard methods
// ---------------------------------------------------------------------------

fn build_get_attribute(doc: &Rc<RefCell<Html>>, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(doc);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let attr_name = arg_to_string(args, 0, ctx, "getAttribute")?;
            let doc = doc.borrow();
            if let Some(nr) = doc.tree.get(nid)
                && let Some(el) = ElementRef::wrap(nr)
            {
                return match el.value().attr(&attr_name) {
                    Some(v) => Ok(JsValue::from(js_string!(v))),
                    None => Ok(JsValue::null()),
                };
            }
            Ok(JsValue::null())
        })
    }
}

fn build_has_attribute(doc: &Rc<RefCell<Html>>, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(doc);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let attr_name = arg_to_string(args, 0, ctx, "hasAttribute")?;
            let doc = doc.borrow();
            if let Some(nr) = doc.tree.get(nid)
                && let Some(el) = ElementRef::wrap(nr)
            {
                return Ok(JsValue::from(el.value().attr(&attr_name).is_some()));
            }
            Ok(JsValue::from(false))
        })
    }
}

fn build_matches(doc: &Rc<RefCell<Html>>, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(doc);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "matches")?;
            let selector = parse_selector(&sel_str)?;
            let doc = doc.borrow();
            let matched = doc
                .tree
                .get(nid)
                .and_then(ElementRef::wrap)
                .is_some_and(|el| selector.matches(&el));
            Ok(JsValue::from(matched))
        })
    }
}

fn build_closest(ectx: &ElementCtx, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "closest")?;
            let selector = parse_selector(&sel_str)?;
            let matched_id = {
                let doc_ref = doc.borrow();
                let mut current_id = nid;
                loop {
                    let node = doc_ref.tree.get(current_id);
                    let el = node.and_then(ElementRef::wrap);
                    if el.is_some_and(|e| selector.matches(&e)) {
                        break Some(current_id);
                    }
                    match node.and_then(|n| n.parent()) {
                        Some(parent) if ElementRef::wrap(parent).is_some() => {
                            current_id = parent.id();
                        }
                        _ => break None,
                    }
                }
            };
            match matched_id {
                Some(id) => Ok(make_element(ctx, &ectx2, id)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    }
}

fn build_contains(doc: &Rc<RefCell<Html>>, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(doc);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let other = args.first().ok_or_else(|| {
                JsNativeError::typ().with_message("contains requires an argument")
            })?;
            let other_obj = other.as_object().ok_or_else(|| {
                JsNativeError::typ().with_message("contains argument must be a node")
            })?;
            let other_nid = extract_node_id(other_obj, ctx)?;
            let doc = doc.borrow();
            // Walk up from other_nid, check if we hit nid
            let mut current = doc.tree.get(other_nid);
            while let Some(node) = current {
                if node.id() == nid {
                    return Ok(JsValue::from(true));
                }
                current = node.parent();
            }
            Ok(JsValue::from(false))
        })
    }
}

fn build_el_qs(ectx: &ElementCtx, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "querySelector")?;
            let selector = parse_selector(&sel_str)?;
            let first = {
                let doc = doc.borrow();
                let el = resolve_element(&doc, nid);
                el.select(&selector).next().map(|m| m.id())
            };
            match first {
                Some(mid) => Ok(make_element(ctx, &ectx2, mid)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    }
}

fn build_el_qsa(ectx: &ElementCtx, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let sel_str = arg_to_string(args, 0, ctx, "querySelectorAll")?;
            let selector = parse_selector(&sel_str)?;
            let ids: Vec<NodeId> = {
                let doc = doc.borrow();
                let el = resolve_element(&doc, nid);
                el.select(&selector).map(|m| m.id()).collect()
            };
            let elements: Vec<JsValue> = ids
                .into_iter()
                .map(|mid| make_element(ctx, &ectx2, mid).map(JsValue::from))
                .collect::<JsResult<_>>()?;
            Ok(JsArray::from_iter(elements, ctx).into())
        })
    }
}

// ---------------------------------------------------------------------------
// Click / Submit
// ---------------------------------------------------------------------------

fn build_click(
    doc: &Rc<RefCell<Html>>,
    node_id: NodeId,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> NativeFunction {
    let doc = Rc::clone(doc);
    let pn = Rc::clone(pending_nav);
    unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let doc = doc.borrow();
            let el = resolve_element(&doc, node_id);
            let tag = el.value().name.local.as_ref();
            match tag {
                "a" => {
                    if let Some(href) = el.value().attr("href") {
                        *pn.borrow_mut() = Some(PendingNavigation::Link {
                            href: href.to_string(),
                        });
                    }
                }
                "button" | "input" => {
                    let input_type = el
                        .value()
                        .attr("type")
                        .unwrap_or("submit")
                        .to_ascii_lowercase();
                    if (input_type == "submit" || tag == "button")
                        && let Some(form_el) = find_ancestor_form(&doc, node_id)
                    {
                        let sel = form_selector_for(&form_el);
                        *pn.borrow_mut() = Some(PendingNavigation::FormSubmit { selector: sel });
                    }
                }
                _ => {}
            }
            Ok(JsValue::undefined())
        })
    }
}

fn build_form_submit(
    doc: &Rc<RefCell<Html>>,
    node_id: NodeId,
    pending_nav: &Rc<RefCell<Option<PendingNavigation>>>,
) -> NativeFunction {
    let doc = Rc::clone(doc);
    let pn = Rc::clone(pending_nav);
    unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let doc = doc.borrow();
            let el = resolve_element(&doc, node_id);
            let sel = form_selector_for(&el);
            *pn.borrow_mut() = Some(PendingNavigation::FormSubmit { selector: sel });
            Ok(JsValue::undefined())
        })
    }
}

pub fn find_ancestor_form<'a>(doc: &'a Html, node_id: NodeId) -> Option<ElementRef<'a>> {
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

// ---------------------------------------------------------------------------
// Phase 1: DOM traversal getters
// ---------------------------------------------------------------------------

fn build_parent_element_getter(
    ctx: &mut Context,
    ectx: &ElementCtx,
    nid: NodeId,
) -> boa_engine::object::builtins::JsFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    FunctionObjectBuilder::new(ctx.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let parent_id = {
                let doc = doc.borrow();
                doc.tree
                    .get(nid)
                    .and_then(|n| n.parent())
                    .filter(|p| ElementRef::wrap(*p).is_some())
                    .map(|p| p.id())
            };
            match parent_id {
                Some(pid) => Ok(make_element(ctx, &ectx2, pid)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    })
    .build()
}

fn build_children_getter(
    ctx: &mut Context,
    ectx: &ElementCtx,
    nid: NodeId,
) -> boa_engine::object::builtins::JsFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    FunctionObjectBuilder::new(ctx.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let child_ids: Vec<NodeId> = {
                let doc = doc.borrow();
                doc.tree
                    .get(nid)
                    .map(|n| {
                        n.children()
                            .filter(|c| ElementRef::wrap(*c).is_some())
                            .map(|c| c.id())
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let elements: Vec<JsValue> = child_ids
                .into_iter()
                .map(|cid| make_element(ctx, &ectx2, cid).map(JsValue::from))
                .collect::<JsResult<_>>()?;
            Ok(JsArray::from_iter(elements, ctx).into())
        })
    })
    .build()
}

fn build_child_element_count_getter(
    ctx: &mut Context,
    doc: &Rc<RefCell<Html>>,
    nid: NodeId,
) -> boa_engine::object::builtins::JsFunction {
    let doc = Rc::clone(doc);
    FunctionObjectBuilder::new(ctx.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let doc = doc.borrow();
            let count = doc
                .tree
                .get(nid)
                .map(|n| {
                    n.children()
                        .filter(|c| ElementRef::wrap(*c).is_some())
                        .count()
                })
                .unwrap_or(0);
            Ok(JsValue::from(count as i32))
        })
    })
    .build()
}

fn build_first_last_child_getter(
    ctx: &mut Context,
    ectx: &ElementCtx,
    nid: NodeId,
    first: bool,
) -> boa_engine::object::builtins::JsFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    FunctionObjectBuilder::new(ctx.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let child_info = {
                let doc = doc.borrow();
                doc.tree
                    .get(nid)
                    .and_then(|n| {
                        if first {
                            n.first_child()
                        } else {
                            n.last_child()
                        }
                    })
                    .map(|c| (c.id(), c.value().is_element()))
            };
            match child_info {
                Some((cid, true)) => Ok(make_element(ctx, &ectx2, cid)?.into()),
                Some((cid, false)) => Ok(make_text_node_obj(ctx, &ectx2.doc, cid)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    })
    .build()
}

fn build_first_last_element_child_getter(
    ctx: &mut Context,
    ectx: &ElementCtx,
    nid: NodeId,
    first: bool,
) -> boa_engine::object::builtins::JsFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    FunctionObjectBuilder::new(ctx.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let child_id = {
                let doc = doc.borrow();
                doc.tree
                    .get(nid)
                    .and_then(|n| {
                        let mut children = n.children().filter(|c| ElementRef::wrap(*c).is_some());
                        if first {
                            children.next()
                        } else {
                            children.last()
                        }
                    })
                    .map(|c| c.id())
            };
            match child_id {
                Some(cid) => Ok(make_element(ctx, &ectx2, cid)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    })
    .build()
}

fn build_sibling_getter(
    ctx: &mut Context,
    ectx: &ElementCtx,
    nid: NodeId,
    next: bool,
    element_only: bool,
) -> boa_engine::object::builtins::JsFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    FunctionObjectBuilder::new(ctx.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| {
            let sibling_info = {
                let doc = doc.borrow();
                doc.tree.get(nid).and_then(|n| {
                    let mut sib = if next {
                        n.next_sibling()
                    } else {
                        n.prev_sibling()
                    };
                    while let Some(s) = sib {
                        if !element_only || ElementRef::wrap(s).is_some() {
                            return Some((s.id(), s.value().is_element()));
                        }
                        sib = if next {
                            s.next_sibling()
                        } else {
                            s.prev_sibling()
                        };
                    }
                    None
                })
            };
            match sibling_info {
                Some((sid, true)) => Ok(make_element(ctx, &ectx2, sid)?.into()),
                Some((sid, false)) => Ok(make_text_node_obj(ctx, &ectx2.doc, sid)?.into()),
                None => Ok(JsValue::null()),
            }
        })
    })
    .build()
}

/// Create a minimal JS object for a text node.
pub(super) fn make_text_node_obj(
    ctx: &mut Context,
    doc: &Rc<RefCell<Html>>,
    nid: NodeId,
) -> JsResult<JsObject> {
    let doc_ref = doc.borrow();
    let text_content = doc_ref
        .tree
        .get(nid)
        .map(|n| match n.value() {
            Node::Text(t) => t.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default();
    drop(doc_ref);

    let nid_raw = node_id_to_raw(nid) as u32;
    Ok(ObjectInitializer::new(ctx)
        .property(js_string!("__node_id__"), nid_raw, Attribute::empty())
        .property(js_string!("nodeType"), 3, Attribute::READONLY)
        .property(
            js_string!("nodeName"),
            js_string!("#text"),
            Attribute::READONLY,
        )
        .property(
            js_string!("textContent"),
            js_string!(text_content),
            Attribute::READONLY,
        )
        .build())
}

// ---------------------------------------------------------------------------
// classList
// ---------------------------------------------------------------------------

fn build_class_list(ctx: &mut Context, doc: &Rc<RefCell<Html>>, nid: NodeId) -> JsObject {
    let doc_ref = doc.borrow();
    let classes: Vec<String> = doc_ref
        .tree
        .get(nid)
        .and_then(ElementRef::wrap)
        .map(|el| {
            el.value()
                .attr("class")
                .unwrap_or("")
                .split_whitespace()
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    drop(doc_ref);

    let len = classes.len() as i32;
    let classes = Rc::new(classes);

    let c1 = Rc::clone(&classes);
    let contains_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let cls = arg_to_string(args, 0, ctx, "classList.contains")?;
            Ok(JsValue::from(c1.iter().any(|c| c == &cls)))
        })
    };

    let c2 = Rc::clone(&classes);
    let item_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, _ctx| {
            let idx = args.first().and_then(|v| v.as_number()).unwrap_or(0.0) as usize;
            match c2.get(idx) {
                Some(cls) => Ok(JsValue::from(js_string!(cls.as_str()))),
                None => Ok(JsValue::null()),
            }
        })
    };

    let c3 = Rc::clone(&classes);
    let to_string_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            Ok(JsValue::from(js_string!(c3.join(" "))))
        })
    };

    let noop = NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()));

    ObjectInitializer::new(ctx)
        .property(js_string!("length"), len, Attribute::READONLY)
        .function(contains_fn, js_string!("contains"), 1)
        .function(item_fn, js_string!("item"), 1)
        .function(to_string_fn, js_string!("toString"), 0)
        .function(noop.clone(), js_string!("add"), 1)
        .function(noop.clone(), js_string!("remove"), 1)
        .function(noop, js_string!("toggle"), 1)
        .build()
}

// ---------------------------------------------------------------------------
// dataset
// ---------------------------------------------------------------------------

fn build_dataset(ctx: &mut Context, doc: &Rc<RefCell<Html>>, nid: NodeId) -> JsObject {
    let doc_ref = doc.borrow();
    let mut builder = ObjectInitializer::new(ctx);

    if let Some(el) = doc_ref.tree.get(nid).and_then(ElementRef::wrap) {
        for (attr_name, value) in el.value().attrs() {
            if let Some(data_key) = attr_name.strip_prefix("data-") {
                // Convert kebab-case to camelCase
                let camel = kebab_to_camel(data_key);
                builder.property(js_string!(camel), js_string!(value), Attribute::READONLY);
            }
        }
    }
    drop(doc_ref);

    builder.build()
}

fn kebab_to_camel(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = false;
    for ch in s.chars() {
        if ch == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Phase 2: DOM Mutation methods
// ---------------------------------------------------------------------------

fn build_set_attribute(ectx: &ElementCtx, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let attr_name = arg_to_string(args, 0, ctx, "setAttribute")?;
            let attr_value = arg_to_string(args, 1, ctx, "setAttribute")?;
            set_attribute_on_node(&doc, nid, &attr_name, &attr_value)?;
            mutated.set(true);
            Ok(JsValue::undefined())
        })
    }
}

fn build_remove_attribute(ectx: &ElementCtx, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let attr_name = arg_to_string(args, 0, ctx, "removeAttribute")?;
            remove_attribute_on_node(&doc, nid, &attr_name)?;
            mutated.set(true);
            Ok(JsValue::undefined())
        })
    }
}

fn build_remove(ectx: &ElementCtx, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let mut doc = doc.borrow_mut();
            if let Some(mut node) = doc.tree.get_mut(nid) {
                node.detach();
                mutated.set(true);
            }
            Ok(JsValue::undefined())
        })
    }
}

fn build_append_child(ectx: &ElementCtx, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let child_obj = args
                .first()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("appendChild requires an argument")
                })?
                .as_object()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("appendChild argument must be a node")
                })?;
            let child_nid = extract_node_id(child_obj, ctx)?;

            let mut doc = doc.borrow_mut();
            // Detach child from current parent first
            if let Some(mut child) = doc.tree.get_mut(child_nid) {
                child.detach();
            }
            // Append to new parent
            if let Some(mut parent) = doc.tree.get_mut(nid) {
                parent.append_id(child_nid);
            }
            mutated.set(true);
            // Return the child
            Ok(args[0].clone())
        })
    }
}

fn build_remove_child(ectx: &ElementCtx, _nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let child_obj = args
                .first()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("removeChild requires an argument")
                })?
                .as_object()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("removeChild argument must be a node")
                })?;
            let child_nid = extract_node_id(child_obj, ctx)?;

            let mut doc = doc.borrow_mut();
            if let Some(mut child) = doc.tree.get_mut(child_nid) {
                child.detach();
                mutated.set(true);
            }
            Ok(args[0].clone())
        })
    }
}

fn build_insert_before(ectx: &ElementCtx, _nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let new_obj = args
                .first()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("insertBefore requires a new node argument")
                })?
                .as_object()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("insertBefore first argument must be a node")
                })?;
            let new_nid = extract_node_id(new_obj, ctx)?;

            let ref_val = args.get(1).cloned().unwrap_or(JsValue::null());
            if ref_val.is_null() || ref_val.is_undefined() {
                // If reference is null, just append (not implemented here, caller should use appendChild)
                return Ok(args[0].clone());
            }

            let ref_obj = ref_val.as_object().ok_or_else(|| {
                JsNativeError::typ().with_message("insertBefore second argument must be a node")
            })?;
            let ref_nid = extract_node_id(ref_obj, ctx)?;

            let mut doc = doc.borrow_mut();
            // Detach new node from current location
            if let Some(mut new_node) = doc.tree.get_mut(new_nid) {
                new_node.detach();
            }
            // Insert before reference
            if let Some(mut ref_node) = doc.tree.get_mut(ref_nid) {
                ref_node.insert_id_before(new_nid);
            }
            mutated.set(true);
            Ok(args[0].clone())
        })
    }
}

/// Detach all children of a node.
fn detach_all_children(doc: &mut Html, nid: NodeId) {
    if let Some(node) = doc.tree.get(nid) {
        let child_ids: Vec<NodeId> = node.children().map(|c| c.id()).collect();
        for cid in child_ids {
            if let Some(mut child) = doc.tree.get_mut(cid) {
                child.detach();
            }
        }
    }
}

fn build_text_content_setter_fn(ectx: &ElementCtx, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let text = arg_to_string(args, 0, ctx, "textContent setter")?;
            let mut doc = doc.borrow_mut();
            detach_all_children(&mut doc, nid);
            if let Some(mut node) = doc.tree.get_mut(nid) {
                node.append(Node::Text(scraper::node::Text { text: text.into() }));
            }
            mutated.set(true);
            Ok(JsValue::undefined())
        })
    }
}

fn build_inner_html_setter_fn(ectx: &ElementCtx, nid: NodeId) -> NativeFunction {
    let doc = Rc::clone(&ectx.doc);
    let mutated = Rc::clone(&ectx.dom_mutated);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let html_str = arg_to_string(args, 0, ctx, "innerHTML setter")?;
            let mut doc = doc.borrow_mut();
            detach_all_children(&mut doc, nid);
            let fragment = Html::parse_fragment(&html_str);
            graft_fragment(&mut doc, nid, &fragment);
            mutated.set(true);
            Ok(JsValue::undefined())
        })
    }
}

/// Graft all children of a parsed fragment into a target node.
fn graft_fragment(doc: &mut Html, target_nid: NodeId, fragment: &Html) {
    // Walk the fragment tree and recreate nodes in the target document.
    // The fragment root is a Fragment node; we want its children.
    let frag_root = fragment.tree.root();
    for child in frag_root.children() {
        graft_node(doc, target_nid, child.value(), &fragment.tree, child.id());
    }
}

fn graft_node(
    doc: &mut Html,
    parent_nid: NodeId,
    node: &Node,
    source_tree: &ego_tree::Tree<Node>,
    source_nid: NodeId,
) {
    // Create the node in the target tree
    let new_nid = if let Some(mut parent) = doc.tree.get_mut(parent_nid) {
        parent.append(node.clone()).id()
    } else {
        return;
    };

    // Recursively graft children
    if let Some(source_node) = source_tree.get(source_nid) {
        for child in source_node.children() {
            graft_node(doc, new_nid, child.value(), source_tree, child.id());
        }
    }
}

// ---------------------------------------------------------------------------
// Attribute mutation helpers
// ---------------------------------------------------------------------------

/// Set an attribute on a node, reconstructing the Element to reset OnceCell caches.
fn set_attribute_on_node(
    doc: &Rc<RefCell<Html>>,
    nid: NodeId,
    attr_name: &str,
    attr_value: &str,
) -> JsResult<()> {
    let mut doc = doc.borrow_mut();
    let mut node = doc
        .tree
        .get_mut(nid)
        .ok_or_else(|| JsNativeError::typ().with_message("Node not found"))?;

    let val = node.value();
    if let Node::Element(ref el) = *val {
        let qual = html5ever::QualName::new(
            None,
            html5ever::ns!(),
            html5ever::LocalName::from(attr_name),
        );
        // Build new attrs list
        let mut new_attrs: Vec<html5ever::Attribute> = Vec::new();
        let mut found = false;
        for (k, v) in el.attrs.iter() {
            if k.local.as_ref() == attr_name {
                new_attrs.push(html5ever::Attribute {
                    name: k.clone(),
                    value: attr_value.into(),
                });
                found = true;
            } else {
                new_attrs.push(html5ever::Attribute {
                    name: k.clone(),
                    value: v.clone(),
                });
            }
        }
        if !found {
            new_attrs.push(html5ever::Attribute {
                name: qual,
                value: attr_value.into(),
            });
        }
        let new_el = scraper::node::Element::new(el.name.clone(), new_attrs);
        *doc.tree.get_mut(nid).unwrap().value() = Node::Element(new_el);
    }
    Ok(())
}

/// Remove an attribute from a node.
fn remove_attribute_on_node(doc: &Rc<RefCell<Html>>, nid: NodeId, attr_name: &str) -> JsResult<()> {
    let mut doc = doc.borrow_mut();
    let mut node = doc
        .tree
        .get_mut(nid)
        .ok_or_else(|| JsNativeError::typ().with_message("Node not found"))?;

    let val = node.value();
    if let Node::Element(ref el) = *val {
        let new_attrs: Vec<html5ever::Attribute> = el
            .attrs
            .iter()
            .filter(|(k, _)| k.local.as_ref() != attr_name)
            .map(|(k, v)| html5ever::Attribute {
                name: k.clone(),
                value: v.clone(),
            })
            .collect();
        let new_el = scraper::node::Element::new(el.name.clone(), new_attrs);
        *doc.tree.get_mut(nid).unwrap().value() = Node::Element(new_el);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 3: Event stubs on elements
// ---------------------------------------------------------------------------

fn build_add_event_listener(event_store: &Rc<RefCell<EventStore>>, nid: NodeId) -> NativeFunction {
    let store = Rc::clone(event_store);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = arg_to_string(args, 0, ctx, "addEventListener")?;
            let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());
            if let Some(cb_obj) = callback.as_object() {
                store
                    .borrow_mut()
                    .add_listener(nid, event_type, cb_obj.clone());
            }
            Ok(JsValue::undefined())
        })
    }
}

fn build_remove_event_listener(
    event_store: &Rc<RefCell<EventStore>>,
    nid: NodeId,
) -> NativeFunction {
    let store = Rc::clone(event_store);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let event_type = arg_to_string(args, 0, ctx, "removeEventListener")?;
            let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());
            if let Some(cb_obj) = callback.as_object() {
                store.borrow_mut().remove_listener(nid, &event_type, cb_obj);
            }
            Ok(JsValue::undefined())
        })
    }
}

fn build_dispatch_event(event_store: &Rc<RefCell<EventStore>>, nid: NodeId) -> NativeFunction {
    let store = Rc::clone(event_store);
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
            let listeners = store.borrow().get_listeners(nid, &event_type);
            for listener in listeners {
                let _ = listener.call(&JsValue::undefined(), &[JsValue::from(event.clone())], ctx);
            }
            Ok(JsValue::from(true))
        })
    }
}

// ---------------------------------------------------------------------------
// Lazy element getter for document.body etc.
// ---------------------------------------------------------------------------

pub fn build_lazy_element_getter(
    ctx: &mut Context,
    ectx: &ElementCtx,
    selector: &Selector,
) -> boa_engine::object::builtins::JsFunction {
    let doc = Rc::clone(&ectx.doc);
    let ectx2 = ectx.clone_refs();
    let node_id = {
        let doc = doc.borrow();
        doc.select(selector).next().map(|el| el.id())
    };
    FunctionObjectBuilder::new(ctx.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, ctx| match node_id {
            Some(nid) => Ok(make_element(ctx, &ectx2, nid)?.into()),
            None => Ok(JsValue::null()),
        })
    })
    .build()
}
