use anyhow::{Context, Result};
use clap::Parser;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use std::net::SocketAddr;
use std::path::Path;
use tokio::signal;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use variance_app::{create_router, AppConfig, AppState};

#[derive(Parser)]
#[command(name = "variance")]
#[command(about = "Variance P2P communication platform", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser)]
enum Commands {
    /// Start the Variance node with HTTP API
    Start {
        /// Path to configuration file
        #[arg(short, long, default_value = "config.toml")]
        config: String,

        /// HTTP API listen address (overrides config)
        #[arg(short, long)]
        listen: Option<String>,

        /// Local DID (required for now)
        #[arg(short, long)]
        did: String,
    },

    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Identity management
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
    },
}

#[derive(Parser)]
enum ConfigAction {
    /// Initialize a new configuration file
    Init {
        /// Output path for config file
        #[arg(short, long, default_value = "config.toml")]
        output: String,

        /// Overwrite existing file
        #[arg(short, long)]
        force: bool,
    },

    /// Show current configuration
    Show {
        /// Path to configuration file
        #[arg(short, long, default_value = "config.toml")]
        config: String,
    },
}

#[derive(Parser)]
enum IdentityAction {
    /// Generate a new identity (DID + signing key)
    Generate {
        /// Output file for identity keypair
        #[arg(short, long, default_value = "identity.json")]
        output: String,

        /// Overwrite existing file
        #[arg(short, long)]
        force: bool,
    },

    /// Show identity information
    Show {
        /// Path to identity file
        #[arg(short, long, default_value = "identity.json")]
        input: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "variance=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            config,
            listen,
            did,
        } => start_node(config, listen, did).await?,

        Commands::Config { action } => match action {
            ConfigAction::Init { output, force } => init_config(output, force)?,
            ConfigAction::Show { config } => show_config(config)?,
        },

        Commands::Identity { action } => match action {
            IdentityAction::Generate { output, force } => generate_identity(output, force)?,
            IdentityAction::Show { input } => show_identity(input)?,
        },
    }

    Ok(())
}

/// Start the Variance node with HTTP API
async fn start_node(config_path: String, listen_override: Option<String>, did: String) -> Result<()> {
    tracing::info!("Starting Variance node");

    // Load configuration
    let config = if Path::new(&config_path).exists() {
        tracing::info!("Loading configuration from {}", config_path);
        AppConfig::from_file(&config_path)
            .context("Failed to load configuration file")?
    } else {
        tracing::warn!("Configuration file not found, using defaults");
        AppConfig::default()
    };

    // Determine listen address
    let listen_addr: SocketAddr = if let Some(addr) = listen_override {
        addr.parse()
            .context("Invalid listen address format")?
    } else {
        format!("{}:{}", config.server.host, config.server.port)
            .parse()
            .context("Invalid server configuration")?
    };

    tracing::info!("HTTP API will listen on: {}", listen_addr);
    tracing::info!("Local DID: {}", did);

    // Create application state
    let state = AppState::new(did.clone());

    // Create Axum router
    let app = create_router(state);

    // Create TCP listener
    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .context("Failed to bind to address")?;

    tracing::info!("✓ Variance node started successfully");
    tracing::info!("  HTTP API: http://{}", listen_addr);
    tracing::info!("  Press Ctrl+C to shutdown");

    // Start HTTP server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server error")?;

    tracing::info!("Variance node shut down gracefully");

    Ok(())
}

/// Initialize a new configuration file
fn init_config(output: String, force: bool) -> Result<()> {
    tracing::info!("Initializing configuration file: {}", output);

    // Check if file exists
    if Path::new(&output).exists() && !force {
        anyhow::bail!(
            "Configuration file already exists: {}. Use --force to overwrite.",
            output
        );
    }

    // Create default configuration
    let config = AppConfig::default();

    // Save to file
    config
        .to_file(&output)
        .context("Failed to write configuration file")?;

    tracing::info!("✓ Configuration file created: {}", output);
    tracing::info!("  Edit the file to customize settings");
    tracing::info!("  Start the node with: variance start --config {}", output);

    Ok(())
}

/// Show current configuration
fn show_config(config_path: String) -> Result<()> {
    tracing::info!("Loading configuration from: {}", config_path);

    let config = AppConfig::from_file(&config_path)
        .context("Failed to load configuration file")?;

    println!("\n{}", "=".repeat(60));
    println!("Variance Configuration");
    println!("{}", "=".repeat(60));

    println!("\n[Server]");
    println!("  Host: {}", config.server.host);
    println!("  Port: {}", config.server.port);

    println!("\n[P2P]");
    println!("  Listen Addresses:");
    for addr in &config.p2p.listen_addrs {
        println!("    - {}", addr);
    }
    println!("  Bootstrap Peers: {}", config.p2p.bootstrap_peers.len());

    println!("\n[Identity]");
    println!("  IPFS API: {}", config.identity.ipfs_api);
    println!("  Cache TTL: {}s", config.identity.cache_ttl_secs);

    println!("\n[Media]");
    println!("  STUN Servers:");
    for server in &config.media.stun_servers {
        println!("    - {}", server);
    }
    println!("  TURN Servers: {}", config.media.turn_servers.len());

    println!("\n[Storage]");
    println!("  Base Directory: {}", config.storage.base_dir.display());
    println!("  Identity Cache: {}", config.storage.identity_cache_dir.display());
    println!("  Message Database: {}", config.storage.message_db_path.display());

    println!("\n{}", "=".repeat(60));

    Ok(())
}

/// Generate a new identity (DID + signing key)
fn generate_identity(output: String, force: bool) -> Result<()> {
    tracing::info!("Generating new identity");

    // Check if file exists
    if Path::new(&output).exists() && !force {
        anyhow::bail!(
            "Identity file already exists: {}. Use --force to overwrite.",
            output
        );
    }

    // Generate new signing key
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    // Create a simple DID (in production, this would be more sophisticated)
    let did = format!(
        "did:variance:{}",
        hex::encode(&verifying_key.to_bytes()[..8])
    );

    // Create identity structure
    let identity = serde_json::json!({
        "did": did,
        "signing_key": hex::encode(signing_key.to_bytes()),
        "verifying_key": hex::encode(verifying_key.to_bytes()),
        "created_at": chrono::Utc::now().to_rfc3339(),
    });

    // Save to file
    std::fs::write(&output, serde_json::to_string_pretty(&identity)?)
        .context("Failed to write identity file")?;

    tracing::info!("✓ Identity generated successfully");
    println!("\n{}", "=".repeat(60));
    println!("New Identity Created");
    println!("{}", "=".repeat(60));
    println!("\n  DID: {}", did);
    println!("  File: {}", output);
    println!("\n⚠️  IMPORTANT: Keep this file secure!");
    println!("  It contains your private signing key.");
    println!("\nTo start the node with this identity:");
    println!("  variance start --did {}", did);
    println!("\n{}", "=".repeat(60));

    Ok(())
}

/// Show identity information
fn show_identity(input: String) -> Result<()> {
    tracing::info!("Loading identity from: {}", input);

    let contents = std::fs::read_to_string(&input)
        .context("Failed to read identity file")?;

    let identity: serde_json::Value = serde_json::from_str(&contents)
        .context("Failed to parse identity file")?;

    println!("\n{}", "=".repeat(60));
    println!("Identity Information");
    println!("{}", "=".repeat(60));

    if let Some(did) = identity.get("did").and_then(|v| v.as_str()) {
        println!("\n  DID: {}", did);
    }

    if let Some(created) = identity.get("created_at").and_then(|v| v.as_str()) {
        println!("  Created: {}", created);
    }

    if let Some(vkey) = identity.get("verifying_key").and_then(|v| v.as_str()) {
        println!("  Verifying Key: {}...", &vkey[..16]);
    }

    println!("\n  ⚠️  Private key is present in file");
    println!("  File: {}", input);

    println!("\n{}", "=".repeat(60));

    Ok(())
}

/// Wait for shutdown signal (Ctrl+C)
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C signal");
        },
        _ = terminate => {
            tracing::info!("Received termination signal");
        },
    }
}
