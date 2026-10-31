//! o3-server: T3 Code wire-compatible server. See docs/COMPATIBILITY.md.
pub mod auth;
pub mod http;
pub mod rpc;
pub mod store;

pub use http::{serve, serve_io};
pub mod events;
pub mod profile;
pub mod provider;
pub mod scopes;
pub mod service;
pub mod settings;

pub mod claude;
mod claude_projection;
pub mod import;
pub mod recovery;
pub mod snapshot;
pub mod workspace;
pub mod codex;
mod codex_projection;
