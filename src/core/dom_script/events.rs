//! Event system stubs: addEventListener, dispatchEvent, Event constructors.
//!
//! Provides basic listener storage and dispatch (flat, no propagation).
//! Prevents crashes on sites that assume these APIs exist.

use std::collections::HashMap;

use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsObject, JsResult, JsValue, NativeFunction, js_string};
use ego_tree::NodeId;

use super::convert::arg_to_string;

// ---------------------------------------------------------------------------
// Event Store
// ---------------------------------------------------------------------------

/// Stores event listeners keyed by (NodeId raw, event_type).
/// We use usize instead of NodeId directly because NodeId doesn't implement Hash.
#[derive(Default)]
pub struct EventStore {
    /// Map from (node_id_raw, event_type) to list of listener objects.
    listeners: HashMap<(usize, String), Vec<JsObject>>,
}

impl EventStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_listener(&mut self, nid: NodeId, event_type: String, callback: JsObject) {
        let key = (super::convert::node_id_to_raw(nid), event_type);
        self.listeners.entry(key).or_default().push(callback);
    }

    pub fn remove_listener(&mut self, nid: NodeId, event_type: &str, callback: &JsObject) {
        let key = (super::convert::node_id_to_raw(nid), event_type.to_string());
        if let Some(listeners) = self.listeners.get_mut(&key) {
            // Remove by object identity (JS reference equality)
            listeners.retain(|l| !JsObject::equals(l, callback));
        }
    }

    pub fn get_listeners(&self, nid: NodeId, event_type: &str) -> Vec<JsObject> {
        let key = (super::convert::node_id_to_raw(nid), event_type.to_string());
        self.listeners.get(&key).cloned().unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Event constructors: Event, CustomEvent, MouseEvent, KeyboardEvent, InputEvent
// ---------------------------------------------------------------------------

/// Register event constructors as globals.
pub fn register_event_constructors(context: &mut Context) -> JsResult<()> {
    // Event and CustomEvent use different constructors; the rest share event_constructor
    for (name, ctor_fn) in [
        (
            "Event",
            event_constructor as fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
        ),
        ("CustomEvent", custom_event_constructor),
        ("MouseEvent", event_constructor),
        ("KeyboardEvent", event_constructor),
        ("InputEvent", event_constructor),
        ("FocusEvent", event_constructor),
    ] {
        let ctor =
            FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(ctor_fn))
                .constructor(true)
                .build();
        context.register_global_property(js_string!(name), ctor, Attribute::all())?;
    }

    Ok(())
}

/// `new Event(type, options?)` — returns a plain object with event properties.
fn event_constructor(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let event_type = arg_to_string(args, 0, ctx, "Event")?;

    // Extract options if provided
    let (bubbles, cancelable) = if let Some(opts) = args.get(1).and_then(|v| v.as_object()) {
        let b = opts
            .get(js_string!("bubbles"), ctx)
            .unwrap_or(JsValue::from(false))
            .to_boolean();
        let c = opts
            .get(js_string!("cancelable"), ctx)
            .unwrap_or(JsValue::from(false))
            .to_boolean();
        (b, c)
    } else {
        (false, false)
    };

    let noop = NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()));

    let event = ObjectInitializer::new(ctx)
        .property(
            js_string!("type"),
            js_string!(event_type),
            Attribute::READONLY,
        )
        .property(js_string!("bubbles"), bubbles, Attribute::READONLY)
        .property(js_string!("cancelable"), cancelable, Attribute::READONLY)
        .property(js_string!("defaultPrevented"), false, Attribute::all())
        .property(js_string!("target"), JsValue::null(), Attribute::all())
        .property(
            js_string!("currentTarget"),
            JsValue::null(),
            Attribute::all(),
        )
        .function(noop.clone(), js_string!("preventDefault"), 0)
        .function(noop.clone(), js_string!("stopPropagation"), 0)
        .function(noop, js_string!("stopImmediatePropagation"), 0)
        .build();

    Ok(event.into())
}

/// `new CustomEvent(type, options?)` — like Event but with `detail`.
fn custom_event_constructor(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let event_type = arg_to_string(args, 0, ctx, "CustomEvent")?;

    let (bubbles, cancelable, detail) = if let Some(opts) = args.get(1).and_then(|v| v.as_object())
    {
        let b = opts
            .get(js_string!("bubbles"), ctx)
            .unwrap_or(JsValue::from(false))
            .to_boolean();
        let c = opts
            .get(js_string!("cancelable"), ctx)
            .unwrap_or(JsValue::from(false))
            .to_boolean();
        let d = opts
            .get(js_string!("detail"), ctx)
            .unwrap_or(JsValue::null());
        (b, c, d)
    } else {
        (false, false, JsValue::null())
    };

    let noop = NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()));

    let event = ObjectInitializer::new(ctx)
        .property(
            js_string!("type"),
            js_string!(event_type),
            Attribute::READONLY,
        )
        .property(js_string!("bubbles"), bubbles, Attribute::READONLY)
        .property(js_string!("cancelable"), cancelable, Attribute::READONLY)
        .property(js_string!("detail"), detail, Attribute::READONLY)
        .property(js_string!("defaultPrevented"), false, Attribute::all())
        .property(js_string!("target"), JsValue::null(), Attribute::all())
        .property(
            js_string!("currentTarget"),
            JsValue::null(),
            Attribute::all(),
        )
        .function(noop.clone(), js_string!("preventDefault"), 0)
        .function(noop.clone(), js_string!("stopPropagation"), 0)
        .function(noop, js_string!("stopImmediatePropagation"), 0)
        .build();

    Ok(event.into())
}
