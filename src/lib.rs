//! crashmon — Linux Crash-Daemon Bibliothek.
//!
//! Module als `pub` exportiert, damit Integrationstests (`tests/`) die
//! Funktionslogik direkt treiben koennen (Plan-Design 8: fake-getriebene
//! Tests). `main.rs` ist duenne Binary darueber.

pub mod aggregate;
pub mod config;
pub mod daemon;
pub mod event;
pub mod gpu;
pub mod ingest;
pub mod output;
