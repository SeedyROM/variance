use tauri::State;
use variance_app::{identity_gen, start_node as node_start, AppConfig, StorageConfig};

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

/// Generate a new identity and write it to the given path.
///
/// Returns the DID and the 12-word mnemonic as a word list.
#[tauri::command]
pub async fn generate_identity(output_path: String) -> Result<GeneratedIdentity, String> {
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

    let json = serde_json::to_string_pretty(&identity).map_err(|e| e.to_string())?;
    std::fs::write(&output_path, json)
        .map_err(|e| format!("Failed to write identity file: {}", e))?;

    let did = identity.did;
    let mnemonic = phrase.split_whitespace().map(String::from).collect();

    Ok(GeneratedIdentity { did, mnemonic })
}

/// Recover an identity from a BIP39 mnemonic phrase and write it to the given path.
///
/// Returns the recovered DID.
#[tauri::command]
pub async fn recover_identity(mnemonic: String, output_path: String) -> Result<String, String> {
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

    let json = serde_json::to_string_pretty(&identity).map_err(|e| e.to_string())?;
    std::fs::write(&output_path, json)
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
/// Falls back to the platform default (`~/Library/Application Support/variance`
/// on macOS, `~/.local/share/variance` on Linux) when the variable is not set.
fn data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("VARIANCE_DATA_DIR") {
        std::path::PathBuf::from(dir)
    } else {
        dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("variance")
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
/// Binds the HTTP server on `127.0.0.1:0` so the OS assigns a free port.
/// Returns the assigned port number.
#[tauri::command]
pub async fn start_node(state: State<'_, NodeState>, identity_path: String) -> Result<u16, String> {
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

    // AppConfig::default() already uses the same dirs::data_local_dir() path,
    // so we only need to override the storage block to keep everything consistent.
    let config = AppConfig {
        storage: StorageConfig {
            identity_path: base_dir.join("identity.json"),
            identity_cache_dir: base_dir.join("identity_cache"),
            message_db_path: base_dir.join("messages.db"),
            base_dir,
        },
        ..AppConfig::default()
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
    let node = node_start(&config, identity_file_path)
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

    // Store state for later shutdown
    *state.app_state.write().await = Some(node.app_state);
    *state.server_port.write().await = Some(port);
    *state.shutdown_tx.write().await = Some(node.shutdown_tx);

    Ok(port)
}

/// Stop the running Variance node.
#[tauri::command]
pub async fn stop_node(state: State<'_, NodeState>) -> Result<(), String> {
    let tx = state.shutdown_tx.write().await.take();
    if let Some(tx) = tx {
        let _ = tx.send(()).await;
    }
    *state.app_state.write().await = None;
    *state.server_port.write().await = None;
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

        let result = generate_identity(path.clone()).await.unwrap();

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
        let generated = generate_identity(path.clone()).await.unwrap();
        let phrase = generated.mnemonic.join(" ");

        // Recover into a different file
        let recover_path = dir
            .path()
            .join("recovered.json")
            .to_str()
            .unwrap()
            .to_string();
        let recovered_did = recover_identity(phrase, recover_path.clone())
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
