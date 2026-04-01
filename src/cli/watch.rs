use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use notify::{Config as NotifyConfig, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::cli::dispatch::{clog, CommandProcessor, ProcessResult};
use crate::service::service::BrowserService;

macro_rules! wlog {
    ($($arg:tt)*) => {
        clog!("watch", $($arg)*)
    };
}

/// Read complete lines appended since `offset`. Partial lines (no trailing
/// newline) are left unconsumed so they can be completed on the next read.
fn read_new_lines(path: &Path, offset: &mut u64) -> Result<Vec<String>> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to open input file: {}", path.display()))?;
    let end = file.seek(SeekFrom::End(0))?;
    if end <= *offset {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(*offset))?;
    let mut buf = vec![0u8; (end - *offset) as usize];
    file.read_exact(&mut buf)?;

    let last_newline = match buf.iter().rposition(|&b| b == b'\n') {
        Some(pos) => pos,
        None => return Ok(Vec::new()),
    };

    let complete = &buf[..last_newline + 1];
    *offset += complete.len() as u64;

    let text = String::from_utf8_lossy(complete);
    Ok(text.lines().map(String::from).collect())
}

pub async fn run_watch(
    service: &mut BrowserService,
    input: &Path,
    output: &Path,
) -> Result<()> {
    if !input.exists() {
        anyhow::bail!(
            "input file does not exist: {} (create it with `touch` first)",
            input.display()
        );
    }

    let input_canonical = input
        .canonicalize()
        .with_context(|| format!("failed to canonicalize input path: {}", input.display()))?;

    let (tx, mut rx) = mpsc::channel::<notify::Result<notify::Event>>(100);

    // Watcher must stay alive for the duration of this function; dropping it stops events.
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            let _ = tx.blocking_send(res);
        },
        NotifyConfig::default(),
    )
    .context("failed to create file watcher")?;

    let watch_dir = input
        .parent()
        .unwrap_or(Path::new("."))
        .canonicalize()
        .unwrap_or_else(|_| input.parent().unwrap_or(Path::new(".")).to_path_buf());

    watcher
        .watch(&watch_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("failed to watch directory: {}", watch_dir.display()))?;

    let mut offset: u64 = 0;
    let mut processor = CommandProcessor::new(output, "watch");

    wlog!("watching {}", input.display());

    let lines = read_new_lines(&input_canonical, &mut offset)?;
    if !lines.is_empty() {
        wlog!("processing {} existing line(s)", lines.len());
        for line in &lines {
            if matches!(
                processor.process_line(service, line).await?,
                ProcessResult::Quit
            ) {
                return Ok(());
            }
        }
    }

    while let Some(event_result) = rx.recv().await {
        let event = match event_result {
            Ok(e) => e,
            Err(e) => {
                wlog!("watcher error: {e}");
                continue;
            }
        };

        if !matches!(event.kind, EventKind::Modify(_)) {
            continue;
        }
        let is_our_file = event
            .paths
            .iter()
            .any(|p| p.canonicalize().ok().as_ref() == Some(&input_canonical));
        if !is_our_file {
            continue;
        }

        let lines = read_new_lines(&input_canonical, &mut offset)?;
        for line in &lines {
            if matches!(
                processor.process_line(service, line).await?,
                ProcessResult::Quit
            ) {
                return Ok(());
            }
        }
    }

    wlog!("watcher channel closed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dispatch::test_helpers::write_commands;
    use crate::core::config::Config;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_watch_processes_existing_content() {
        let input = write_commands(&[
            r#"{"id":"a","action":"info","v":1,"seq":1}"#,
            r#"{"id":"b","action":"quit","v":1,"seq":2}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service = BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore)).await.unwrap();

        run_watch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["ok"], true);
        assert_eq!(first["seq"], 1);

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["ok"], true);
        assert_eq!(second["seq"], 2);
        assert_eq!(second["status"], "quit");
    }

    #[tokio::test]
    async fn test_watch_quit_exits_cleanly() {
        let input =
            write_commands(&[r#"{"id":"q","action":"quit","v":1,"seq":1}"#]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service = BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore)).await.unwrap();

        run_watch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["status"], "quit");
    }

    #[tokio::test]
    async fn test_watch_empty_lines_skipped() {
        let input = write_commands(&[
            "",
            r#"{"id":"a","action":"info","v":1,"seq":1}"#,
            "",
            r#"{"id":"b","action":"quit","v":1,"seq":2}"#,
            "",
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service = BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore)).await.unwrap();

        run_watch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[tokio::test]
    async fn test_watch_seq_validation() {
        let input = write_commands(&[
            r#"{"id":"a","action":"info","v":1,"seq":5}"#,
            r#"{"id":"b","action":"info","v":1,"seq":3}"#,
            r#"{"id":"c","action":"info","v":1,"seq":6}"#,
            r#"{"id":"d","action":"quit","v":1,"seq":7}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service = BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore)).await.unwrap();

        run_watch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 4);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["ok"], true);

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["ok"], false);
        assert_eq!(second["code"], "INVALID_COMMAND");

        let third: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(third["ok"], true);

        let fourth: serde_json::Value = serde_json::from_str(lines[3]).unwrap();
        assert_eq!(fourth["ok"], true);
        assert_eq!(fourth["status"], "quit");
    }

    #[tokio::test]
    async fn test_watch_appended_commands() {
        let input = NamedTempFile::new().unwrap();
        let input_path = input.path().to_path_buf();
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service = BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore)).await.unwrap();

        let watch_input = input_path.clone();
        let watch_output = output_path.clone();
        let handle = tokio::spawn(async move {
            run_watch(&mut service, &watch_input, &watch_output).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        {
            use std::fs::OpenOptions;
            let mut f = OpenOptions::new()
                .append(true)
                .open(&input_path)
                .unwrap();
            writeln!(f, r#"{{"id":"a","action":"info","v":1,"seq":1}}"#).unwrap();
            writeln!(f, r#"{{"id":"b","action":"quit","v":1,"seq":2}}"#).unwrap();
            f.flush().unwrap();
            f.sync_all().unwrap();
        }

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(result.is_ok(), "watch did not exit within timeout");
        result.unwrap().unwrap().unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["ok"], true);
        assert_eq!(first["seq"], 1);

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["ok"], true);
        assert_eq!(second["status"], "quit");
    }
}
