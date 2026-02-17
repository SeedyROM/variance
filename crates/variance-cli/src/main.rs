use anyhow::{Context, Result};
use bip39::{Language, Mnemonic};
use clap::Parser;
use ed25519_dalek::SigningKey;
use rand::RngCore;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::signal;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use variance_app::{create_router, AppConfig, AppState, EventRouter};

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
        } => start_node(config, listen, did).await?,

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
async fn start_node(
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

    tracing::info!("HTTP API will listen on: {}", listen_addr);

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

    tracing::info!("Loading identity from: {}", identity_path.display());

    // Create P2P node configuration
    let mut listen_addresses = Vec::new();
    for addr_str in &config.p2p.listen_addrs {
        listen_addresses.push(
            addr_str
                .parse()
                .with_context(|| format!("Invalid listen address: {}", addr_str))?,
        );
    }

    let mut bootstrap_peers = Vec::new();
    for peer_str in &config.p2p.bootstrap_peers {
        let parts: Vec<&str> = peer_str.split('@').collect();
        if parts.len() == 2 {
            bootstrap_peers.push(variance_p2p::BootstrapPeer {
                peer_id: parts[0].to_string(),
                multiaddr: parts[1]
                    .parse()
                    .with_context(|| format!("Invalid bootstrap peer address: {}", parts[1]))?,
            });
        } else {
            tracing::warn!("Skipping invalid bootstrap peer format: {}", peer_str);
        }
    }

    let p2p_config = variance_p2p::Config {
        listen_addresses,
        bootstrap_peers,
        enable_mdns: true,
        storage_path: config.storage.base_dir.clone(),
        ..Default::default()
    };

    // Create P2P node and get handle
    tracing::info!("Initializing P2P node...");
    let (mut node, node_handle) = variance_p2p::Node::new(p2p_config.clone())?;

    // Get EventChannels reference before spawning node
    let event_channels = Arc::new(node.events().clone());

    // Spawn node in background task
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    let node_task = tokio::spawn(async move {
        // Start listening on configured addresses
        if let Err(e) = node.listen(&p2p_config).await {
            tracing::error!("Failed to start listening: {}", e);
            return Err(e);
        }
        // Run event loop
        node.run(shutdown_rx).await
    });

    tracing::info!("P2P node running, creating application state...");

    // Create app state with the node handle and event channels
    let state = AppState::from_identity_file(
        identity_path,
        config.storage.message_db_path.to_str().unwrap(),
        node_handle,
        Some(event_channels.clone()),
    )?;

    // Start event router to bridge P2P events to WebSocket clients
    let router = EventRouter::new(state.ws_manager.clone());
    router.start((*event_channels).clone());
    tracing::info!("EventRouter started");

    tracing::info!("Local DID: {}", state.local_did);

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
    tokio::select! {
        result = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()) => {
            result.context("HTTP server error")?;
        }
    }

    // Shutdown P2P node
    tracing::info!("Shutting down P2P node...");
    let _ = shutdown_tx.send(()).await;

    // Wait for node to finish with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(5), node_task).await {
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

    // Check if file exists
    if output_path.exists() && !force {
        anyhow::bail!(
            "Identity file already exists: {}. Use --force to overwrite.",
            output
        );
    }

    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create directory for identity file")?;
    }

    // Generate BIP39 mnemonic (12 words = 16 bytes of entropy)
    let mut entropy = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut entropy);
    let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy)
        .context("Failed to generate mnemonic")?;

    // Derive Ed25519 signing key from mnemonic seed
    let signing_key = derive_signing_key_from_mnemonic(&mnemonic);
    let verifying_key = signing_key.verifying_key();

    // Generate signaling key (separate from identity signing key)
    let signaling_key = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());

    // Create DID from verifying key
    let did = create_did_from_verifying_key(&verifying_key);

    // Create identity structure
    let identity = serde_json::json!({
        "did": did,
        "signing_key": hex::encode(signing_key.to_bytes()),
        "verifying_key": hex::encode(verifying_key.to_bytes()),
        "signaling_key": hex::encode(signaling_key.to_bytes()),
        "created_at": chrono::Utc::now().to_rfc3339(),
    });

    // Save to file
    std::fs::write(output_path, serde_json::to_string_pretty(&identity)?)
        .context("Failed to write identity file")?;

    tracing::info!("✓ Identity generated successfully");

    // Display mnemonic with prominent warnings
    println!("\n{}", "=".repeat(70));
    println!("🔐 NEW IDENTITY CREATED");
    println!("{}", "=".repeat(70));
    println!("\n⚠️  CRITICAL: WRITE DOWN THESE 12 WORDS TO RECOVER YOUR IDENTITY ⚠️");
    println!("\n{}", "-".repeat(70));

    // Display mnemonic words in a grid
    let words: Vec<&str> = mnemonic.words().collect();
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

    // Check if file exists
    if output_path.exists() && !force {
        anyhow::bail!(
            "Identity file already exists: {}. Use --force to overwrite.",
            output
        );
    }

    // Ensure parent directory exists
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create directory for identity file")?;
    }

    println!("\n{}", "=".repeat(70));
    println!("🔄 IDENTITY RECOVERY");
    println!("{}", "=".repeat(70));
    println!("\nEnter your 12-word recovery phrase (separated by spaces):");
    println!("Example: word1 word2 word3 word4 word5 word6 word7 word8 word9 word10 word11 word12");
    print!("\n> ");

    // Flush stdout to ensure prompt is displayed
    use std::io::Write;
    std::io::stdout().flush()?;

    // Read mnemonic from user input
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read input")?;

    let mnemonic_phrase = input.trim();

    // Validate and parse mnemonic
    let mnemonic = Mnemonic::parse_in(Language::English, mnemonic_phrase)
        .context("Invalid mnemonic phrase. Please check your words and try again.")?;

    // Verify it's 12 words
    if mnemonic.word_count() != 12 {
        anyhow::bail!(
            "Expected 12 words, got {}. Please enter exactly 12 words.",
            mnemonic.word_count()
        );
    }

    // Derive Ed25519 signing key from mnemonic seed (same as generate)
    let signing_key = derive_signing_key_from_mnemonic(&mnemonic);
    let verifying_key = signing_key.verifying_key();

    // Generate new signaling key (not recoverable from mnemonic)
    let signaling_key = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());

    // Create DID from verifying key
    let did = create_did_from_verifying_key(&verifying_key);

    // Create identity structure
    let identity = serde_json::json!({
        "did": did,
        "signing_key": hex::encode(signing_key.to_bytes()),
        "verifying_key": hex::encode(verifying_key.to_bytes()),
        "signaling_key": hex::encode(signaling_key.to_bytes()),
        "created_at": chrono::Utc::now().to_rfc3339(),
    });

    // Save to file
    std::fs::write(output_path, serde_json::to_string_pretty(&identity)?)
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

    let contents = std::fs::read_to_string(&input).context("Failed to read identity file")?;

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

/// Derive signing key from mnemonic (helper for testing)
fn derive_signing_key_from_mnemonic(mnemonic: &Mnemonic) -> SigningKey {
    let seed = mnemonic.to_seed("");
    SigningKey::from_bytes(&seed[..32].try_into().unwrap())
}

/// Create DID from verifying key (helper for testing)
fn create_did_from_verifying_key(verifying_key: &ed25519_dalek::VerifyingKey) -> String {
    format!(
        "did:variance:{}",
        hex::encode(&verifying_key.to_bytes()[..8])
    )
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
    fn test_mnemonic_deterministic() {
        // Same mnemonic should always generate same keys
        let mnemonic_phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic = Mnemonic::parse_in(Language::English, mnemonic_phrase).unwrap();

        let key1 = derive_signing_key_from_mnemonic(&mnemonic);
        let key2 = derive_signing_key_from_mnemonic(&mnemonic);

        assert_eq!(key1.to_bytes(), key2.to_bytes());
    }

    #[test]
    fn test_did_from_mnemonic() {
        // Same mnemonic should always generate same DID
        let mnemonic_phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic = Mnemonic::parse_in(Language::English, mnemonic_phrase).unwrap();

        let signing_key = derive_signing_key_from_mnemonic(&mnemonic);
        let verifying_key = signing_key.verifying_key();
        let did = create_did_from_verifying_key(&verifying_key);

        // DID should be consistent
        assert!(did.starts_with("did:variance:"));
        assert_eq!(did.len(), 29); // "did:variance:" + 16 hex chars
    }

    #[test]
    fn test_generate_and_recover_match() {
        // Generate a mnemonic, derive keys, then verify recovery produces same keys
        let mut entropy = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut entropy);
        let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy).unwrap();

        // Generate keys
        let original_key = derive_signing_key_from_mnemonic(&mnemonic);
        let original_did = create_did_from_verifying_key(&original_key.verifying_key());

        // Simulate recovery by parsing the same mnemonic
        let recovered_mnemonic =
            Mnemonic::parse_in(Language::English, mnemonic.to_string()).unwrap();
        let recovered_key = derive_signing_key_from_mnemonic(&recovered_mnemonic);
        let recovered_did = create_did_from_verifying_key(&recovered_key.verifying_key());

        // Keys and DIDs should match
        assert_eq!(original_key.to_bytes(), recovered_key.to_bytes());
        assert_eq!(original_did, recovered_did);
    }

    #[test]
    fn test_invalid_mnemonic() {
        // Invalid mnemonic should be rejected
        let invalid_phrase = "invalid word sequence that is not valid";
        let result = Mnemonic::parse_in(Language::English, invalid_phrase);
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_word_count() {
        // 11 words (should be 12)
        let wrong_count = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
        let result = Mnemonic::parse_in(Language::English, wrong_count);
        assert!(result.is_err());
    }

    #[test]
    fn test_different_mnemonics_different_keys() {
        // Different mnemonics should generate different keys
        let mut entropy1 = [0u8; 16];
        let mut entropy2 = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut entropy1);
        rand::thread_rng().fill_bytes(&mut entropy2);

        let mnemonic1 = Mnemonic::from_entropy_in(Language::English, &entropy1).unwrap();
        let mnemonic2 = Mnemonic::from_entropy_in(Language::English, &entropy2).unwrap();

        let key1 = derive_signing_key_from_mnemonic(&mnemonic1);
        let key2 = derive_signing_key_from_mnemonic(&mnemonic2);

        // Extremely unlikely to be equal (2^256 keyspace)
        assert_ne!(key1.to_bytes(), key2.to_bytes());
    }
}
