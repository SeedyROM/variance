use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "variance")]
#[command(about = "Variance P2P communication platform", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser)]
enum Commands {
    /// Start the P2P node
    Start {
        /// Path to configuration file
        #[arg(short, long, default_value = "config.toml")]
        config: String,

        /// HTTP API listen address
        #[arg(short, long, default_value = "127.0.0.1:3000")]
        listen: String,
    },

    /// Generate a new identity
    GenIdentity {
        /// Output file for identity
        #[arg(short, long, default_value = "identity.json")]
        output: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "variance=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start { config, listen } => {
            tracing::info!("Starting Variance node");
            tracing::info!("Config: {}", config);
            tracing::info!("Listen: {}", listen);

            // TODO: Load config, start node, start HTTP API
            tokio::signal::ctrl_c().await?;
            tracing::info!("Shutting down");
        }
        Commands::GenIdentity { output } => {
            tracing::info!("Generating new identity");
            tracing::info!("Output: {}", output);

            // TODO: Generate DID, save to file
        }
    }

    Ok(())
}
