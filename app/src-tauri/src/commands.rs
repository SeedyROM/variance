use tauri::State;
use variance_app::{identity_gen, start_node as node_start, AppConfig, AppState, StorageConfig};

use crate::state::NodeState;

#[derive(Debug, serde::Serialize)]
pub struct GeneratedIdentity {
    pub did: String,
    pub mnemonic: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct NodeStatus {
    pub running: bool,
    pub local_did: Option<String>,
    pub api_port: Option<u16>,
}

/// Check whether an identity file exists at the given path.
#[tauri::command]
pub async fn has_identity(identity_path: String) -> Result<bool, String> {
    Ok(std::path::Path::new(&identity_path).exists())
}

/// Check whether the identity file at the given path is passphrase-encrypted.
///
/// Returns `true` when the file starts with the `VEID` magic header written by
/// `identity_crypto::encrypt`. Returns `false` for plaintext (unencrypted) files.
/// Returns an error if the file cannot be read.
#[tauri::command]
pub async fn check_identity_encrypted(identity_path: String) -> Result<bool, String> {
    let bytes = std::fs::read(&identity_path)
        .map_err(|e| format!("Failed to read identity file: {}", e))?;
    Ok(variance_app::identity_crypto::is_encrypted(&bytes))
}

/// Generate a new identity and write it to the given path.
///
/// Pass `passphrase` to encrypt the identity file at rest (Argon2id + AES-256-GCM).
/// When `None`, the file is written as plaintext JSON (backward-compatible).
///
/// Returns the DID and the 12-word mnemonic as a word list.
#[tauri::command]
pub async fn generate_identity(
    output_path: String,
    passphrase: Option<String>,
) -> Result<GeneratedIdentity, String> {
    let (identity, phrase) = identity_gen::generate().map_err(|e| e.to_string())?;

    let dir = std::path::Path::new(&output_path).parent().and_then(|p| {
        if p == std::path::Path::new("") {
            None
        } else {
            Some(p)
        }
    });
    if let Some(parent) = dir {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    AppState::save_identity(
        std::path::Path::new(&output_path),
        &identity,
        passphrase.as_deref(),
    )
    .map_err(|e| format!("Failed to write identity file: {}", e))?;

    let did = identity.did;
    let mnemonic = phrase.split_whitespace().map(String::from).collect();

    Ok(GeneratedIdentity { did, mnemonic })
}

/// Recover an identity from a BIP39 mnemonic phrase and write it to the given path.
///
/// Pass `passphrase` to encrypt the recovered identity file at rest.
///
/// Returns the recovered DID.
#[tauri::command]
pub async fn recover_identity(
    mnemonic: String,
    output_path: String,
    passphrase: Option<String>,
) -> Result<String, String> {
    let identity = identity_gen::recover(&mnemonic).map_err(|e| e.to_string())?;

    let dir = std::path::Path::new(&output_path).parent().and_then(|p| {
        if p == std::path::Path::new("") {
            None
        } else {
            Some(p)
        }
    });
    if let Some(parent) = dir {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    AppState::save_identity(
        std::path::Path::new(&output_path),
        &identity,
        passphrase.as_deref(),
    )
    .map_err(|e| format!("Failed to write identity file: {}", e))?;

    Ok(identity.did)
}

/// Resolve the base data directory for this instance.
///
/// Reads `VARIANCE_DATA_DIR` first so a second instance can be run with a
/// different identity by setting that variable before launching the binary:
///
///   VARIANCE_DATA_DIR=/tmp/peer-b ./variance-app
///
/// Falls back to the platform default. In debug builds, uses `variance-dev`
/// to keep dev data separate from the installed release app:
///   - Release: `~/Library/Application Support/variance` (macOS)
///   - Debug:   `~/Library/Application Support/variance-dev` (macOS)
fn data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("VARIANCE_DATA_DIR") {
        std::path::PathBuf::from(dir)
    } else {
        let dir_name = if cfg!(debug_assertions) {
            "variance-dev"
        } else {
            "variance"
        };
        dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(dir_name)
    }
}

/// Return the default identity file path for this instance.
///
/// Respects `VARIANCE_DATA_DIR` so multiple instances can each have their own
/// identity without conflicting.
#[tauri::command]
pub fn default_identity_path() -> String {
    data_dir()
        .join("identity.json")
        .to_string_lossy()
        .into_owned()
}

/// Start the Variance P2P node and HTTP API server.
///
/// Pass `passphrase` when the identity file is encrypted. When `None`, the file
/// is treated as plaintext JSON.
///
/// Binds the HTTP server on `127.0.0.1:0` so the OS assigns a free port.
/// Returns the assigned port number.
#[tauri::command]
pub async fn start_node(
    state: State<'_, NodeState>,
    identity_path: String,
    passphrase: Option<String>,
) -> Result<u16, String> {
    // Hold the start lock for the entire startup sequence. This prevents the
    // race caused by React StrictMode mounting effects twice in dev, which
    // would otherwise let two concurrent calls both pass the "already running"
    // check before either call finishes and sets the port.
    let _start_guard = state
        .start_lock
        .try_lock()
        .map_err(|_| "Node is already starting".to_string())?;

    // If already running, return the existing port.
    if let Some(port) = *state.server_port.read().await {
        return Ok(port);
    }

    let base_dir = data_dir();

    // Ensure the data directory exists before sled or the identity loader touch it.
    std::fs::create_dir_all(&base_dir)
        .map_err(|e| format!("Failed to create data directory: {}", e))?;

    // Load config from {data_dir}/config.toml, creating it with defaults if absent.
    // Storage paths are always derived from base_dir at runtime and override whatever
    // is in the file, so the file only needs to carry user-editable settings (relay
    // peers, bootstrap peers, etc.).
    let config_path = base_dir.join("config.toml");
    let mut config = if config_path.exists() {
        AppConfig::from_file(config_path.to_str().unwrap_or_default())
            .map_err(|e| format!("Failed to load config.toml: {}", e))?
    } else {
        let default_cfg = AppConfig::default();
        if let Err(e) = default_cfg.to_file(config_path.to_str().unwrap_or_default()) {
            tracing::warn!("Failed to write default config.toml: {}", e);
        }
        default_cfg
    };

    // Always derive storage paths from the runtime base_dir so multiple instances
    // (each with their own VARIANCE_DATA_DIR) get correct, non-overlapping paths.
    config.storage = StorageConfig {
        identity_path: base_dir.join("identity.json"),
        identity_cache_dir: base_dir.join("identity_cache"),
        message_db_path: base_dir.join("messages.db"),
        group_message_max_age_days: config.storage.group_message_max_age_days,
        base_dir,
    };

    let identity_file_path = std::path::Path::new(&identity_path);

    // Verify the identity file exists before handing off to node startup,
    // so we can return a clear error rather than an opaque IO error.
    if !identity_file_path.exists() {
        return Err(format!(
            "Identity file not found at {}. Please regenerate your identity.",
            identity_file_path.display()
        ));
    }

    // Start the variance node (P2P + AppState + EventRouter + Router)
    let node = node_start(&config, identity_file_path, passphrase.as_deref())
        .await
        .map_err(|e| format!("Failed to start Variance node: {}", e))?;

    // Bind HTTP server to random port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind: {}", e))?;

    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get port: {}", e))?
        .port();

    tracing::info!("Variance HTTP API started on port {}", port);

    // Spawn HTTP server in background
    let router = node.router;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!("HTTP server error: {}", e);
        }
    });

    // Wrap the node task so we can store a type-erased JoinHandle<anyhow::Result<()>>.
    let raw_task = node.node_task;
    let wrapped_task: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
        raw_task
            .await
            .map_err(|e| anyhow::anyhow!("Node task panicked: {}", e))?
            .map_err(|e| anyhow::anyhow!("Node error: {}", e))
    });

    // Store state for later shutdown
    *state.app_state.write().await = Some(node.app_state);
    *state.server_port.write().await = Some(port);
    *state.shutdown_tx.write().await = Some(node.shutdown_tx);
    *state.node_task.write().await = Some(wrapped_task);

    Ok(port)
}

/// Stop the running Variance node.
#[tauri::command]
pub async fn stop_node(state: State<'_, NodeState>) -> Result<(), String> {
    state.stop().await;
    Ok(())
}

/// Change the passphrase used to encrypt the identity file.
///
/// Validates `current_passphrase` against the one the node was started with,
/// re-encrypts the file with `new_passphrase`, then stops the node so the
/// stale in-memory passphrase cannot corrupt future saves.
///
/// The frontend should navigate the user back to the unlock screen after this.
#[tauri::command]
pub async fn change_passphrase(
    state: State<'_, NodeState>,
    current_passphrase: Option<String>,
    new_passphrase: Option<String>,
) -> Result<(), String> {
    let app_state_guard = state.app_state.read().await;
    let app_state = app_state_guard
        .as_ref()
        .ok_or_else(|| "Node is not running".to_string())?;

    let identity_path = app_state.identity_path.clone();
    let stored_passphrase: Option<String> = app_state
        .identity_passphrase
        .as_ref()
        .map(|s| s.as_str().to_string());
    drop(app_state_guard);

    // Verify current passphrase matches
    if current_passphrase != stored_passphrase {
        return Err("Current passphrase is incorrect".to_string());
    }

    let identity =
        AppState::load_identity_with_passphrase(&identity_path, current_passphrase.as_deref())
            .map_err(|e| format!("Failed to load identity: {}", e))?;

    AppState::save_identity(&identity_path, &identity, new_passphrase.as_deref())
        .map_err(|e| format!("Failed to save identity with new passphrase: {}", e))?;

    // Stop the node — the in-memory passphrase is now stale.
    state.stop().await;

    Ok(())
}

/// Return the current HTTP API port, if the node is running.
#[tauri::command]
pub async fn get_api_port(state: State<'_, NodeState>) -> Result<Option<u16>, String> {
    Ok(*state.server_port.read().await)
}

/// Return the current node status.
#[tauri::command]
pub async fn get_node_status(state: State<'_, NodeState>) -> Result<NodeStatus, String> {
    let port = *state.server_port.read().await;
    let did = state
        .app_state
        .read()
        .await
        .as_ref()
        .map(|s| s.local_did.clone());

    Ok(NodeStatus {
        running: port.is_some(),
        local_did: did,
        api_port: port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_has_identity_false() {
        let result = has_identity("/tmp/nonexistent_variance_identity.json".to_string())
            .await
            .unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_has_identity_true() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");
        std::fs::write(&path, "{}").unwrap();

        let result = has_identity(path.to_str().unwrap().to_string())
            .await
            .unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_generate_identity() {
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join("identity.json")
            .to_str()
            .unwrap()
            .to_string();

        let result = generate_identity(path.clone(), None).await.unwrap();

        assert!(result.did.starts_with("did:variance:"));
        assert_eq!(result.mnemonic.len(), 12);
        assert!(std::path::Path::new(&path).exists());
    }

    #[tokio::test]
    async fn test_recover_identity() {
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join("identity.json")
            .to_str()
            .unwrap()
            .to_string();

        // Generate first to get a valid mnemonic
        let generated = generate_identity(path.clone(), None).await.unwrap();
        let phrase = generated.mnemonic.join(" ");

        // Recover into a different file
        let recover_path = dir
            .path()
            .join("recovered.json")
            .to_str()
            .unwrap()
            .to_string();
        let recovered_did = recover_identity(phrase, recover_path.clone(), None)
            .await
            .unwrap();

        assert_eq!(generated.did, recovered_did);
        assert!(std::path::Path::new(&recover_path).exists());
    }

    #[test]
    fn test_default_identity_path() {
        let path = default_identity_path();
        assert!(path.ends_with("identity.json"));
    }

    #[test]
    fn test_data_dir_env_override() {
        std::env::set_var("VARIANCE_DATA_DIR", "/tmp/test-peer");
        let path = default_identity_path();
        std::env::remove_var("VARIANCE_DATA_DIR");
        assert_eq!(path, "/tmp/test-peer/identity.json");
    }
}
