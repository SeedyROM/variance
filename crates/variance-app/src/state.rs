// Application state

pub struct AppState {
    // TODO: P2P node, identity manager, etc.
}

impl AppState {
    /// Create a new application state
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_state() {
        let state = AppState::new();
        let _ = state; // Ensure it compiles
    }

    #[test]
    fn test_default_state() {
        let state = AppState::default();
        let _ = state; // Ensure it compiles
    }
}
