//! o3-server: T3 Code wire-compatible server. See docs/COMPATIBILITY.md.
#[cfg(test)]
extern crate self as o3_server;
pub mod additions;
pub mod auth;
mod deletion;
pub mod http;
mod mcp;
mod mcp_access;
mod mcp_attachments;
mod mcp_mutations;
mod mcp_projects;
mod mcp_tools;
pub mod native_mcp;
mod repositories;
pub mod rpc;
pub mod store;

pub use http::{serve, serve_io};
pub mod command_output;
pub mod events;
pub mod ops;
#[cfg(target_os = "linux")]
mod process_scope;
pub mod profile;
pub mod provider;
mod provider_policy;
pub mod pty;
pub mod routes;
pub mod scheduled;
pub mod scopes;
pub mod service;
pub mod settings;

pub mod assets;
pub mod body;
#[cfg(feature = "bridge")]
pub mod bridge;
#[cfg(feature = "bridge")]
mod bridge_forwards;
pub mod claude;
mod claude_projection;
pub mod codex;
mod codex_projection;
mod content_search;
pub mod ext;
pub mod files;
pub mod import;
pub mod keybind;
pub mod misc;
pub mod p2;
pub mod paths;
pub mod preview;
pub mod proc;
pub mod projects;
pub mod pullreq;
pub mod recovery;
pub mod review;
pub mod sessions;
pub mod setup;
pub mod snapshot;
pub mod stacked;
pub mod streams;
pub mod terminal;
pub mod thread_commands;
pub mod threadq;
pub mod vcs;
pub mod workspace;
mod workspace_transition;
mod worktree_handoff;

pub mod checkpoint;
mod projection;

pub mod executable;

pub mod outbox;

mod provider_input;

mod claude_models;

mod approval_policy;

mod settlement;

pub mod heap;

pub mod device;

mod background;

mod config_sources;

mod title;

mod storage;

mod source_control;

mod limits;

pub mod usage_rates;

mod usage_ledger;

mod webhook;

mod external_update;

mod server_browser;

mod agent_browser;

mod run_restart;

mod attachment_claims;
mod attachment_intake;
mod launch_names;
mod launch_worker;
mod managed_folders;
mod mcp_launch;
mod thread_launch;
mod worktree_setup;

pub mod document_text;

mod context_transfer;
mod handoff_history;

mod handoff_delivery;

mod handoff_budget;
mod native_recovery;

mod provider_switch;
