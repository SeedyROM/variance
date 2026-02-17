use tauri::State;
use variance_app::{identity_gen, start_node as node_start, AppConfig};

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

/// Return the default identity file path: `$HOME/.variance/identity.json`.
#[tauri::command]
pub fn default_identity_path() -> String {
    std::env::var("HOME")
        .map(|home| format!("{}/.variance/identity.json", home))
        .unwrap_or_else(|_| ".variance/identity.json".to_string())
}

/// Start the Variance P2P node and HTTP API server.
///
/// Binds the HTTP server on `127.0.0.1:0` so the OS assigns a free port.
/// Returns the assigned port number.
#[tauri::command]
pub async fn start_node(state: State<'_, NodeState>, identity_path: String) -> Result<u16, String> {
    // Don't start twice
    {
        let port = state.server_port.read().await;
        if port.is_some() {
            return port.ok_or_else(|| "Already started".to_string());
        }
    }

    let config = AppConfig::default();

    let identity_file_path = std::path::Path::new(&identity_path);

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
        assert!(path.ends_with(".variance/identity.json"));
    }
}
