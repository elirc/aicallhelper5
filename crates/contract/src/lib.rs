//! The contract every other crate agrees on.
//!
//! * [`types`] — everything that crosses the Rust core <-> page boundary
//!   (commands, events, snapshots, settings view). Exported to TypeScript by
//!   `cargo test -p callcore-contract export_bindings` into `src/generated/`.
//!   Never hand-edit the generated files; CI fails if they drift.
//! * [`ports`] — the traits every external dependency (audio device, STT socket,
//!   LLM provider, settings, clock, keystore, window affinity, event sink) sits
//!   behind, so the session rules are testable with fakes.
//! * [`secret`] — the redacting `Secret` newtype.
//! * [`config`] — every timeout and limit, as named constants.

pub mod config;
pub mod ports;
pub mod secret;
pub mod types;

pub use secret::Secret;
pub use types::*;
