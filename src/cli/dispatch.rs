use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rand::Rng;
use serde::Serialize;

use crate::cli::protocol::{
    Action, CommandEnvelope, ConfigAction, CookiesAction, DeleteCookieAction, DomQueryAction,
    FetchAction, FillAction, ResultEnvelope, SetCookieAction, StepDelay, StorageClearAction,
    StorageDeleteAction, StorageGetAction, StorageListAction, StorageSetAction, SubmitAction,
};
use crate::core::error::ErrorCode;
use crate::core::page::{FetchMode, FetchOptions};
use crate::service::service::{BrowserService, classify_error};

pub(crate) const ROTATION_SIZE: u64 = 10 * 1024 * 1024; // 10MB
pub(crate) const ROTATION_CHECK_BYTES: u64 = 100_000; // Check rotation every ~100KB

pub(crate) fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs = now % 60;
    let mins = (now / 60) % 60;
    let hrs = (now / 3600) % 24;
    let days = now / 86400;
    let (y, m, d) = days_to_ymd(days);
    format!("{y:04}{m:02}{d:02} {hrs:02}:{mins:02}:{secs:02}")
}

pub(crate) fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let era_days = days + 719468;
    let era = era_days / 146097;
    let doe = era_days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

macro_rules! clog {
    ($prefix:expr, $($arg:tt)*) => {
        eprintln!("[{} {}] {}", $prefix, $crate::cli::dispatch::timestamp(), format_args!($($arg)*))
    };
}
pub(crate) use clog;

fn action_label(action: &Action) -> String {
    match action {
        Action::Config(_) => "config".into(),
        Action::Fetch(f) => match f.resolve_mode() {
            Some(FetchMode::Url(url)) => format!("fetch {url}"),
            Some(FetchMode::Index(i)) => format!("fetch [{}]", i),
            Some(FetchMode::Text(t)) => format!("fetch \"{t}\""),
            None => "fetch (invalid)".into(),
        },
        Action::Links => "links".into(),
        Action::Back => "back".into(),
        Action::Forward => "forward".into(),
        Action::Info => "info".into(),
        Action::Quit => "quit".into(),
        Action::DomQuery(_) => "dom_query".into(),
        Action::Fill(_) => "fill".into(),
        Action::Submit(_) => "submit".into(),
        Action::Cookies(_) => "cookies".into(),
        Action::SetCookie(_) => "set_cookie".into(),
        Action::DeleteCookie(_) => "delete_cookie".into(),
        Action::StorageGet(_) => "storage_get".into(),
        Action::StorageSet(_) => "storage_set".into(),
        Action::StorageDelete(_) => "storage_delete".into(),
        Action::StorageList(_) => "storage_list".into(),
        Action::StorageClear(_) => "storage_clear".into(),
        Action::History(_) => "history".into(),
    }
}

#[derive(Default)]
struct RunConfig {
    step_delay: Option<StepDelay>,
}

impl RunConfig {
    fn apply(&mut self, config: &ConfigAction) {
        self.step_delay = config.step_delay;
    }

    async fn delay(&self) {
        let secs = match self.step_delay {
            Some(StepDelay::Fixed(s)) => s,
            Some(StepDelay::Range(min, max)) => rand::rng().random_range(min..=max),
            None => return,
        };
        if secs > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(secs)).await;
        }
    }
}

pub(crate) enum ProcessResult {
    Continue,
    Quit,
}

/// Encapsulates per-line command processing state shared by batch and watch modes.
pub(crate) struct CommandProcessor<'a> {
    last_seq: Option<u64>,
    line_num: usize,
    run_config: RunConfig,
    writer: ResultWriter<'a>,
    label: &'static str,
}

impl<'a> CommandProcessor<'a> {
    pub(crate) fn new(output: &'a Path, label: &'static str) -> Self {
        Self {
            last_seq: None,
            line_num: 0,
            run_config: RunConfig::default(),
            writer: ResultWriter::new(output),
            label,
        }
    }

    pub(crate) async fn process_line(
        &mut self,
        service: &mut BrowserService,
        line: &str,
    ) -> Result<ProcessResult> {
        self.line_num += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(ProcessResult::Continue);
        }

        let envelope: CommandEnvelope = match serde_json::from_str(trimmed) {
            Ok(cmd) => cmd,
            Err(e) => {
                clog!(self.label, "line {}: parse error: {e}", self.line_num);
                let result = ResultEnvelope::error(
                    String::new(),
                    self.last_seq.map(|s| s + 1).unwrap_or(0),
                    ErrorCode::InvalidCommand,
                    format!("line {}: {e}", self.line_num),
                );
                self.writer.write(&result)?;
                return Ok(ProcessResult::Continue);
            }
        };

        if let Some(prev) = self.last_seq
            && envelope.seq <= prev
        {
            clog!(
                self.label,
                "seq {}: rejected (must be > {prev})",
                envelope.seq
            );
            let result = ResultEnvelope::error(
                envelope.id.clone(),
                envelope.seq,
                ErrorCode::InvalidCommand,
                format!(
                    "seq {} must be greater than previous seq {prev}",
                    envelope.seq
                ),
            );
            self.writer.write(&result)?;
            return Ok(ProcessResult::Continue);
        }
        self.last_seq = Some(envelope.seq);

        let label = action_label(&envelope.action);

        if let Action::Config(ref cfg) = envelope.action {
            self.run_config.apply(cfg);
            clog!(self.label, "seq {}: {label}", envelope.seq);
            let result = ResultEnvelope::success(
                envelope.id,
                envelope.seq,
                serde_json::json!({"status": "configured"}),
            )?;
            self.writer.write(&result)?;
            return Ok(ProcessResult::Continue);
        }

        if matches!(envelope.action, Action::Quit) {
            clog!(self.label, "seq {}: {label}", envelope.seq);
            let result = ResultEnvelope::success(
                envelope.id,
                envelope.seq,
                serde_json::json!({"status": "quit"}),
            )?;
            self.writer.write(&result)?;
            return Ok(ProcessResult::Quit);
        }

        self.run_config.delay().await;

        clog!(self.label, "seq {}: {label}", envelope.seq);
        let result = dispatch(service, &envelope).await;
        if result.ok {
            clog!(self.label, "seq {}: ok", envelope.seq);
        } else {
            clog!(
                self.label,
                "seq {}: error [{}] {}",
                envelope.seq,
                result
                    .code
                    .as_ref()
                    .map(|c| format!("{c:?}"))
                    .unwrap_or_default(),
                result.error.as_deref().unwrap_or("")
            );
        }
        self.writer.write(&result)?;
        Ok(ProcessResult::Continue)
    }
}

async fn dispatch(
    service: &mut BrowserService,
    cmd: &CommandEnvelope,
) -> ResultEnvelope {
    let id = cmd.id.clone();
    let seq = cmd.seq;

    match &cmd.action {
        Action::Fetch(fetch) => dispatch_fetch(service, &id, seq, fetch).await,
        Action::Links => wrap_result(id, seq, service.links()),
        Action::Back => wrap_result(id, seq, service.back()),
        Action::Forward => wrap_result(id, seq, service.forward()),
        Action::Info => wrap_result(id, seq, Ok(service.info())),
        Action::DomQuery(dq) => dispatch_dom_query(service, &id, seq, dq),
        Action::Fill(fill) => dispatch_fill(service, &id, seq, fill),
        Action::Submit(submit) => dispatch_submit(service, &id, seq, submit).await,
        Action::Cookies(c) => dispatch_cookies(service, &id, seq, c),
        Action::SetCookie(sc) => dispatch_set_cookie(service, &id, seq, sc),
        Action::DeleteCookie(dc) => dispatch_delete_cookie(service, &id, seq, dc),
        Action::StorageGet(a) => dispatch_storage_get(service, &id, seq, a),
        Action::StorageSet(a) => dispatch_storage_set(service, &id, seq, a),
        Action::StorageDelete(a) => dispatch_storage_delete(service, &id, seq, a),
        Action::StorageList(a) => dispatch_storage_list(service, &id, seq, a),
        Action::StorageClear(a) => dispatch_storage_clear(service, &id, seq, a),
        Action::History(a) => {
            let limit = a.limit.unwrap_or(10);
            wrap_result(id, seq, Ok(service.history(a.query.as_deref(), limit)))
        }
        Action::Config(_) | Action::Quit => unreachable!(),
    }
}

async fn dispatch_fetch(
    service: &mut BrowserService,
    id: &str,
    seq: u64,
    fetch: &FetchAction,
) -> ResultEnvelope {
    let mode = match fetch.resolve_mode() {
        Some(m) => m,
        None => {
            return ResultEnvelope::error(
                id.into(),
                seq,
                ErrorCode::InvalidCommand,
                "fetch requires url, index, or text".into(),
            );
        }
    };

    let opts = service.merge_options(&fetch.options);
    let result = match mode {
        FetchMode::Url(url) => {
            service
                .fetch(
                    &url,
                    &fetch.method,
                    &fetch.headers,
                    fetch.body.clone(),
                    fetch.auth_profile.as_deref(),
                    &opts,
                )
                .await
        }
        FetchMode::Index(index) => {
            service
                .fetch_by_index(
                    index,
                    &fetch.method,
                    &fetch.headers,
                    fetch.body.clone(),
                    fetch.auth_profile.as_deref(),
                    &opts,
                )
                .await
        }
        FetchMode::Text(text) => {
            service
                .fetch_by_text(
                    &text,
                    &fetch.method,
                    &fetch.headers,
                    fetch.body.clone(),
                    fetch.auth_profile.as_deref(),
                    &opts,
                )
                .await
        }
    };

    match result {
        Ok(page) => ResultEnvelope::success(id.into(), seq, &page).unwrap_or_else(|e| {
            ResultEnvelope::error(id.into(), seq, ErrorCode::SessionError, e.to_string())
        }),
        Err(e) => to_error_result(id.into(), seq, e),
    }
}

fn dispatch_dom_query(
    service: &mut BrowserService,
    id: &str,
    seq: u64,
    dq: &DomQueryAction,
) -> ResultEnvelope {
    if let Some(ref selector) = dq.selector {
        let attr = dq.attr.as_deref().unwrap_or("textContent");
        wrap_result(id.into(), seq, service.dom_query(selector, attr))
    } else if let Some(ref script) = dq.script {
        wrap_result(id.into(), seq, service.dom_script(script))
    } else {
        ResultEnvelope::error(
            id.into(),
            seq,
            ErrorCode::InvalidCommand,
            "dom_query requires selector or script".into(),
        )
    }
}

fn dispatch_fill(
    service: &mut BrowserService,
    id: &str,
    seq: u64,
    fill: &FillAction,
) -> ResultEnvelope {
    // Build field list from either single selector/value or fields array
    let fields: Vec<(String, String)> = if let Some(ref fields_vec) = fill.fields {
        fields_vec
            .iter()
            .map(|f| (f.selector.clone(), f.value.clone()))
            .collect()
    } else if let (Some(selector), Some(value)) = (&fill.selector, &fill.value) {
        vec![(selector.clone(), value.clone())]
    } else {
        return ResultEnvelope::error(
            id.into(),
            seq,
            ErrorCode::InvalidCommand,
            "fill requires either selector+value or fields array".into(),
        );
    };

    wrap_result(id.into(), seq, service.fill(&fields))
}

async fn dispatch_submit(
    service: &mut BrowserService,
    id: &str,
    seq: u64,
    submit: &SubmitAction,
) -> ResultEnvelope {
    let opts = FetchOptions {
        max_tokens: submit.max_tokens,
        ..service.default_fetch_options()
    };
    let result = service.submit(&submit.selector, &opts).await;
    match result {
        Ok(page) => ResultEnvelope::success(id.into(), seq, &page).unwrap_or_else(|e| {
            ResultEnvelope::error(id.into(), seq, ErrorCode::SessionError, e.to_string())
        }),
        Err(e) => to_error_result(id.into(), seq, e),
    }
}

fn dispatch_cookies(
    service: &BrowserService,
    id: &str,
    seq: u64,
    action: &CookiesAction,
) -> ResultEnvelope {
    wrap_result(
        id.into(),
        seq,
        service.cookies(action.domain.as_deref()),
    )
}

fn dispatch_set_cookie(
    service: &mut BrowserService,
    id: &str,
    seq: u64,
    action: &SetCookieAction,
) -> ResultEnvelope {
    wrap_result(
        id.into(),
        seq,
        service.set_cookie(
            &action.name,
            &action.value,
            &action.domain,
            action.path.as_deref().unwrap_or("/"),
            action.secure,
            action.http_only,
            action.max_age_secs,
        ),
    )
}

fn dispatch_delete_cookie(
    service: &mut BrowserService,
    id: &str,
    seq: u64,
    action: &DeleteCookieAction,
) -> ResultEnvelope {
    wrap_result(
        id.into(),
        seq,
        service.delete_cookie(&action.name, &action.domain),
    )
}

fn dispatch_storage_get(
    service: &BrowserService,
    id: &str,
    seq: u64,
    action: &StorageGetAction,
) -> ResultEnvelope {
    wrap_result(
        id.into(),
        seq,
        service.storage_get(&action.key, action.origin.as_deref()),
    )
}

fn dispatch_storage_set(
    service: &mut BrowserService,
    id: &str,
    seq: u64,
    action: &StorageSetAction,
) -> ResultEnvelope {
    wrap_result(
        id.into(),
        seq,
        service.storage_set(&action.key, &action.value, action.origin.as_deref()),
    )
}

fn dispatch_storage_delete(
    service: &mut BrowserService,
    id: &str,
    seq: u64,
    action: &StorageDeleteAction,
) -> ResultEnvelope {
    wrap_result(
        id.into(),
        seq,
        service.storage_delete(&action.key, action.origin.as_deref()),
    )
}

fn dispatch_storage_list(
    service: &BrowserService,
    id: &str,
    seq: u64,
    action: &StorageListAction,
) -> ResultEnvelope {
    wrap_result(
        id.into(),
        seq,
        service.storage_list(action.origin.as_deref()),
    )
}

fn dispatch_storage_clear(
    service: &mut BrowserService,
    id: &str,
    seq: u64,
    action: &StorageClearAction,
) -> ResultEnvelope {
    wrap_result(
        id.into(),
        seq,
        service.storage_clear(action.origin.as_deref()),
    )
}

fn wrap_result<T: Serialize>(
    id: String,
    seq: u64,
    result: anyhow::Result<T>,
) -> ResultEnvelope {
    match result {
        Ok(data) => ResultEnvelope::success(id.clone(), seq, &data).unwrap_or_else(|e| {
            ResultEnvelope::error(id, seq, ErrorCode::SessionError, e.to_string())
        }),
        Err(e) => to_error_result(id, seq, e),
    }
}

fn to_error_result(id: String, seq: u64, err: anyhow::Error) -> ResultEnvelope {
    let se = classify_error(err);
    ResultEnvelope::error(id, seq, se.code, se.message)
}

pub(crate) struct ResultWriter<'a> {
    path: &'a Path,
    bytes_since_check: u64,
}

impl<'a> ResultWriter<'a> {
    pub(crate) fn new(path: &'a Path) -> Self {
        Self {
            path,
            bytes_since_check: 0,
        }
    }

    pub(crate) fn write(&mut self, result: &ResultEnvelope) -> Result<()> {
        if self.bytes_since_check >= ROTATION_CHECK_BYTES {
            self.maybe_rotate()?;
            self.bytes_since_check = 0;
        }

        let line = serde_json::to_string(result)?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path)
            .with_context(|| format!("failed to open output file: {}", self.path.display()))?;

        writeln!(file, "{line}")?;
        file.flush()?;
        file.sync_all()?;

        self.bytes_since_check += line.len() as u64;

        Ok(())
    }

    fn maybe_rotate(&self) -> Result<()> {
        let meta = match fs::metadata(self.path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        if meta.len() < ROTATION_SIZE {
            return Ok(());
        }

        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("results");
        let ext = self
            .path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("jsonl");
        let parent = self.path.parent().unwrap_or(Path::new("."));

        let mut n = 1u32;
        loop {
            let rotated = parent.join(format!("{stem}.{n}.{ext}"));
            if !rotated.exists() {
                fs::rename(self.path, &rotated).with_context(|| {
                    format!(
                        "failed to rotate {} -> {}",
                        self.path.display(),
                        rotated.display()
                    )
                })?;
                break;
            }
            n += 1;
        }

        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::io::Write;
    use tempfile::NamedTempFile;

    pub(crate) fn write_commands(lines: &[&str]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        f.flush().unwrap();
        f
    }
}
