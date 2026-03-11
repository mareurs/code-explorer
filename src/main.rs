use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "codescout", about = "High-performance coding agent MCP server")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server
    Start {
        /// Project root path to activate on startup
        #[arg(short, long)]
        project: Option<std::path::PathBuf>,

        /// Transport mode
        #[arg(long, default_value = "stdio", value_parser = ["stdio", "http"])]
        transport: String,

        /// Listen address (HTTP transport only)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Listen port (HTTP transport only)
        #[arg(long, default_value_t = 8090)]
        port: u16,

        /// Bearer token for HTTP transport authentication.
        /// If not provided when using HTTP transport, a token is auto-generated.
        #[arg(long)]
        auth_token: Option<String>,

        /// Enable debug logging to .codescout/debug.log
        #[arg(long)]
        debug: bool,
    },

    /// Index the current project for semantic search
    Index {
        /// Project root path (defaults to CWD)
        #[arg(short, long)]
        project: Option<std::path::PathBuf>,

        /// Force full reindex (skip incremental)
        #[arg(long)]
        force: bool,
    },

    /// Launch the project dashboard web UI
    #[cfg(feature = "dashboard")]
    Dashboard {
        /// Project root path (defaults to CWD)
        #[arg(short, long)]
        project: Option<std::path::PathBuf>,

        /// Listen address
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Listen port
        #[arg(long, default_value_t = 8099)]
        port: u16,

        /// Don't auto-open the browser
        #[arg(long)]
        no_open: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logging init happens before CLI parsing so startup errors are captured.
    // We peek at raw args to detect --debug before clap processes them.
    // Caveat: this fires for any subcommand that receives "--debug" as an argument.
    // Currently only `start` has --debug, so this is safe — revisit if other
    // subcommands add conflicting flags.
    let debug_mode = std::env::args().any(|a| a == "--debug");
    let _log_guard = codescout::logging::init(debug_mode);

    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            project,
            transport,
            host,
            port,
            auth_token,
            debug,
        } => {
            tracing::info!("Starting codescout MCP server (transport={})", transport);
            codescout::server::run(project, &transport, &host, port, auth_token, debug).await?;
        }
        Commands::Index { project, force } => {
            let root = project
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            tracing::info!("Indexing project at {}", root.display());
            codescout::embed::index::build_index(&root, force, None).await?;
        }
        #[cfg(feature = "dashboard")]
        Commands::Dashboard {
            project,
            host,
            port,
            no_open,
        } => {
            let root = project
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            tracing::info!("Launching dashboard for {}", root.display());
            codescout::dashboard::serve(root, host, port, !no_open).await?;
        }
    }

    Ok(())
}
