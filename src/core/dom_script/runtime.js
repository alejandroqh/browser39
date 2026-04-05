// Browser39 DOM bootstrap for deno_core (V8).
// Wires Rust ops into document/window/console globals.
// Each op_* call crosses into Rust via Deno.core.ops.

const ops = Deno.core.ops;

// -------------------------------------------------------------------------
// Element cache — avoids re-creating wrappers for the same node
// -------------------------------------------------------------------------
const _elCache = new Map();

function _clearCache() { _elCache.clear(); }

// -------------------------------------------------------------------------
// Text node wrapper
// -------------------------------------------------------------------------
function _wrapTextNode(nid) {
  if (nid === 0) return null;
  const cached = _elCache.get(nid);
  if (cached) return cached;
  const node = {
    nodeType: 3,
    nodeName: '#text',
    __node_id__: nid,
    get textContent() { return ops.op_node_text(nid); },
    set textContent(v) { ops.op_node_set_text(nid, String(v)); },
    get parentElement() { return _wrapNullable(ops.op_element_parent(nid)); },
    get parentNode() { return _wrapNullable(ops.op_element_parent(nid)); },
    get nextSibling() { return _wrapNodeNullable(ops.op_element_next_sibling(nid, false)); },
    get previousSibling() { return _wrapNodeNullable(ops.op_element_prev_sibling(nid, false)); },
    get nextElementSibling() { return _wrapNullable(ops.op_element_next_sibling(nid, true)); },
    get previousElementSibling() { return _wrapNullable(ops.op_element_prev_sibling(nid, true)); },
  };
  _elCache.set(nid, node);
  return node;
}

// -------------------------------------------------------------------------
// Element wrapper
// -------------------------------------------------------------------------
function _wrapElement(nid) {
  if (nid === 0) return null;
  const cached = _elCache.get(nid);
  if (cached && cached.nodeType === 1) return cached;

  const info = ops.op_element_info(nid);
  if (!info) return null;

  const el = {
    __node_id__: nid,
    nodeType: 1,
    tagName: info.tag_name,
    nodeName: info.tag_name,
    id: info.id,
    className: info.class_name,

    // --- Lazy computed properties ---
    get textContent() { return ops.op_element_text_content(nid); },
    set textContent(v) { ops.op_element_set_text_content(nid, String(v)); },
    get innerHTML() { return ops.op_element_inner_html(nid); },
    set innerHTML(v) { ops.op_element_set_inner_html(nid, String(v)); },
    get outerHTML() { return ops.op_element_outer_html(nid); },
    get href() { return ops.op_element_get_attribute(nid, "href"); },

    // --- DOM traversal ---
    get parentElement() { return _wrapNullable(ops.op_element_parent(nid)); },
    get parentNode() { return _wrapNullable(ops.op_element_parent(nid)); },
    get children() {
      return ops.op_element_children(nid).map(id => _wrapElement(id));
    },
    get childElementCount() { return ops.op_element_child_count(nid); },
    get firstChild() { return _wrapNodeNullable(ops.op_element_first_child(nid)); },
    get lastChild() { return _wrapNodeNullable(ops.op_element_last_child(nid)); },
    get firstElementChild() { return _wrapNullable(ops.op_element_first_element_child(nid)); },
    get lastElementChild() { return _wrapNullable(ops.op_element_last_element_child(nid)); },
    get nextSibling() { return _wrapNodeNullable(ops.op_element_next_sibling(nid, false)); },
    get previousSibling() { return _wrapNodeNullable(ops.op_element_prev_sibling(nid, false)); },
    get nextElementSibling() { return _wrapNullable(ops.op_element_next_sibling(nid, true)); },
    get previousElementSibling() { return _wrapNullable(ops.op_element_prev_sibling(nid, true)); },

    // --- Attribute methods ---
    getAttribute(name) { return ops.op_element_get_attribute(nid, name); },
    hasAttribute(name) { return ops.op_element_has_attribute(nid, name); },
    setAttribute(name, value) { ops.op_element_set_attribute(nid, name, String(value)); },
    removeAttribute(name) { ops.op_element_remove_attribute(nid, name); },

    // --- Query methods ---
    querySelector(sel) { return _wrapNullable(ops.op_element_query_selector(nid, sel)); },
    querySelectorAll(sel) {
      return ops.op_element_query_selector_all(nid, sel).map(id => _wrapElement(id));
    },

    // --- Mutation methods ---
    appendChild(child) {
      ops.op_element_append_child(nid, child.__node_id__);
      return child;
    },
    removeChild(child) {
      ops.op_element_remove_child(nid, child.__node_id__);
      return child;
    },
    insertBefore(newNode, refNode) {
      const refNid = refNode ? refNode.__node_id__ : 0;
      ops.op_element_insert_before(nid, newNode.__node_id__, refNid);
      return newNode;
    },
    remove() { ops.op_element_remove(nid); },

    // --- Matching ---
    matches(sel) { return ops.op_element_matches(nid, sel); },
    closest(sel) { return _wrapNullable(ops.op_element_closest(nid, sel)); },
    contains(other) {
      if (!other || !other.__node_id__) return false;
      return ops.op_element_contains(nid, other.__node_id__);
    },

    // --- Click ---
    click() { ops.op_element_click(nid); },

    // --- Events ---
    addEventListener(type, cb) { _eventStore.add(nid, type, cb); },
    removeEventListener(type, cb) { _eventStore.remove(nid, type, cb); },
    dispatchEvent(event) { return _dispatchEvent(nid, event); },

    // --- Attribute-based getters ---
    get disabled() { return ops.op_element_has_attribute(nid, "disabled"); },
    get checked() { return ops.op_element_has_attribute(nid, "checked"); },
    get hidden() { return ops.op_element_has_attribute(nid, "hidden"); },
    get type() { return ops.op_element_get_attribute(nid, "type") || ""; },
    get name() { return ops.op_element_get_attribute(nid, "name") || ""; },
    get src() { return ops.op_element_get_attribute(nid, "src") || ""; },
    get alt() { return ops.op_element_get_attribute(nid, "alt") || ""; },
    get placeholder() { return ops.op_element_get_attribute(nid, "placeholder") || ""; },

    // --- classList ---
    get classList() {
      const classes = (ops.op_element_get_attribute(nid, "class") || "").split(/\s+/).filter(Boolean);
      return {
        contains(c) { return classes.includes(c); },
        add(c) {
          if (!classes.includes(c)) {
            classes.push(c);
            ops.op_element_set_attribute(nid, "class", classes.join(" "));
          }
        },
        remove(c) {
          const idx = classes.indexOf(c);
          if (idx >= 0) {
            classes.splice(idx, 1);
            ops.op_element_set_attribute(nid, "class", classes.join(" "));
          }
        },
        toggle(c) {
          if (classes.includes(c)) { this.remove(c); return false; }
          this.add(c); return true;
        },
        get length() { return classes.length; },
        item(i) { return classes[i] || null; },
        [Symbol.iterator]() { return classes[Symbol.iterator](); },
      };
    },

    // --- dataset ---
    get dataset() {
      return new Proxy({}, {
        get(_, prop) {
          const attr = "data-" + prop.replace(/([A-Z])/g, "-$1").toLowerCase();
          return ops.op_element_get_attribute(nid, attr);
        },
        set(_, prop, value) {
          const attr = "data-" + prop.replace(/([A-Z])/g, "-$1").toLowerCase();
          ops.op_element_set_attribute(nid, attr, String(value));
          return true;
        },
      });
    },
  };

  // --- Form field value accessor ---
  const tagLower = info.tag_name.toLowerCase();
  if (tagLower === "input" || tagLower === "textarea" || tagLower === "select") {
    Object.defineProperty(el, "value", {
      get() { return ops.op_field_value_get(nid); },
      set(v) { ops.op_field_value_set(nid, String(v)); },
      enumerable: true,
      configurable: true,
    });
  }

  // --- form.submit() ---
  if (tagLower === "form") {
    el.submit = function() { ops.op_form_submit(nid); };
  }

  _elCache.set(nid, el);
  return el;
}

// -------------------------------------------------------------------------
// Nullable wrappers — ops return 0 for "not found"
// -------------------------------------------------------------------------
function _wrapNullable(nid) {
  return (nid && nid !== 0) ? _wrapElement(nid) : null;
}

// For nodes that might be text or element — ops return [nid, isElement]
function _wrapNodeNullable(result) {
  if (!result || result[0] === 0) return null;
  return result[1] ? _wrapElement(result[0]) : _wrapTextNode(result[0]);
}

// -------------------------------------------------------------------------
// Event system (pure JS)
// -------------------------------------------------------------------------
const _eventStore = {
  _listeners: new Map(),
  _key(nid, type) { return nid + ":" + type; },
  add(nid, type, cb) {
    const k = this._key(nid, type);
    if (!this._listeners.has(k)) this._listeners.set(k, []);
    this._listeners.get(k).push(cb);
  },
  remove(nid, type, cb) {
    const k = this._key(nid, type);
    const arr = this._listeners.get(k);
    if (arr) {
      const idx = arr.indexOf(cb);
      if (idx >= 0) arr.splice(idx, 1);
    }
  },
  get(nid, type) {
    return this._listeners.get(this._key(nid, type)) || [];
  },
};

function _dispatchEvent(nid, event) {
  // Set target/currentTarget
  const target = _wrapElement(nid) || _wrapTextNode(nid);
  try { event.target = target; } catch(e) {}
  try { event.currentTarget = target; } catch(e) {}

  const listeners = _eventStore.get(nid, event.type);
  for (const cb of listeners) {
    try { cb.call(target, event); } catch(e) { ops.op_console("error", ["Uncaught in event handler: " + e]); }
  }
  return true;
}

// -------------------------------------------------------------------------
// Document global
// -------------------------------------------------------------------------
const document = {
  nodeType: 9,
  nodeName: "#document",
  get title() { return ops.op_doc_title(); },

  querySelector(sel) { return _wrapNullable(ops.op_doc_query_selector(sel)); },
  querySelectorAll(sel) {
    return ops.op_doc_query_selector_all(sel).map(id => _wrapElement(id));
  },
  getElementById(id) { return _wrapNullable(ops.op_doc_get_element_by_id(id)); },
  getElementsByClassName(cls) {
    return ops.op_doc_get_elements_by_class(cls).map(id => _wrapElement(id));
  },
  getElementsByTagName(tag) {
    return ops.op_doc_get_elements_by_tag(tag).map(id => _wrapElement(id));
  },
  getElementsByName(name) {
    return ops.op_doc_get_elements_by_name(name).map(id => _wrapElement(id));
  },

  createElement(tag) {
    const nid = ops.op_doc_create_element(tag);
    return _wrapElement(nid);
  },
  createTextNode(text) {
    const nid = ops.op_doc_create_text_node(text);
    return _wrapTextNode(nid);
  },

  get body() { return _wrapNullable(ops.op_doc_query_selector("body")); },
  get head() { return _wrapNullable(ops.op_doc_query_selector("head")); },
  get documentElement() { return _wrapNullable(ops.op_doc_query_selector("html")); },
  get forms() { return ops.op_doc_query_selector_all("form").map(id => _wrapElement(id)); },
  get links() { return ops.op_doc_query_selector_all("a[href]").map(id => _wrapElement(id)); },

  get cookie() { return ops.op_cookie_get(); },
  set cookie(v) { ops.op_cookie_set(String(v)); },

  addEventListener(type, cb) { _eventStore.add(0xFFFFFFFF, type, cb); },
  removeEventListener(type, cb) { _eventStore.remove(0xFFFFFFFF, type, cb); },
  dispatchEvent(event) { return _dispatchEvent(0xFFFFFFFF, event); },
};
globalThis.document = document;

// -------------------------------------------------------------------------
// Window / Location
// -------------------------------------------------------------------------
const location = {
  get href() { return ops.op_location_href(); },
  set href(v) { ops.op_location_navigate(String(v)); },
  replace(url) { ops.op_location_navigate(String(url)); },
  assign(url) { ops.op_location_navigate(String(url)); },
};

const window = { location, parent: { location } };
globalThis.window = window;
globalThis.location = location;

// -------------------------------------------------------------------------
// Console
// -------------------------------------------------------------------------
globalThis.console = {
  log(...args) { ops.op_console("log", args.map(String)); },
  warn(...args) { ops.op_console("warn", args.map(String)); },
  error(...args) { ops.op_console("error", args.map(String)); },
  info(...args) { ops.op_console("info", args.map(String)); },
  debug(...args) { ops.op_console("debug", args.map(String)); },
};

// -------------------------------------------------------------------------
// localStorage
// -------------------------------------------------------------------------
globalThis.localStorage = {
  getItem(key) { return ops.op_storage_get(key); },
  setItem(key, value) { ops.op_storage_set(key, String(value)); },
  removeItem(key) { ops.op_storage_remove(key); },
  clear() { ops.op_storage_clear(); },
};

// -------------------------------------------------------------------------
// Event constructors
// -------------------------------------------------------------------------
function _makeEventClass(name, extraProps) {
  return class extends Object {
    constructor(type, opts) {
      super();
      this.type = type;
      this.bubbles = opts?.bubbles ?? false;
      this.cancelable = opts?.cancelable ?? false;
      this.defaultPrevented = false;
      this.target = null;
      this.currentTarget = null;
      if (extraProps) extraProps(this, opts);
    }
    preventDefault() { this.defaultPrevented = true; }
    stopPropagation() {}
    stopImmediatePropagation() {}
  };
}

globalThis.Event = _makeEventClass("Event");
globalThis.CustomEvent = _makeEventClass("CustomEvent", (e, opts) => {
  e.detail = opts?.detail ?? null;
});
globalThis.MouseEvent = _makeEventClass("MouseEvent");
globalThis.KeyboardEvent = _makeEventClass("KeyboardEvent", (e, opts) => {
  e.key = opts?.key ?? "";
  e.code = opts?.code ?? "";
  e.keyCode = opts?.keyCode ?? 0;
  e.which = opts?.which ?? 0;
});
globalThis.InputEvent = _makeEventClass("InputEvent", (e, opts) => {
  e.data = opts?.data ?? null;
  e.inputType = opts?.inputType ?? "insertText";
});
globalThis.FocusEvent = _makeEventClass("FocusEvent");

// -------------------------------------------------------------------------
// Timers (sync stubs — Phase 1)
// -------------------------------------------------------------------------
let _timerId = 0;
globalThis.setTimeout = function(cb, ms) {
  const id = ++_timerId;
  if (typeof cb === "function") { try { cb(); } catch(e) { ops.op_console("error", ["Uncaught in setTimeout: " + e]); } }
  else if (typeof cb === "string") { /* skip eval for safety */ }
  return id;
};
globalThis.clearTimeout = function() {};
globalThis.setInterval = function() { return ++_timerId; };
globalThis.clearInterval = function() {};
globalThis.requestAnimationFrame = function(cb) {
  if (typeof cb === "function") { try { cb(0); } catch(e) {} }
  return 1;
};
globalThis.cancelAnimationFrame = function() {};

// -------------------------------------------------------------------------
// Stubs
// -------------------------------------------------------------------------
globalThis.btoa = function(s) { return ops.op_btoa(s); };
globalThis.atob = function(s) { return ops.op_atob(s); };

globalThis.getComputedStyle = function() {
  return { getPropertyValue() { return ""; } };
};

globalThis.MutationObserver = class {
  constructor() {}
  observe() {}
  disconnect() {}
  takeRecords() { return []; }
};
