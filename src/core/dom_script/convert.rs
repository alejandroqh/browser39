//! JSON conversion helpers and shared utility functions for the JS sandbox.

use boa_engine::object::builtins::JsArray;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use scraper::Selector;

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

pub fn arg_to_string(
    args: &[JsValue],
    index: usize,
    ctx: &mut Context,
    fn_name: &str,
) -> JsResult<String> {
    let val = args.get(index).ok_or_else(|| {
        JsNativeError::typ()
            .with_message(format!("{fn_name} requires an argument at index {index}"))
    })?;
    let js_str = val.to_string(ctx)?;
    Ok(js_str.to_std_string_escaped())
}

pub fn parse_selector(sel_str: &str) -> JsResult<Selector> {
    Selector::parse(sel_str).map_err(|_| {
        JsNativeError::syntax()
            .with_message(format!("Invalid selector: {sel_str}"))
            .into()
    })
}

/// Extract a NodeId stored as `__node_id__` from a JS object.
pub fn extract_node_id(obj: &JsObject, ctx: &mut Context) -> JsResult<ego_tree::NodeId> {
    let val = obj.get(js_string!("__node_id__"), ctx)?;
    let raw = val.to_u32(ctx)? as usize;
    // SAFETY: NodeId is repr(transparent) around NonZero<usize>.
    // ego-tree internally uses 1-based indices, so raw > 0.
    if raw == 0 {
        return Err(JsNativeError::typ()
            .with_message("Object is not a DOM node")
            .into());
    }
    // Reconstruct NodeId from its raw usize representation.
    // ego-tree's NodeId wraps NonZero<usize>; we stored the raw value.
    Ok(node_id_from_raw(raw))
}

/// Convert a raw usize to ego_tree::NodeId.
///
/// ego-tree stores NodeId as NonZero<usize> (1-based index).
/// We use transmute because NodeId doesn't expose a from_raw constructor.
pub fn node_id_from_raw(raw: usize) -> ego_tree::NodeId {
    debug_assert!(raw > 0, "NodeId raw value must be > 0");
    // SAFETY: NodeId is #[repr(transparent)] around NonZero<usize>.
    unsafe { std::mem::transmute(raw) }
}

/// Convert a NodeId to its raw usize representation for storage.
pub fn node_id_to_raw(nid: ego_tree::NodeId) -> usize {
    // SAFETY: NodeId is #[repr(transparent)] around NonZero<usize>.
    unsafe { std::mem::transmute(nid) }
}

// ---------------------------------------------------------------------------
// JSON conversion
// ---------------------------------------------------------------------------

const MAX_ARRAY_LEN: usize = 10_000;

pub fn js_value_to_json(
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
    let json_obj = global
        .get(js_string!("JSON"), ctx)
        .ok()?
        .as_object()?
        .clone();
    let stringify_val = json_obj.get(js_string!("stringify"), ctx).ok()?;
    let stringify_fn = stringify_val.as_object()?;
    let result = stringify_fn
        .call(&JsValue::undefined(), std::slice::from_ref(val), ctx)
        .ok()?;
    let s = result.as_string()?.to_std_string_escaped();
    serde_json::from_str(&s).ok()
}
