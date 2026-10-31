//! o3-server: T3 Code wire-compatible server. See docs/COMPATIBILITY.md.
pub mod auth;
pub mod http;
pub mod rpc;
pub mod store;

pub use http::serve;
