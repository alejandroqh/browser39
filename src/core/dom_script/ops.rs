//! deno_core ops for DOM access. Each op reads/writes the shared DomState.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use deno_core::{op2, OpState};
use ego_tree::NodeId;
use scraper::{ElementRef, Html, Node, Selector};

use crate::core::http_client::CookieJar;
use crate::core::page::PendingNavigation;

// ---------------------------------------------------------------------------
// Shared state stored in OpState
// ---------------------------------------------------------------------------

pub struct DomState {
    pub doc: RefCell<Html>,
    pub filled_fields: RefCell<HashMap<String, String>>,
    pub pending_nav: RefCell<Option<PendingNavigation>>,
    pub dom_mutated: RefCell<bool>,
    pub console_output: RefCell<Vec<String>>,
    pub storage: RefCell<HashMap<String, String>>,
    pub cookie_jar: Option<Arc<CookieJar>>,
    pub current_url: Option<String>,
}

// ---------------------------------------------------------------------------
// NodeId helpers
// ---------------------------------------------------------------------------

const _: () = assert!(std::mem::size_of::<NodeId>() == std::mem::size_of::<usize>());

fn nid_from_raw(raw: u32) -> NodeId {
    debug_assert!(raw > 0, "NodeId raw value must be > 0");
    // SAFETY: NodeId is #[repr(transparent)] around NonZero<usize>.
    // The compile-time size assertion above guards against layout changes.
    unsafe { std::mem::transmute(raw as usize) }
}

fn nid_to_raw(nid: NodeId) -> u32 {
    let raw: usize = unsafe { std::mem::transmute(nid) };
    raw as u32
}

fn resolve_element(doc: &Html, nid: NodeId) -> Option<ElementRef<'_>> {
    doc.tree.get(nid).and_then(ElementRef::wrap)
}

/// Build CSS selector key for a form field (for filled_fields overlay).
fn field_selector(tag: &str, id: &str, name: Option<&str>) -> String {
    if !id.is_empty() {
        return format!("#{id}");
    }
    if let Some(n) = name {
        if !n.is_empty() {
            return format!("{tag}[name='{n}']");
        }
    }
    tag.to_string()
}

fn form_selector_for(el: &ElementRef) -> String {
    if let Some(id) = el.value().id() {
        return format!("form#{id}");
    }
    "form".to_string()
}

// ---------------------------------------------------------------------------
// Element info op (returns struct with tag, id, class)
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct ElementInfo {
    pub tag_name: String,
    pub id: String,
    pub class_name: String,
}

#[op2]
#[serde]
pub fn op_element_info(
    state: &OpState,
    #[smi] nid_raw: u32,
) -> Option<ElementInfo> {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let el = resolve_element(&doc, nid_from_raw(nid_raw))?;
    Some(ElementInfo {
        tag_name: el.value().name.local.as_ref().to_ascii_uppercase(),
        id: el.value().id().unwrap_or("").to_string(),
        class_name: el.value().attr("class").unwrap_or("").to_string(),
    })
}

// ---------------------------------------------------------------------------
// Text content ops
// ---------------------------------------------------------------------------

#[op2]
#[string]
pub fn op_element_text_content(state: &OpState, #[smi] nid_raw: u32) -> String {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    resolve_element(&doc, nid_from_raw(nid_raw))
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default()
}

#[op2]
#[string]
pub fn op_element_inner_html(state: &OpState, #[smi] nid_raw: u32) -> String {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    resolve_element(&doc, nid_from_raw(nid_raw))
        .map(|el| el.inner_html())
        .unwrap_or_default()
}

#[op2]
#[string]
pub fn op_element_outer_html(state: &OpState, #[smi] nid_raw: u32) -> String {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    resolve_element(&doc, nid_from_raw(nid_raw))
        .map(|el| el.html())
        .unwrap_or_default()
}

#[op2]
#[string]
pub fn op_node_text(state: &OpState, #[smi] nid_raw: u32) -> String {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    doc.tree
        .get(nid_from_raw(nid_raw))
        .map(|n| match n.value() {
            Node::Text(t) => t.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

fn clear_children(doc: &mut Html, nid: NodeId) {
    if let Some(mut node) = doc.tree.get_mut(nid) {
        while node.first_child().is_some() {
            node.first_child().unwrap().detach();
        }
    }
}

fn replace_children_with_text(doc: &mut Html, nid: NodeId, text: &str) {
    clear_children(doc, nid);
    if let Some(mut node) = doc.tree.get_mut(nid) {
        node.append(Node::Text(scraper::node::Text {
            text: text.into(),
        }));
    }
}

#[op2(fast)]
pub fn op_node_set_text(state: &OpState, #[smi] nid_raw: u32, #[string] text: &str) {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    replace_children_with_text(&mut doc, nid_from_raw(nid_raw), text);
    *ds.dom_mutated.borrow_mut() = true;
}

// ---------------------------------------------------------------------------
// Attribute ops
// ---------------------------------------------------------------------------

#[op2]
#[string]
pub fn op_element_get_attribute(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] attr: &str,
) -> Option<String> {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    resolve_element(&doc, nid_from_raw(nid_raw))
        .and_then(|el| el.value().attr(attr).map(|v| v.to_string()))
}

#[op2(fast)]
pub fn op_element_has_attribute(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] attr: &str,
) -> bool {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    resolve_element(&doc, nid_from_raw(nid_raw))
        .is_some_and(|el| el.value().attr(attr).is_some())
}

#[op2(fast)]
pub fn op_element_set_attribute(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] attr: &str,
    #[string] value: &str,
) {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    let nid = nid_from_raw(nid_raw);
    if let Some(mut node) = doc.tree.get_mut(nid) {
        if let Node::Element(ref mut el) = *node.value() {
            let qname = html5ever::QualName::new(
                None,
                html5ever::ns!(),
                html5ever::LocalName::from(attr),
            );
            let mut found = false;
            for a in el.attrs.iter_mut() {
                if a.0.local.as_ref() == attr {
                    a.1 = value.into();
                    found = true;
                    break;
                }
            }
            if !found {
                el.attrs.push((qname, value.into()));
            }
        }
    }
    *ds.dom_mutated.borrow_mut() = true;
}

#[op2(fast)]
pub fn op_element_remove_attribute(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] attr: &str,
) {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    let nid = nid_from_raw(nid_raw);
    if let Some(mut node) = doc.tree.get_mut(nid) {
        if let Node::Element(ref mut el) = *node.value() {
            el.attrs.retain(|a| a.0.local.as_ref() != attr);
        }
    }
    *ds.dom_mutated.borrow_mut() = true;
}

// ---------------------------------------------------------------------------
// DOM traversal ops
// ---------------------------------------------------------------------------

#[op2(fast)]
#[smi]
pub fn op_element_parent(state: &OpState, #[smi] nid_raw: u32) -> u32 {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    doc.tree
        .get(nid_from_raw(nid_raw))
        .and_then(|n| n.parent())
        .filter(|p| ElementRef::wrap(*p).is_some())
        .map(|p| nid_to_raw(p.id()))
        .unwrap_or(0)
}

#[op2]
#[serde]
pub fn op_element_children(state: &OpState, #[smi] nid_raw: u32) -> Vec<u32> {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    doc.tree
        .get(nid_from_raw(nid_raw))
        .map(|n| {
            n.children()
                .filter(|c| ElementRef::wrap(*c).is_some())
                .map(|c| nid_to_raw(c.id()))
                .collect()
        })
        .unwrap_or_default()
}

#[op2(fast)]
#[smi]
pub fn op_element_child_count(state: &OpState, #[smi] nid_raw: u32) -> u32 {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    doc.tree
        .get(nid_from_raw(nid_raw))
        .map(|n| {
            n.children()
                .filter(|c| ElementRef::wrap(*c).is_some())
                .count() as u32
        })
        .unwrap_or(0)
}

/// Returns [nid, is_element] or [0, false] if not found.
#[op2]
#[serde]
pub fn op_element_first_child(state: &OpState, #[smi] nid_raw: u32) -> (u32, bool) {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    doc.tree
        .get(nid_from_raw(nid_raw))
        .and_then(|n| n.first_child())
        .map(|c| (nid_to_raw(c.id()), c.value().is_element()))
        .unwrap_or((0, false))
}

#[op2]
#[serde]
pub fn op_element_last_child(state: &OpState, #[smi] nid_raw: u32) -> (u32, bool) {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    doc.tree
        .get(nid_from_raw(nid_raw))
        .and_then(|n| n.last_child())
        .map(|c| (nid_to_raw(c.id()), c.value().is_element()))
        .unwrap_or((0, false))
}

#[op2(fast)]
#[smi]
pub fn op_element_first_element_child(state: &OpState, #[smi] nid_raw: u32) -> u32 {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    doc.tree
        .get(nid_from_raw(nid_raw))
        .and_then(|n| n.children().find(|c| ElementRef::wrap(*c).is_some()))
        .map(|c| nid_to_raw(c.id()))
        .unwrap_or(0)
}

#[op2(fast)]
#[smi]
pub fn op_element_last_element_child(state: &OpState, #[smi] nid_raw: u32) -> u32 {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    doc.tree
        .get(nid_from_raw(nid_raw))
        .and_then(|n| {
            n.children()
                .filter(|c| ElementRef::wrap(*c).is_some())
                .last()
        })
        .map(|c| nid_to_raw(c.id()))
        .unwrap_or(0)
}

/// Returns [nid, is_element] for next sibling. If element_only, skips non-elements.
#[op2]
#[serde]
pub fn op_element_next_sibling(
    state: &OpState,
    #[smi] nid_raw: u32,
    element_only: bool,
) -> (u32, bool) {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let mut sib = doc.tree.get(nid_from_raw(nid_raw)).and_then(|n| n.next_sibling());
    while let Some(s) = sib {
        if !element_only || ElementRef::wrap(s).is_some() {
            return (nid_to_raw(s.id()), s.value().is_element());
        }
        sib = s.next_sibling();
    }
    (0, false)
}

#[op2]
#[serde]
pub fn op_element_prev_sibling(
    state: &OpState,
    #[smi] nid_raw: u32,
    element_only: bool,
) -> (u32, bool) {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let mut sib = doc.tree.get(nid_from_raw(nid_raw)).and_then(|n| n.prev_sibling());
    while let Some(s) = sib {
        if !element_only || ElementRef::wrap(s).is_some() {
            return (nid_to_raw(s.id()), s.value().is_element());
        }
        sib = s.prev_sibling();
    }
    (0, false)
}

// ---------------------------------------------------------------------------
// Query ops
// ---------------------------------------------------------------------------

#[op2(fast)]
#[smi]
pub fn op_doc_query_selector(state: &OpState, #[string] sel: &str) -> u32 {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    Selector::parse(sel)
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|el| nid_to_raw(el.id()))
        .unwrap_or(0)
}

#[op2]
#[serde]
pub fn op_doc_query_selector_all(state: &OpState, #[string] sel: &str) -> Vec<u32> {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    Selector::parse(sel)
        .ok()
        .map(|s| doc.select(&s).map(|el| nid_to_raw(el.id())).collect())
        .unwrap_or_default()
}

#[op2(fast)]
#[smi]
pub fn op_doc_get_element_by_id(state: &OpState, #[string] id: &str) -> u32 {
    let sel_str = format!("[id=\"{}\"]", id.replace('"', "\\\""));
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    Selector::parse(&sel_str)
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|el| nid_to_raw(el.id()))
        .unwrap_or(0)
}

#[op2]
#[serde]
pub fn op_doc_get_elements_by_class(
    state: &OpState,
    #[string] cls: &str,
) -> Vec<u32> {
    let sel_str: String = cls.split_whitespace().map(|c| format!(".{c}")).collect();
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    Selector::parse(&sel_str)
        .ok()
        .map(|s| doc.select(&s).map(|el| nid_to_raw(el.id())).collect())
        .unwrap_or_default()
}

#[op2]
#[serde]
pub fn op_doc_get_elements_by_tag(
    state: &OpState,
    #[string] tag: &str,
) -> Vec<u32> {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    Selector::parse(tag)
        .ok()
        .map(|s| doc.select(&s).map(|el| nid_to_raw(el.id())).collect())
        .unwrap_or_default()
}

#[op2]
#[serde]
pub fn op_doc_get_elements_by_name(
    state: &OpState,
    #[string] name: &str,
) -> Vec<u32> {
    let sel_str = format!("[name=\"{}\"]", name.replace('"', "\\\""));
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    Selector::parse(&sel_str)
        .ok()
        .map(|s| doc.select(&s).map(|el| nid_to_raw(el.id())).collect())
        .unwrap_or_default()
}

#[op2]
#[string]
pub fn op_doc_title(state: &OpState) -> String {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let sel = Selector::parse("title").unwrap();
    doc.select(&sel)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Element-scoped query ops
// ---------------------------------------------------------------------------

#[op2(fast)]
#[smi]
pub fn op_element_query_selector(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] sel: &str,
) -> u32 {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let parent = resolve_element(&doc, nid_from_raw(nid_raw));
    parent
        .and_then(|p| Selector::parse(sel).ok().and_then(|s| p.select(&s).next()))
        .map(|el| nid_to_raw(el.id()))
        .unwrap_or(0)
}

#[op2]
#[serde]
pub fn op_element_query_selector_all(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] sel: &str,
) -> Vec<u32> {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    resolve_element(&doc, nid_from_raw(nid_raw))
        .and_then(|p| {
            Selector::parse(sel)
                .ok()
                .map(|s| p.select(&s).map(|el| nid_to_raw(el.id())).collect())
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Matching ops
// ---------------------------------------------------------------------------

#[op2(fast)]
pub fn op_element_matches(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] sel: &str,
) -> bool {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    Selector::parse(sel)
        .ok()
        .and_then(|s| resolve_element(&doc, nid_from_raw(nid_raw)).map(|el| s.matches(&el)))
        .unwrap_or(false)
}

#[op2(fast)]
#[smi]
pub fn op_element_closest(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] sel: &str,
) -> u32 {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let selector = match Selector::parse(sel) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let mut current_id = nid_from_raw(nid_raw);
    loop {
        if let Some(el) = resolve_element(&doc, current_id) {
            if selector.matches(&el) {
                return nid_to_raw(current_id);
            }
        }
        match doc.tree.get(current_id).and_then(|n| n.parent()) {
            Some(parent) if ElementRef::wrap(parent).is_some() => {
                current_id = parent.id();
            }
            _ => return 0,
        }
    }
}

#[op2(fast)]
pub fn op_element_contains(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[smi] other_raw: u32,
) -> bool {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let target = nid_from_raw(nid_raw);
    let mut current = doc.tree.get(nid_from_raw(other_raw));
    while let Some(node) = current {
        if node.id() == target {
            return true;
        }
        current = node.parent();
    }
    false
}

// ---------------------------------------------------------------------------
// Mutation ops
// ---------------------------------------------------------------------------

#[op2(fast)]
pub fn op_element_set_text_content(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] text: &str,
) {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    replace_children_with_text(&mut doc, nid_from_raw(nid_raw), text);
    *ds.dom_mutated.borrow_mut() = true;
}

#[op2(fast)]
pub fn op_element_set_inner_html(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] html_str: &str,
) {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    let nid = nid_from_raw(nid_raw);

    clear_children(&mut doc, nid);
    let fragment = Html::parse_fragment(html_str);
    graft_fragment(&mut doc, nid, &fragment);
    *ds.dom_mutated.borrow_mut() = true;
}

#[op2(fast)]
pub fn op_element_append_child(
    state: &OpState,
    #[smi] parent_raw: u32,
    #[smi] child_raw: u32,
) {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    let child_nid = nid_from_raw(child_raw);
    let parent_nid = nid_from_raw(parent_raw);
    // Detach child from old parent
    if let Some(mut node) = doc.tree.get_mut(child_nid) {
        node.detach();
    }
    // Append to new parent
    if let Some(mut parent) = doc.tree.get_mut(parent_nid) {
        parent.append_id(child_nid);
    }
    *ds.dom_mutated.borrow_mut() = true;
}

#[op2(fast)]
pub fn op_element_remove_child(
    state: &OpState,
    #[smi] _parent_raw: u32,
    #[smi] child_raw: u32,
) {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    if let Some(mut node) = doc.tree.get_mut(nid_from_raw(child_raw)) {
        node.detach();
    }
    *ds.dom_mutated.borrow_mut() = true;
}

#[op2(fast)]
pub fn op_element_insert_before(
    state: &OpState,
    #[smi] _parent_raw: u32,
    #[smi] new_raw: u32,
    #[smi] ref_raw: u32,
) {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    let new_nid = nid_from_raw(new_raw);
    // Detach the new node first
    if let Some(mut node) = doc.tree.get_mut(new_nid) {
        node.detach();
    }
    if ref_raw > 0 {
        let ref_nid = nid_from_raw(ref_raw);
        if let Some(mut ref_node) = doc.tree.get_mut(ref_nid) {
            ref_node.insert_id_before(new_nid);
        }
    }
    *ds.dom_mutated.borrow_mut() = true;
}

#[op2(fast)]
pub fn op_element_remove(state: &OpState, #[smi] nid_raw: u32) {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    if let Some(mut node) = doc.tree.get_mut(nid_from_raw(nid_raw)) {
        node.detach();
    }
    *ds.dom_mutated.borrow_mut() = true;
}

// ---------------------------------------------------------------------------
// createElement / createTextNode
// ---------------------------------------------------------------------------

#[op2(fast)]
#[smi]
pub fn op_doc_create_element(state: &OpState, #[string] tag: &str) -> u32 {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    let tag_lower = tag.to_ascii_lowercase();
    let new_el = scraper::node::Element::new(
        html5ever::QualName::new(
            None,
            html5ever::ns!(html),
            html5ever::LocalName::from(tag_lower.as_str()),
        ),
        vec![],
    );
    let root_id = doc.tree.root().id();
    let new_nid = {
        let mut root = doc.tree.get_mut(root_id).unwrap();
        root.append(Node::Element(new_el)).id()
    };
    *ds.dom_mutated.borrow_mut() = true;
    nid_to_raw(new_nid)
}

#[op2(fast)]
#[smi]
pub fn op_doc_create_text_node(state: &OpState, #[string] text: &str) -> u32 {
    let ds = state.borrow::<DomState>();
    let mut doc = ds.doc.borrow_mut();
    let root_id = doc.tree.root().id();
    let new_nid = {
        let mut root = doc.tree.get_mut(root_id).unwrap();
        root.append(Node::Text(scraper::node::Text {
            text: text.into(),
        }))
        .id()
    };
    *ds.dom_mutated.borrow_mut() = true;
    nid_to_raw(new_nid)
}

// ---------------------------------------------------------------------------
// Click / Submit
// ---------------------------------------------------------------------------

#[op2(fast)]
pub fn op_element_click(state: &OpState, #[smi] nid_raw: u32) {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let nid = nid_from_raw(nid_raw);
    if let Some(el) = resolve_element(&doc, nid) {
        let tag = el.value().name.local.as_ref();
        match tag {
            "a" => {
                if let Some(href) = el.value().attr("href") {
                    *ds.pending_nav.borrow_mut() = Some(PendingNavigation::Link {
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
                if input_type == "submit" || tag == "button" {
                    // Find ancestor form
                    let mut current = doc.tree.get(nid);
                    while let Some(node) = current {
                        if let Some(form) = ElementRef::wrap(node) {
                            if form.value().name.local.as_ref() == "form" {
                                let sel = form_selector_for(&form);
                                *ds.pending_nav.borrow_mut() =
                                    Some(PendingNavigation::FormSubmit { selector: sel });
                                break;
                            }
                        }
                        current = node.parent();
                    }
                }
            }
            _ => {}
        }
    }
}

#[op2(fast)]
pub fn op_form_submit(state: &OpState, #[smi] nid_raw: u32) {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    if let Some(el) = resolve_element(&doc, nid_from_raw(nid_raw)) {
        let sel = form_selector_for(&el);
        *ds.pending_nav.borrow_mut() = Some(PendingNavigation::FormSubmit { selector: sel });
    }
}

// ---------------------------------------------------------------------------
// Form field value ops (filled_fields overlay)
// ---------------------------------------------------------------------------

#[op2]
#[string]
pub fn op_field_value_get(state: &OpState, #[smi] nid_raw: u32) -> String {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let nid = nid_from_raw(nid_raw);
    if let Some(el) = resolve_element(&doc, nid) {
        let tag = el.value().name.local.as_ref();
        let id_val = el.value().id().unwrap_or("");
        let name_val = el.value().attr("name");
        let selector = field_selector(tag, id_val, name_val);

        let ff = ds.filled_fields.borrow();
        if let Some(val) = ff.get(&selector) {
            return val.clone();
        }
        // DOM default
        match tag {
            "textarea" => el.text().collect::<String>(),
            _ => el.value().attr("value").unwrap_or("").to_string(),
        }
    } else {
        String::new()
    }
}

#[op2(fast)]
pub fn op_field_value_set(
    state: &OpState,
    #[smi] nid_raw: u32,
    #[string] value: &str,
) {
    let ds = state.borrow::<DomState>();
    let doc = ds.doc.borrow();
    let nid = nid_from_raw(nid_raw);
    if let Some(el) = resolve_element(&doc, nid) {
        let tag = el.value().name.local.as_ref();
        let id_val = el.value().id().unwrap_or("");
        let name_val = el.value().attr("name");
        let selector = field_selector(tag, id_val, name_val);
        ds.filled_fields
            .borrow_mut()
            .insert(selector, value.to_string());
    }
}

// ---------------------------------------------------------------------------
// Window / Location ops
// ---------------------------------------------------------------------------

#[op2]
#[string]
pub fn op_location_href(state: &OpState) -> String {
    let ds = state.borrow::<DomState>();
    ds.current_url.clone().unwrap_or_default()
}

#[op2(fast)]
pub fn op_location_navigate(state: &OpState, #[string] url: &str) {
    let ds = state.borrow::<DomState>();
    *ds.pending_nav.borrow_mut() = Some(PendingNavigation::Link {
        href: url.to_string(),
    });
}

// ---------------------------------------------------------------------------
// Console ops
// ---------------------------------------------------------------------------

#[op2]
pub fn op_console(
    state: &OpState,
    #[string] level: &str,
    #[serde] args: Vec<String>,
) {
    let ds = state.borrow::<DomState>();
    let msg = format!("[{level}] {}", args.join(" "));
    ds.console_output.borrow_mut().push(msg);
}

// ---------------------------------------------------------------------------
// Storage ops
// ---------------------------------------------------------------------------

#[op2]
#[string]
pub fn op_storage_get(state: &OpState, #[string] key: &str) -> Option<String> {
    let ds = state.borrow::<DomState>();
    ds.storage.borrow().get(key).cloned()
}

#[op2(fast)]
pub fn op_storage_set(state: &OpState, #[string] key: &str, #[string] value: &str) {
    let ds = state.borrow::<DomState>();
    ds.storage
        .borrow_mut()
        .insert(key.to_string(), value.to_string());
}

#[op2(fast)]
pub fn op_storage_remove(state: &OpState, #[string] key: &str) {
    let ds = state.borrow::<DomState>();
    ds.storage.borrow_mut().remove(key);
}

#[op2(fast)]
pub fn op_storage_clear(state: &OpState) {
    let ds = state.borrow::<DomState>();
    ds.storage.borrow_mut().clear();
}

// ---------------------------------------------------------------------------
// Cookie ops
// ---------------------------------------------------------------------------

#[op2]
#[string]
pub fn op_cookie_get(state: &OpState) -> String {
    let ds = state.borrow::<DomState>();
    if let Some(ref jar) = ds.cookie_jar {
        if let Some(ref url_str) = ds.current_url {
            if let Ok(url) = url_str.parse::<reqwest::Url>() {
                let domain = url.host_str().unwrap_or_default();
                let cookies = jar.list_cookies(Some(domain));
                return cookies
                    .iter()
                    .map(|c| format!("{}={}", c.name, c.value))
                    .collect::<Vec<_>>()
                    .join("; ");
            }
        }
    }
    String::new()
}

#[op2(fast)]
pub fn op_cookie_set(state: &OpState, #[string] cookie_str: &str) {
    let ds = state.borrow::<DomState>();
    if let Some(ref jar) = ds.cookie_jar {
        if let Some(ref url_str) = ds.current_url {
            if let Ok(url) = url_str.parse::<reqwest::Url>() {
                jar.add_cookie_str(cookie_str, &url);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Base64 ops
// ---------------------------------------------------------------------------

#[op2]
#[string]
pub fn op_btoa(#[string] input: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(input.as_bytes())
}

#[op2]
#[string]
pub fn op_atob(#[string] input: &str) -> Result<String, deno_error::JsErrorBox> {
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(input.as_bytes())
        .map_err(|e| deno_error::JsErrorBox::type_error(format!("atob: {e}")))?;
    String::from_utf8(decoded)
        .map_err(|e| deno_error::JsErrorBox::type_error(format!("atob: {e}")))
}

// ---------------------------------------------------------------------------
// Fragment grafting helper (for innerHTML setter)
// ---------------------------------------------------------------------------

fn graft_fragment(doc: &mut Html, parent_nid: NodeId, fragment: &Html) {
    // Fragment root's children are the parsed nodes
    let frag_root = fragment.tree.root();
    for child in frag_root.children() {
        graft_node(doc, parent_nid, &fragment.tree, child.id());
    }
}

fn graft_node(
    doc: &mut Html,
    parent_nid: NodeId,
    src_tree: &ego_tree::Tree<Node>,
    src_nid: NodeId,
) {
    let src_node = src_tree.get(src_nid).unwrap();
    let cloned_node = src_node.value().clone();
    let new_nid = {
        let mut parent = doc.tree.get_mut(parent_nid).unwrap();
        parent.append(cloned_node).id()
    };
    // Recursively graft children
    for child in src_node.children() {
        graft_node(doc, new_nid, src_tree, child.id());
    }
}
