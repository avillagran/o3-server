//! o3-server: T3 Code wire-compatible server. See docs/COMPATIBILITY.md.
pub mod auth;
pub mod http;
pub mod rpc;
pub mod store;

pub use http::{serve, serve_io};
pub mod command_output;
pub mod events;
pub mod ops;
pub mod profile;
pub mod provider;
pub mod pty;
pub mod routes;
pub mod scheduled;
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
pub mod assets;
pub mod body;
pub mod ext;
pub mod stacked;
pub mod files;
pub mod paths;
pub mod proc;
pub mod projects;
pub mod review;
pub mod threadq;
pub mod vcs;
pub mod streams;
pub mod terminal;
pub mod thread_commands;
#[cfg(feature = "bridge")]
pub mod bridge;
