//! Window, location, console, timers, and utility globals.

use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{
    Context, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, Source, js_string,
};

use super::convert::arg_to_string;
use crate::core::page::PendingNavigation;

// ---------------------------------------------------------------------------
// Window / Location
// ---------------------------------------------------------------------------

pub fn register_window(
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
        .function(
            make_nav_closure(pending_nav, "location.replace"),
            js_string!("replace"),
            1,
        )
        .function(
            make_nav_closure(pending_nav, "location.assign"),
            js_string!("assign"),
            1,
        )
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

// ---------------------------------------------------------------------------
// Console
// ---------------------------------------------------------------------------

pub fn register_console(context: &mut Context, output: &Rc<RefCell<Vec<String>>>) -> JsResult<()> {
    let log_fn = build_console_fn(context, output, "log");
    let warn_fn = build_console_fn(context, output, "warn");
    let error_fn = build_console_fn(context, output, "error");
    let info_fn = build_console_fn(context, output, "info");
    let debug_fn = build_console_fn(context, output, "debug");

    let console = ObjectInitializer::new(context)
        .function(log_fn, js_string!("log"), 0)
        .function(warn_fn, js_string!("warn"), 0)
        .function(error_fn, js_string!("error"), 0)
        .function(info_fn, js_string!("info"), 0)
        .function(debug_fn, js_string!("debug"), 0)
        .build();

    context.register_global_property(js_string!("console"), console, Attribute::all())?;
    Ok(())
}

fn build_console_fn(
    _context: &mut Context,
    output: &Rc<RefCell<Vec<String>>>,
    level: &'static str,
) -> NativeFunction {
    let out = Rc::clone(output);
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let parts: Vec<String> = args
                .iter()
                .map(|a| {
                    a.to_string(ctx)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_else(|_| "[object]".to_string())
                })
                .collect();
            let msg = format!("[{level}] {}", parts.join(" "));
            out.borrow_mut().push(msg);
            Ok(JsValue::undefined())
        })
    }
}

// ---------------------------------------------------------------------------
// Timers: setTimeout, clearTimeout, setInterval, clearInterval
// ---------------------------------------------------------------------------

pub fn register_timers(context: &mut Context) -> JsResult<()> {
    let timer_id = Rc::new(RefCell::new(0u32));

    let set_timeout_id = Rc::clone(&timer_id);
    let set_timeout = FunctionObjectBuilder::new(context.realm(), unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let callback = args.first().cloned().unwrap_or(JsValue::undefined());
            let id = {
                let mut tid = set_timeout_id.borrow_mut();
                *tid += 1;
                *tid
            };
            if let Some(cb) = callback.as_callable() {
                let _ = cb.call(&JsValue::undefined(), &[], ctx);
            } else if callback.is_string() {
                let code = callback.to_string(ctx)?.to_std_string_escaped();
                let _ = ctx.eval(Source::from_bytes(code.as_bytes()));
            }
            Ok(JsValue::from(id))
        })
    })
    .build();

    let clear_timeout = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined())),
    )
    .build();

    let set_interval_id = Rc::clone(&timer_id);
    let set_interval = FunctionObjectBuilder::new(context.realm(), unsafe {
        NativeFunction::from_closure(move |_this, _args, _ctx| {
            let id = {
                let mut tid = set_interval_id.borrow_mut();
                *tid += 1;
                *tid
            };
            Ok(JsValue::from(id))
        })
    })
    .build();

    let clear_interval = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined())),
    )
    .build();

    let raf = FunctionObjectBuilder::new(context.realm(), unsafe {
        NativeFunction::from_closure(|_this, args, ctx| {
            if let Some(cb) = args.first().and_then(|v| v.as_callable()) {
                let _ = cb.call(&JsValue::undefined(), &[JsValue::from(0)], ctx);
            }
            Ok(JsValue::from(1))
        })
    })
    .build();

    let caf = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined())),
    )
    .build();

    context.register_global_property(js_string!("setTimeout"), set_timeout, Attribute::all())?;
    context.register_global_property(
        js_string!("clearTimeout"),
        clear_timeout,
        Attribute::all(),
    )?;
    context.register_global_property(js_string!("setInterval"), set_interval, Attribute::all())?;
    context.register_global_property(
        js_string!("clearInterval"),
        clear_interval,
        Attribute::all(),
    )?;
    context.register_global_property(js_string!("requestAnimationFrame"), raf, Attribute::all())?;
    context.register_global_property(js_string!("cancelAnimationFrame"), caf, Attribute::all())?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Base64: atob / btoa
// ---------------------------------------------------------------------------

pub fn register_base64(context: &mut Context) -> JsResult<()> {
    use base64::Engine as _;

    let btoa = FunctionObjectBuilder::new(context.realm(), unsafe {
        NativeFunction::from_closure(|_this, args, ctx| {
            let input = arg_to_string(args, 0, ctx, "btoa")?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(input.as_bytes());
            Ok(JsValue::from(js_string!(encoded)))
        })
    })
    .build();

    let atob = FunctionObjectBuilder::new(context.realm(), unsafe {
        NativeFunction::from_closure(|_this, args, ctx| {
            let input = arg_to_string(args, 0, ctx, "atob")?;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(input.as_bytes())
                .map_err(|e| JsNativeError::typ().with_message(format!("atob: {e}")))?;
            let s = String::from_utf8(decoded)
                .map_err(|e| JsNativeError::typ().with_message(format!("atob: {e}")))?;
            Ok(JsValue::from(js_string!(s)))
        })
    })
    .build();

    context.register_global_property(js_string!("btoa"), btoa, Attribute::all())?;
    context.register_global_property(js_string!("atob"), atob, Attribute::all())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// MutationObserver stub
// ---------------------------------------------------------------------------

pub fn register_mutation_observer(context: &mut Context) -> JsResult<()> {
    let ctor = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(|_this, _args, ctx| {
            let noop = NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()));
            let empty_array = NativeFunction::from_fn_ptr(|_this, _args, ctx| {
                Ok(boa_engine::object::builtins::JsArray::new(ctx).into())
            });

            let observer = ObjectInitializer::new(ctx)
                .function(noop.clone(), js_string!("observe"), 2)
                .function(noop.clone(), js_string!("disconnect"), 0)
                .function(empty_array, js_string!("takeRecords"), 0)
                .build();

            Ok(observer.into())
        }),
    )
    .constructor(true)
    .build();

    context.register_global_property(js_string!("MutationObserver"), ctor, Attribute::all())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// getComputedStyle stub
// ---------------------------------------------------------------------------

pub fn register_get_computed_style(context: &mut Context) -> JsResult<()> {
    let gcs = FunctionObjectBuilder::new(
        context.realm(),
        NativeFunction::from_fn_ptr(|_this, _args, ctx| {
            let style = ObjectInitializer::new(ctx)
                .function(
                    NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                        Ok(JsValue::from(js_string!("")))
                    }),
                    js_string!("getPropertyValue"),
                    1,
                )
                .build();
            Ok(style.into())
        }),
    )
    .build();

    context.register_global_property(js_string!("getComputedStyle"), gcs, Attribute::all())?;
    Ok(())
}
