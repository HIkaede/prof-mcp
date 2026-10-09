//! Core, deterministic folded-stack profile analysis for prof-mcp.
//!
//! Rust modules are implementation interfaces and may change between 0.x releases.
//! The public collapse function also serves the development fuzz harness.

pub mod cache;
pub mod capture;
pub mod config;
pub mod error;
pub mod output;
pub mod profile;
pub mod query;
pub mod registry;
pub mod server;
pub mod setup;
