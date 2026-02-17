//! Main application logic and HTTP API
//!
//! This crate provides:
//! - HTTP API for Tauri frontend communication
//! - Application state management
//! - Event broadcasting
//! - Configuration loading
//! - WebSocket real-time updates

pub mod api;
pub mod config;
pub mod error;
pub mod event_router;
pub mod state;
pub mod websocket;

pub use api::create_router;
pub use config::AppConfig;
pub use error::{Error, Result};
pub use event_router::EventRouter;
pub use state::AppState;
pub use websocket::WebSocketManager;
