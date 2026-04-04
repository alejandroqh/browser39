use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::dispatch::{CommandProcessor, ProcessResult};
use crate::service::service::BrowserService;

pub async fn run_batch(service: &mut BrowserService, input: &Path, output: &Path) -> Result<()> {
    let file = File::open(input)
        .with_context(|| format!("failed to open input file: {}", input.display()))?;
    let reader = BufReader::new(file);
    let mut processor = CommandProcessor::new(output, "batch");

    for line in reader.lines() {
        let line = line?;
        if matches!(
            processor.process_line(service, &line).await?,
            ProcessResult::Quit
        ) {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dispatch::test_helpers::write_commands;
    use crate::core::config::Config;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_batch_info_command() {
        let input = write_commands(&[r#"{"id":"a","action":"info","v":1,"seq":1}"#]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["seq"], 1);
        assert_eq!(result["alive"], true);
    }

    #[tokio::test]
    async fn test_batch_quit_stops_processing() {
        let input = write_commands(&[
            r#"{"id":"a","action":"info","v":1,"seq":1}"#,
            r#"{"id":"b","action":"quit","v":1,"seq":2}"#,
            r#"{"id":"c","action":"info","v":1,"seq":3}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let quit_result: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(quit_result["ok"], true);
        assert_eq!(quit_result["seq"], 2);
    }

    #[tokio::test]
    async fn test_batch_seq_validation() {
        let input = write_commands(&[
            r#"{"id":"a","action":"info","v":1,"seq":5}"#,
            r#"{"id":"b","action":"info","v":1,"seq":3}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["ok"], true);

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["ok"], false);
        assert_eq!(second["code"], "INVALID_COMMAND");
    }

    #[tokio::test]
    async fn test_batch_invalid_json() {
        let input = write_commands(&[
            "not valid json",
            r#"{"id":"a","action":"info","v":1,"seq":1}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["ok"], false);
        assert_eq!(first["code"], "INVALID_COMMAND");

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["ok"], true);
    }

    #[tokio::test]
    async fn test_batch_empty_lines_skipped() {
        let input = write_commands(&["", r#"{"id":"a","action":"info","v":1,"seq":1}"#, "", ""]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 1);
    }

    #[tokio::test]
    async fn test_batch_back_no_page_error() {
        let input = write_commands(&[r#"{"id":"a","action":"back","v":1,"seq":1}"#]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "NO_HISTORY");
    }

    #[tokio::test]
    async fn test_batch_links_no_page_error() {
        let input = write_commands(&[r#"{"id":"a","action":"links","v":1,"seq":1}"#]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "NO_PAGE");
    }

    #[tokio::test]
    async fn test_batch_fetch_missing_target() {
        let input = write_commands(&[r#"{"id":"a","action":"fetch","v":1,"seq":1}"#]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "INVALID_COMMAND");
    }

    #[tokio::test]
    async fn test_batch_config_fixed_delay() {
        let input = write_commands(&[
            r#"{"id":"cfg","action":"config","v":1,"seq":0,"step_delay":0.01}"#,
            r#"{"id":"a","action":"info","v":1,"seq":1}"#,
            r#"{"id":"b","action":"info","v":1,"seq":2}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        let start = std::time::Instant::now();
        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 3);

        let cfg_result: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(cfg_result["ok"], true);
        assert_eq!(cfg_result["status"], "configured");

        assert!(elapsed.as_millis() >= 20);
    }

    #[tokio::test]
    async fn test_batch_config_range_delay() {
        let input = write_commands(&[
            r#"{"id":"cfg","action":"config","v":1,"seq":0,"step_delay":[0.01,0.02]}"#,
            r#"{"id":"a","action":"info","v":1,"seq":1}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        let start = std::time::Instant::now();
        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        assert!(elapsed.as_millis() >= 10);
    }

    #[tokio::test]
    async fn test_batch_fill_no_page() {
        let input = write_commands(&[
            r##"{"id":"a","action":"fill","v":1,"seq":1,"selector":"#username","value":"agent"}"##,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "NO_PAGE");
    }

    #[tokio::test]
    async fn test_batch_fill_missing_fields() {
        let input = write_commands(&[r##"{"id":"a","action":"fill","v":1,"seq":1}"##]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "INVALID_COMMAND");
    }

    #[tokio::test]
    async fn test_batch_submit_no_page() {
        let input = write_commands(&[
            r##"{"id":"a","action":"submit","v":1,"seq":1,"selector":"form#login"}"##,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "NO_PAGE");
    }

    #[tokio::test]
    async fn test_batch_config_no_delay_on_config_itself() {
        let input =
            write_commands(&[r#"{"id":"cfg","action":"config","v":1,"seq":0,"step_delay":10}"#]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        let start = std::time::Instant::now();
        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed.as_millis() < 1000);
    }

    #[tokio::test]
    async fn test_batch_cookie_lifecycle() {
        let input = write_commands(&[
            // Set a cookie
            r#"{"id":"s1","action":"set_cookie","v":1,"seq":1,"name":"session","value":"tok123","domain":"example.com"}"#,
            // List cookies
            r#"{"id":"l1","action":"cookies","v":1,"seq":2}"#,
            // List with domain filter
            r#"{"id":"l2","action":"cookies","v":1,"seq":3,"domain":"example.com"}"#,
            // List with non-matching domain
            r#"{"id":"l3","action":"cookies","v":1,"seq":4,"domain":"other.com"}"#,
            // Delete the cookie
            r#"{"id":"d1","action":"delete_cookie","v":1,"seq":5,"name":"session","domain":"example.com"}"#,
            // Verify empty
            r#"{"id":"l4","action":"cookies","v":1,"seq":6}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let results: Vec<serde_json::Value> = contents
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        // set_cookie succeeded
        assert_eq!(results[0]["ok"], true);
        assert_eq!(results[0]["name"], "session");
        assert_eq!(results[0]["domain"], "example.com");

        // list all: 1 cookie
        assert_eq!(results[1]["ok"], true);
        assert_eq!(results[1]["count"], 1);
        assert_eq!(results[1]["cookies"][0]["name"], "session");
        assert_eq!(results[1]["cookies"][0]["value"], "tok123");

        // list filtered by matching domain: 1 cookie
        assert_eq!(results[2]["ok"], true);
        assert_eq!(results[2]["count"], 1);

        // list filtered by non-matching domain: 0 cookies
        assert_eq!(results[3]["ok"], true);
        assert_eq!(results[3]["count"], 0);

        // delete succeeded
        assert_eq!(results[4]["ok"], true);
        assert_eq!(results[4]["deleted"], true);

        // list after delete: 0 cookies
        assert_eq!(results[5]["ok"], true);
        assert_eq!(results[5]["count"], 0);
    }

    #[tokio::test]
    async fn test_batch_delete_nonexistent_cookie() {
        let input = write_commands(&[
            r#"{"id":"d1","action":"delete_cookie","v":1,"seq":1,"name":"nope","domain":"example.com"}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["deleted"], false);
    }

    #[tokio::test]
    async fn test_batch_storage_lifecycle() {
        let input = write_commands(&[
            // Set a value with explicit origin
            r#"{"id":"s1","action":"storage_set","v":1,"seq":1,"key":"token","value":"abc123","origin":"https://app.example.com"}"#,
            // Get it back
            r#"{"id":"g1","action":"storage_get","v":1,"seq":2,"key":"token","origin":"https://app.example.com"}"#,
            // Get non-existent key
            r#"{"id":"g2","action":"storage_get","v":1,"seq":3,"key":"missing","origin":"https://app.example.com"}"#,
            // Set another key on same origin
            r#"{"id":"s2","action":"storage_set","v":1,"seq":4,"key":"theme","value":"dark","origin":"https://app.example.com"}"#,
            // List entries for origin
            r#"{"id":"l1","action":"storage_list","v":1,"seq":5,"origin":"https://app.example.com"}"#,
            // Delete one key
            r#"{"id":"d1","action":"storage_delete","v":1,"seq":6,"key":"token","origin":"https://app.example.com"}"#,
            // List again — should have 1 entry
            r#"{"id":"l2","action":"storage_list","v":1,"seq":7,"origin":"https://app.example.com"}"#,
            // Clear remaining
            r#"{"id":"c1","action":"storage_clear","v":1,"seq":8,"origin":"https://app.example.com"}"#,
            // List after clear — should be empty
            r#"{"id":"l3","action":"storage_list","v":1,"seq":9,"origin":"https://app.example.com"}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let results: Vec<serde_json::Value> = contents
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(results.len(), 9);

        // s1: storage_set returns the set value
        assert_eq!(results[0]["ok"], true);
        assert_eq!(results[0]["key"], "token");
        assert_eq!(results[0]["value"], "abc123");

        // g1: storage_get returns the value
        assert_eq!(results[1]["ok"], true);
        assert_eq!(results[1]["key"], "token");
        assert_eq!(results[1]["value"], "abc123");

        // g2: storage_get for missing key returns null value
        assert_eq!(results[2]["ok"], true);
        assert_eq!(results[2]["key"], "missing");
        assert_eq!(results[2]["value"], serde_json::Value::Null);

        // l1: storage_list returns 2 entries
        assert_eq!(results[4]["ok"], true);
        assert_eq!(results[4]["origin"], "https://app.example.com");
        assert_eq!(results[4]["count"], 2);
        assert_eq!(results[4]["entries"]["token"], "abc123");
        assert_eq!(results[4]["entries"]["theme"], "dark");

        // d1: storage_delete succeeds
        assert_eq!(results[5]["ok"], true);
        assert_eq!(results[5]["deleted"], true);

        // l2: storage_list after delete has 1 entry
        assert_eq!(results[6]["ok"], true);
        assert_eq!(results[6]["count"], 1);
        assert_eq!(results[6]["entries"]["theme"], "dark");

        // c1: storage_clear cleared 1 entry
        assert_eq!(results[7]["ok"], true);
        assert_eq!(results[7]["cleared"], 1);

        // l3: storage_list after clear is empty
        assert_eq!(results[8]["ok"], true);
        assert_eq!(results[8]["count"], 0);
    }

    #[tokio::test]
    async fn test_batch_storage_cross_origin_isolation() {
        let input = write_commands(&[
            // Set on origin A
            r#"{"id":"s1","action":"storage_set","v":1,"seq":1,"key":"data","value":"origin-a","origin":"https://a.example.com"}"#,
            // Set same key on origin B
            r#"{"id":"s2","action":"storage_set","v":1,"seq":2,"key":"data","value":"origin-b","origin":"https://b.example.com"}"#,
            // Get from origin A
            r#"{"id":"g1","action":"storage_get","v":1,"seq":3,"key":"data","origin":"https://a.example.com"}"#,
            // Get from origin B
            r#"{"id":"g2","action":"storage_get","v":1,"seq":4,"key":"data","origin":"https://b.example.com"}"#,
            // List origin A
            r#"{"id":"l1","action":"storage_list","v":1,"seq":5,"origin":"https://a.example.com"}"#,
            // Clear origin A
            r#"{"id":"c1","action":"storage_clear","v":1,"seq":6,"origin":"https://a.example.com"}"#,
            // Origin B still has its data
            r#"{"id":"g3","action":"storage_get","v":1,"seq":7,"key":"data","origin":"https://b.example.com"}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let results: Vec<serde_json::Value> = contents
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(results.len(), 7);

        // Origins are isolated
        assert_eq!(results[2]["value"], "origin-a");
        assert_eq!(results[3]["value"], "origin-b");

        // Clearing A doesn't affect B
        assert_eq!(results[5]["cleared"], 1);
        assert_eq!(results[6]["value"], "origin-b");
    }

    #[tokio::test]
    async fn test_batch_storage_delete_nonexistent() {
        let input = write_commands(&[
            r#"{"id":"d1","action":"storage_delete","v":1,"seq":1,"key":"nope","origin":"https://example.com"}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["deleted"], false);
    }

    #[tokio::test]
    async fn test_batch_storage_no_origin_no_page() {
        // When no origin is provided and no page is loaded, should error
        let input =
            write_commands(&[r#"{"id":"g1","action":"storage_get","v":1,"seq":1,"key":"test"}"#]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let config = Config::default();
        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let result: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["code"], "NO_PAGE");
    }

    #[tokio::test]
    async fn test_batch_storage_preloaded_from_config() {
        use crate::core::config::StorageConfig;

        let mut config = Config::default();
        config.storage.push(StorageConfig {
            origin: "https://app.example.com".into(),
            key: "api_token".into(),
            value: Some("preloaded-value".into()),
            value_env: None,
            sensitive: false,
            resolved_value: Some("preloaded-value".into()),
        });

        let input = write_commands(&[
            r#"{"id":"g1","action":"storage_get","v":1,"seq":1,"key":"api_token","origin":"https://app.example.com"}"#,
            r#"{"id":"l1","action":"storage_list","v":1,"seq":2,"origin":"https://app.example.com"}"#,
        ]);
        let output = NamedTempFile::new().unwrap();
        let output_path = output.path().to_path_buf();
        drop(output);

        let mut service =
            BrowserService::new(config, Box::new(crate::core::session_store::InMemoryStore))
                .await
                .unwrap();

        run_batch(&mut service, input.path(), &output_path)
            .await
            .unwrap();

        let contents = std::fs::read_to_string(&output_path).unwrap();
        let results: Vec<serde_json::Value> = contents
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        // Preloaded value is accessible
        assert_eq!(results[0]["ok"], true);
        assert_eq!(results[0]["value"], "preloaded-value");

        // Shows up in list
        assert_eq!(results[1]["ok"], true);
        assert_eq!(results[1]["count"], 1);
        assert_eq!(results[1]["entries"]["api_token"], "preloaded-value");
    }
}
