//! Fabio-Claw library entry point.
//!
//! This file exposes the internal modules so that integration tests in
//! `tests/integration.rs` can import them via `fabio_claw::...`.

pub mod agent;
pub mod api;
pub mod errors;
pub mod llm;
pub mod memory;
pub mod plugins;
pub mod scheduler;
pub mod security;
pub mod telegram;
