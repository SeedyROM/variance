use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use tokio::signal;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use variance_app::{identity_gen, start_node, AppConfig};

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

        /// Local DID (optional, overrides identity file)
        #[arg(short, long)]
        did: Option<String>,
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
        #[arg(short, long, default_value = ".variance/identity.json")]
        output: String,

        /// Overwrite existing file
        #[arg(short, long)]
        force: bool,
    },

    /// Recover identity from BIP39 mnemonic (12 words)
    Recover {
        /// Output file for recovered identity
        #[arg(short, long, default_value = ".variance/identity.json")]
        output: String,

        /// Overwrite existing file
        #[arg(short, long)]
        force: bool,
    },

    /// Show identity information
    Show {
        /// Path to identity file
        #[arg(short, long, default_value = ".variance/identity.json")]
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
        } => start_node_cmd(config, listen, did).await?,

        Commands::Config { action } => match action {
            ConfigAction::Init { output, force } => init_config(output, force)?,
            ConfigAction::Show { config } => show_config(config)?,
        },

        Commands::Identity { action } => match action {
            IdentityAction::Generate { output, force } => generate_identity(output, force)?,
            IdentityAction::Recover { output, force } => recover_identity(output, force)?,
            IdentityAction::Show { input } => show_identity(input)?,
        },
    }

    Ok(())
}

/// Start the Variance node with HTTP API
async fn start_node_cmd(
    config_path: String,
    listen_override: Option<String>,
    did_override: Option<String>,
) -> Result<()> {
    tracing::info!("Starting Variance node");

    // Load configuration
    let config = if Path::new(&config_path).exists() {
        tracing::info!("Loading configuration from {}", config_path);
        AppConfig::from_file(&config_path).context("Failed to load configuration file")?
    } else {
        tracing::warn!("Configuration file not found, using defaults");
        AppConfig::default()
    };

    // Determine listen address
    let listen_addr: SocketAddr = if let Some(addr) = listen_override {
        addr.parse().context("Invalid listen address format")?
    } else {
        format!("{}:{}", config.server.host, config.server.port)
            .parse()
            .context("Invalid server configuration")?
    };

    // Warn if DID override is provided (deprecated)
    if did_override.is_some() {
        tracing::warn!("--did flag is deprecated and ignored. Identity is loaded from file.");
    }

    // Load identity from file
    let identity_path = &config.storage.identity_path;

    if !identity_path.exists() {
        anyhow::bail!(
            "No identity file found at: {}\n\n\
            To create an identity, run:\n  \
            variance identity generate\n\n\
            This will create your DID and signing keys.",
            identity_path.display()
        );
    }

    // Start the variance node (P2P + AppState + EventRouter + Router)
    // Passphrase support for encrypted identity files: read from VARIANCE_PASSPHRASE env var.
    let passphrase = std::env::var("VARIANCE_PASSPHRASE").ok();
    let node = start_node(&config, identity_path, passphrase.as_deref())
        .await
        .context("Failed to start Variance node")?;

    tracing::info!("Local DID: {}", node.app_state.local_did);

    // Create TCP listener
    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .context("Failed to bind to address")?;

    tracing::info!("✓ Variance node started successfully");
    tracing::info!("  HTTP API: http://{}", listen_addr);
    tracing::info!("  Press Ctrl+C to shutdown");

    // Extract components before moving router
    let router = node.router;
    let shutdown_tx = node.shutdown_tx;
    let node_task = node.node_task;

    // Start HTTP server with graceful shutdown
    tokio::select! {
        result = axum::serve(listener, router).with_graceful_shutdown(shutdown_signal()) => {
            result.context("HTTP server error")?;
        }
    }

    // Shutdown P2P node
    tracing::info!("Shutting down P2P node...");
    let _ = shutdown_tx.send(()).await;

    // Wait for node to finish with timeout
    match tokio::time::timeout(Duration::from_secs(5), node_task).await {
        Ok(Ok(Ok(_))) => tracing::info!("P2P node shut down successfully"),
        Ok(Ok(Err(e))) => tracing::error!("P2P node error during shutdown: {}", e),
        Ok(Err(e)) => tracing::error!("P2P node task panicked: {}", e),
        Err(_) => tracing::warn!("P2P node shutdown timed out"),
    }

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

    let config = AppConfig::from_file(&config_path).context("Failed to load configuration file")?;

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
    println!(
        "  Identity Cache: {}",
        config.storage.identity_cache_dir.display()
    );
    println!(
        "  Message Database: {}",
        config.storage.message_db_path.display()
    );

    println!("\n{}", "=".repeat(60));

    Ok(())
}

/// Generate a new identity (DID + signing key) with BIP39 mnemonic recovery
fn generate_identity(output: String, force: bool) -> Result<()> {
    tracing::info!("Generating new identity");

    let output_path = Path::new(&output);

    if output_path.exists() && !force {
        anyhow::bail!(
            "Identity file already exists: {}. Use --force to overwrite.",
            output
        );
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).context("Failed to create directory for identity file")?;
    }

    let (identity, phrase) = identity_gen::generate(None).context("Failed to generate identity")?;
    let did = identity.did.clone();

    fs::write(output_path, serde_json::to_string_pretty(&identity)?)
        .context("Failed to write identity file")?;

    tracing::info!("✓ Identity generated successfully");

    println!("\n{}", "=".repeat(70));
    println!("🔐 NEW IDENTITY CREATED");
    println!("{}", "=".repeat(70));
    println!("\n⚠️  CRITICAL: WRITE DOWN THESE 12 WORDS TO RECOVER YOUR IDENTITY ⚠️");
    println!("\n{}", "-".repeat(70));

    let words: Vec<&str> = phrase.split_whitespace().collect();
    for (i, chunk) in words.chunks(4).enumerate() {
        print!("  ");
        for (j, word) in chunk.iter().enumerate() {
            let num = i * 4 + j + 1;
            print!("{:2}. {:<12}", num, *word);
        }
        println!();
    }

    println!("{}", "-".repeat(70));
    println!("\n⚠️  THIS RECOVERY PHRASE WILL NEVER BE SHOWN AGAIN! ⚠️");
    println!("\n📝 What to do:");
    println!("   1. Write these words on paper (in order)");
    println!("   2. Store the paper in a safe place");
    println!("   3. NEVER store these words digitally (no photos, no cloud)");
    println!("   4. Anyone with these words can access your identity");
    println!("\n💾 Identity Details:");
    println!("   DID:  {}", did);
    println!("   File: {}", output);
    println!("\n🚀 To start the node:");
    println!("   variance start");
    println!("\n🔄 To recover this identity from the 12 words:");
    println!("   variance identity recover");
    println!("\n{}", "=".repeat(70));

    Ok(())
}

/// Recover identity from BIP39 mnemonic
fn recover_identity(output: String, force: bool) -> Result<()> {
    tracing::info!("Starting identity recovery");

    let output_path = Path::new(&output);

    if output_path.exists() && !force {
        anyhow::bail!(
            "Identity file already exists: {}. Use --force to overwrite.",
            output
        );
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).context("Failed to create directory for identity file")?;
    }

    println!("\n{}", "=".repeat(70));
    println!("🔄 IDENTITY RECOVERY");
    println!("{}", "=".repeat(70));
    println!("\nEnter your 12-word recovery phrase (separated by spaces):");
    println!("Example: word1 word2 word3 word4 word5 word6 word7 word8 word9 word10 word11 word12");
    print!("\n> ");

    use std::io::Write;
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read input")?;

    let mnemonic_phrase = input.trim();
    let identity = identity_gen::recover(mnemonic_phrase).context("Failed to recover identity")?;
    let did = identity.did.clone();

    fs::write(output_path, serde_json::to_string_pretty(&identity)?)
        .context("Failed to write identity file")?;

    tracing::info!("✓ Identity recovered successfully");
    println!("\n{}", "=".repeat(70));
    println!("✅ IDENTITY RECOVERED SUCCESSFULLY");
    println!("{}", "=".repeat(70));
    println!("\n💾 Identity Details:");
    println!("   DID:  {}", did);
    println!("   File: {}", output);
    println!("\n🚀 To start the node:");
    println!("   variance start");
    println!("\n{}", "=".repeat(70));

    Ok(())
}

/// Show identity information
fn show_identity(input: String) -> Result<()> {
    tracing::info!("Loading identity from: {}", input);

    let contents = fs::read_to_string(&input).context("Failed to read identity file")?;

    let identity: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse identity file")?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_recover_match() {
        let (identity, phrase) = identity_gen::generate(None).unwrap();
        let recovered = identity_gen::recover(&phrase).unwrap();
        assert_eq!(identity.did, recovered.did);
        assert_eq!(identity.signing_key, recovered.signing_key);
    }

    #[test]
    fn test_invalid_mnemonic_rejected() {
        assert!(identity_gen::recover("invalid word sequence that is not valid").is_err());
    }
}
