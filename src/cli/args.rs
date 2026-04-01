use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "browser39", about = "Web browser for AI agents")]
pub struct Cli {
    /// Path to config file (overrides BROWSER39_CONFIG and default)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Disable disk persistence (in-memory session only)
    #[arg(long, alias = "only-memory", global = true)]
    pub no_persist: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Fetch a URL and output as markdown or JSON
    Fetch {
        /// URL to fetch
        url: String,

        /// Output format
        #[arg(long, default_value = "text", value_enum)]
        output: OutputFormat,
    },

    /// Process commands from a JSONL file and write results to a JSONL file
    Batch {
        /// Path to input commands JSONL file
        input: PathBuf,

        /// Path to output results JSONL file
        #[arg(long, default_value = "results.jsonl")]
        output: PathBuf,
    },

    /// Watch a commands JSONL file for new lines and process them
    Watch {
        /// Path to input commands JSONL file (must exist)
        input: PathBuf,

        /// Path to output results JSONL file
        #[arg(long, default_value = "results.jsonl")]
        output: PathBuf,
    },

    /// Start MCP (Model Context Protocol) server
    Mcp {
        /// Transport type
        #[arg(long, default_value = "stdio", value_enum)]
        transport: McpTransport,

        /// Port for SSE transport
        #[arg(long, default_value_t = 8039)]
        port: u16,
    },
}

#[derive(Debug, Clone, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum McpTransport {
    Stdio,
    Sse,
}
