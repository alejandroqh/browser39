use std::collections::HashMap;
use std::process;

use clap::Parser;

use browser39::cli::args::{Cli, Commands, McpTransport, OutputFormat};
use browser39::cli::batch::run_batch;
use browser39::cli::watch::run_watch;
use browser39::core::config::{Config, PersistenceMode};
use browser39::core::page::{FetchOptions, HttpMethod};
use browser39::core::session_store::{self, InMemoryStore};
use browser39::mcp;
use browser39::service::service::BrowserService;

fn create_store(config: &Config) -> Box<dyn session_store::SessionStore> {
    match session_store::create_session_store(
        &config.session.persistence,
        config.session.session_path.as_deref(),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error creating session store: {e}");
            process::exit(1);
        }
    }
}

// Clearing start_url avoids an auto-fetch on init that the one-shot
// caller would immediately follow with its own fetch.
async fn make_one_shot_service(mut config: Config) -> BrowserService {
    config.session.start_url = None;
    let store: Box<dyn session_store::SessionStore> = Box::new(InMemoryStore);
    match BrowserService::new(config, store).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error initializing service: {e}");
            process::exit(1);
        }
    }
}

async fn fetch_and_print(
    service: &mut BrowserService,
    url: &str,
    mut options: FetchOptions,
    output: OutputFormat,
    error_context: &str,
) {
    if matches!(output, OutputFormat::Json) {
        options.compact_links = true;
    }
    let result = match service
        .fetch(url, &HttpMethod::Get, &HashMap::new(), None, None, &options)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error {error_context}: {e}");
            process::exit(1);
        }
    };
    match output {
        OutputFormat::Text => println!("{}", result.markdown),
        OutputFormat::Json => match serde_json::to_string_pretty(&result) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("Error serializing result: {e}");
                process::exit(1);
            }
        },
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let mut config = match Config::load(cli.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading config: {e}");
            process::exit(1);
        }
    };

    if cli.no_persist {
        config.session.persistence = PersistenceMode::Memory;
    }

    match cli.command {
        Commands::Fetch { url, output } => {
            let mut service = make_one_shot_service(config).await;
            let options = service.default_fetch_options();
            let ctx = format!("fetching {url}");
            fetch_and_print(&mut service, &url, options, output, &ctx).await;
        }
        Commands::Search { query, output } => {
            let mut service = make_one_shot_service(config).await;
            let url = service.search_url(&query);
            let mut options = service.default_fetch_options();
            options.show_selectors_first = false;
            let ctx = format!("searching for {query:?}");
            fetch_and_print(&mut service, &url, options, output, &ctx).await;
        }
        Commands::Batch { input, output } => {
            // Batch mode: always in-memory
            let store: Box<dyn session_store::SessionStore> = Box::new(InMemoryStore);
            let mut service = match BrowserService::new(config, store).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error initializing service: {e}");
                    process::exit(1);
                }
            };

            if let Err(e) = run_batch(&mut service, &input, &output).await {
                eprintln!("Error running batch: {e}");
                process::exit(1);
            }
        }
        Commands::Watch { input, output } => {
            let store = create_store(&config);
            let mut service = match BrowserService::new(config, store).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error initializing service: {e}");
                    process::exit(1);
                }
            };

            if let Err(e) = run_watch(&mut service, &input, &output).await {
                eprintln!("Error running watch: {e}");
                process::exit(1);
            }
        }
        Commands::Mcp { transport, port } => match transport {
            McpTransport::Stdio => {
                let store = create_store(&config);
                if let Err(e) = mcp::run_mcp_stdio(config, store).await {
                    eprintln!("MCP server error: {e}");
                    process::exit(1);
                }
            }
            McpTransport::Sse => {
                if let Err(e) = mcp::run_mcp_sse(config, port).await {
                    eprintln!("MCP SSE server error: {e}");
                    process::exit(1);
                }
            }
        },
    }
}
