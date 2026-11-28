//! A user-selected AcroForm is a temporary second realm in the current invocation.
use crate::host::{FieldKey, Shared, error, host, read, s};
use crate::{Context, JsObject, JsResult, JsValue, js_string, session::Job};
use boa_engine::realm::Realm;
use std::{cell::RefCell, sync::Arc};

/// An inert snapshot prepared by the engine after checking the source's copy permission.
#[derive(Clone)]
pub struct SelectedPdf(pub Arc<Job>);

#[derive(Default)]
struct Selection {
    field: Option<FieldKey>,
    pending: Option<SelectedPdf>,
    attempted: bool,
    opened: bool,
    failure: Option<&'static str>,
    deferred_alerts: Vec<usize>,
}

struct Primary(Realm);
struct Source(Realm, Shared);
type SelectionState = RefCell<Selection>;

const PATH: &str = "/pdfcraft-selected/source.pdf";

pub(crate) fn begin(ctx: &mut Context) {
    ctx.insert_data(Primary(ctx.realm().clone()));
    ctx.insert_data(SelectionState::default());
}

pub(crate) fn primary(ctx: &Context) -> bool {
    ctx.get_data::<Primary>().is_none_or(|p| &p.0 == ctx.realm())
}

pub(crate) fn source_host(ctx: &Context) -> JsResult<&Shared> {
    ctx.get_data::<Source>().filter(|source| &source.0 == ctx.realm()).map(|source| &source.1).ok_or_else(|| error("the selected PDF is closed"))
}

pub(crate) fn finish(ctx: &mut Context, out: &mut crate::Outcome) {
    drop_source(ctx);
    if let Some(selection) = ctx.remove_data::<SelectionState>().map(|state| state.into_inner())
        && selection.attempted
    {
        out.error = out.error.take().or_else(|| selection.failure.map(str::to_owned));
        let edited = !out.changed.is_empty()
            || !out.page_changes.is_empty()
            || !out.document_changes.is_empty()
            || out.requests.iter().any(|request| matches!(request, crate::Request::Reset(_)));
        out.execution_refused |= out.error.is_some() || crate::interaction::cancelled() || (!selection.opened && edited);
        if out.execution_refused {
            let mut index = 0;
            out.alerts.retain(|_| {
                let keep = !selection.deferred_alerts.contains(&index);
                index += 1;
                keep
            });
        } else {
            out.post_commit_alerts = selection.deferred_alerts.iter().filter_map(|&index| out.alerts.get(index).cloned()).collect();
        }
    }
}

pub(crate) fn defer_alert(ctx: &Context, index: usize, recorded: bool) -> bool {
    if !primary(ctx) {
        return false;
    }
    let Some(selection) = ctx.get_data::<SelectionState>() else { return false };
    let mut selection = selection.borrow_mut();
    if selection.opened && recorded {
        selection.deferred_alerts.push(index);
    }
    selection.opened
}

pub(crate) fn shift_alert_indices(ctx: &Context, offset: usize) {
    if let Some(selection) = ctx.get_data::<SelectionState>() {
        for index in &mut selection.borrow_mut().deferred_alerts {
            *index += offset;
        }
    }
}

fn drop_source(ctx: &mut Context) {
    if let Some(source) = ctx.remove_data::<Source>() {
        let host = source.1.borrow();
        if (host.reset_limit_exceeded || host.calculation_limit_exceeded)
            && let Some(selection) = ctx.get_data::<SelectionState>()
        {
            selection.borrow_mut().failure = Some("The selected PDF exceeded its script work limit");
        }
    }
}

pub(crate) fn browse(_: &JsValue, _: &[JsValue], key: &FieldKey, ctx: &mut Context) -> JsResult<JsValue> {
    if !primary(ctx) || ctx.get_data::<SelectionState>().is_none() {
        return Err(error("PDF selection requires an interactive document script"));
    }
    let shared = host(ctx)?;
    {
        let h = shared.borrow();
        if h.calculate_depth != 0 || !matches!(h.event_name.as_str(), "Console" | "Mouse Up") {
            return Err(error("PDF selection requires a button or console script"));
        }
    }
    if read(ctx, key)?.kind != crate::FieldType::Text {
        return Err(error("PDF selection requires a text field"));
    }
    let Some(selection) = ctx.get_data::<SelectionState>() else { return Err(error("PDF selector unavailable")) };
    let mut selection = selection.borrow_mut();
    if selection.opened {
        return Err(error("only one source PDF may be opened per script"));
    }
    selection.attempted = true;
    selection.field = None;
    selection.pending = None;
    selection.failure = None;
    match crate::interaction::select_pdf() {
        Ok(Some(pdf)) => {
            selection.field = Some(key.clone());
            selection.pending = Some(pdf);
            Ok(true.into())
        }
        Ok(None) => Ok(false.into()),
        Err(e) => {
            selection.failure = Some("The selected PDF could not be opened");
            Err(e)
        }
    }
}

pub(crate) fn field_path(ctx: &Context, key: &FieldKey) -> Option<JsValue> {
    if !primary(ctx) {
        return None;
    }
    ctx.get_data::<SelectionState>().filter(|sel| sel.borrow().field.as_ref() == Some(key)).map(|_| s(PATH))
}

pub(crate) fn clear_field_path(ctx: &mut Context, key: &FieldKey) {
    if primary(ctx)
        && let Some(selection) = ctx.get_data::<SelectionState>()
        && selection.borrow().field.as_ref() == Some(key)
    {
        selection.borrow_mut().field = None;
    }
}

pub(crate) fn open(path: &str, ctx: &mut Context) -> JsResult<Option<JsValue>> {
    if path != PATH {
        if ctx.get_data::<SelectionState>().is_some_and(|state| state.borrow().attempted) {
            return Err(error("openDoc requires the path of the selected PDF"));
        }
        return Ok(None);
    }
    if host(ctx)?.borrow().calculate_depth != 0 {
        return Err(error("field calculations cannot open the selected PDF"));
    }
    let pdf =
        ctx.get_data::<SelectionState>().and_then(|sel| sel.borrow_mut().pending.take()).ok_or_else(|| error("no selected PDF is available"))?;
    let opened = initialize(&pdf.0, ctx);
    if let Some(selection) = ctx.get_data::<SelectionState>() {
        let mut selection = selection.borrow_mut();
        selection.opened = opened.is_ok();
        if !selection.opened {
            selection.failure = Some("The selected PDF's document scripts could not be initialized");
        }
    }
    opened.map(Some)
}

fn initialize(job: &Job, ctx: &mut Context) -> JsResult<JsValue> {
    if crate::run::document_scripts_over_budget(&job.doc_scripts) {
        return Err(error("selected PDF document scripts exceed their budget"));
    }
    let scripts = job
        .doc_scripts
        .iter()
        .enumerate()
        .map(|(i, text)| match crate::run::refuse_document_script(text) {
            Some(why) => Err(error(why)),
            None => Ok((i, text.as_str())),
        })
        .collect::<JsResult<Vec<_>>>()?;
    let realm = ctx.create_realm()?;
    let previous = ctx.enter_realm(realm.clone());
    let destination = ctx.remove_data::<Shared>();
    // During setup the ordinary runner needs the source host in its own slot. Keep the
    // destination marker so any source API that could prompt or schedule work stays disabled.
    ctx.insert_data(Initializing(realm.clone()));
    let out = crate::run::run_in_context(
        ctx,
        &job.script,
        &crate::Event::doc("Open"),
        &job.doc,
        job.fields.clone(),
        crate::run::ScriptSetup {
            doc_scripts: &scripts,
            skipped: &[],
            calculations: &job.calculations,
            initialize: true,
            reset_calls: 0,
            calculation_evaluations: 0,
            calculation_limit_exceeded: false,
            call: None,
        },
        crate::Limits::default(),
    );
    ctx.remove_data::<Initializing>();
    let source = ctx.remove_data::<Shared>();
    let global = ctx.global_object();
    ctx.enter_realm(previous);
    if let Some(destination) = destination {
        ctx.insert_data(*destination);
    }
    if out.execution_refused || out.calculation_limit_exceeded || out.reset_limit_exceeded || out.error.is_some() || !out.doc_errors.is_empty() {
        return Err(error("selected PDF initialization failed"));
    }
    ctx.insert_data(Source(realm, *source.ok_or_else(|| error("selected PDF host unavailable"))?));
    Ok(global.into())
}

struct Initializing(Realm);

pub(crate) fn initializing(ctx: &Context) -> bool {
    ctx.get_data::<Initializing>().is_some_and(|source| &source.0 == ctx.realm())
}

pub(crate) fn close(this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    if primary(ctx) || !crate::host::arg(args, 0).to_boolean() || !this.as_object().is_some_and(|obj| JsObject::equals(&obj, &ctx.global_object())) {
        return Err(error("closeDoc requires discarding the selected source PDF"));
    }
    drop_source(ctx);
    Ok(JsValue::undefined())
}

pub(crate) fn install(ctx: &mut Context) -> JsResult<()> {
    ctx.register_global_builtin_callable(js_string!("closeDoc"), 1, crate::NativeFunction::from_fn_ptr(close))?;
    let tag = boa_engine::JsSymbol::to_string_tag();
    ctx.global_object().define_property_or_throw(tag, boa_engine::property::PropertyDescriptor::builder().value(s("Doc")).configurable(true), ctx)?;
    Ok(())
}
