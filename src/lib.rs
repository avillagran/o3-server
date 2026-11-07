//! o3-server: T3 Code wire-compatible server. See docs/COMPATIBILITY.md.
#[cfg(test)]
extern crate self as o3_server;
pub mod additions;
pub mod auth;
mod deletion;
mod desktop_auth;
pub mod desktop_bootstrap;
pub mod desktop_bootstrap_fd;
pub mod desktop_bootstrap_grant;
pub mod desktop_browser;
pub mod desktop_browser_channel;
pub mod desktop_startup;
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
mod rpc_input;
pub mod shared_readiness;
pub mod store;
#[cfg(test)]
#[path = "../tests/support/sandbox.rs"]
pub(crate) mod test_sandbox;

pub use http::{serve, serve_io};
pub mod command_output;
mod command_shared;
mod composer_context;
pub mod events;
pub mod managed_clients;
pub mod managed_limits;
pub mod managed_policy;
mod managed_settings;
mod managed_settings_codec;
pub mod managed_tailscale;
pub mod managed_tailscale_guard;
pub mod ops;
#[cfg(target_os = "linux")]
mod process_scope;
pub mod profile;
pub mod provider;
mod provider_failure;
mod provider_policy;
mod provider_secrets;
pub mod pty;
mod relay_origin;
pub mod routes;
pub mod scheduled;
pub mod scopes;
pub mod secret_resolver;
pub mod server_secret_store;
pub mod service;
pub mod settings;
pub mod settings_secret_transaction;
pub mod settings_secrets;

pub mod agent_scope;
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
mod forge;
mod fresh_context;
pub mod import;
pub mod keybind;
pub mod misc;
mod native_goal;
pub mod p2;
pub mod paths;
pub mod preview;
pub mod proc;
pub mod projects;
mod provider_frames;
pub mod pullreq;
pub mod recovery;
pub mod review;
pub mod sessions;
pub mod setup;
pub mod snapshot;
pub mod stacked;
pub mod streams;
pub mod t3_shared;
mod task_shared;
pub mod terminal;
pub mod thread_commands;
pub mod threadq;
pub mod vcs;
mod webhook_dedupe;
mod webhook_native;
mod webhook_payload;
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
pub mod device_native_acquire;
pub mod device_native_client;
pub mod device_native_hub;
pub mod device_native_inventory;
pub mod device_native_local;
pub mod device_native_owner;
pub mod device_native_platform;
pub mod device_native_proxy;
pub mod device_native_runtime;
pub mod device_native_ssh;
pub mod device_native_ssh_owner;
pub mod device_native_ssh_transport;
pub mod device_native_state;
pub mod device_native_tools;

mod background;

mod cliproxy;
mod cliproxy_projection;
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

mod provider_lifetime;
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

mod provider_home;

pub mod selection_transition;

pub mod cleanup;

pub mod worktree_removal;
