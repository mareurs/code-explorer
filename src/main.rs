use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

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
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            project,
            transport,
            host,
            port,
            auth_token,
        } => {
            tracing::info!("Starting codescout MCP server (transport={})", transport);
            codescout::server::run(project, &transport, &host, port, auth_token).await?;
        }
        Commands::Index { project, force } => {
            let root = project
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            tracing::info!("Indexing project at {}", root.display());
            codescout::embed::index::build_index(&root, force).await?;
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
