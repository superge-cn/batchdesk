//! Shared shell-side adapters used by the CLI and the Tauri shell.
//!
//! This crate may perform filesystem I/O; it must not contain domain rules.
//! The kernel remains pure and receives these adapters through its ports.

pub mod config_store;
pub mod fs;
pub mod ids;
pub mod journal;
pub mod oplog;
pub mod preflight;
pub mod protected;
pub mod retention;
pub mod scan;
pub mod text;
