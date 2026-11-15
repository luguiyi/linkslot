//! linkSlot — WebDAV save-state sync for Slot on the RG34XX.
//!
//! This crate is the standalone CLI that the Slot frontend spawns through
//! `/bin/sh <root>/System/linkSlot/shell/linkslot.sh`. It is deliberately synchronous (no
//! tokio) and has no UI: it pushes/pulls a single save-state unit over WebDAV
//! and reports the result through the `LINKSLOT_*` stdout contract.
//!
//! Feature layout:
//! - default build: HTTP only. HTTPS configs are rejected with a clear
//!   configuration error (exit code 3), so the workspace builds without a C
//!   toolchain on Windows.
//! - `tls-rustls`: HTTPS support (rustls + webpki-roots, optional `ca_cert`).

pub mod cache;
pub mod cli;
pub mod config;
pub mod error;
pub mod hash;
pub mod meta;
pub mod mods;
pub mod net;
pub mod output;
pub mod playtime;
pub mod store;
pub mod sync;
pub mod unit;
pub mod util;
pub mod webdav;

pub use error::{ErrorKind, LinkError};
pub use output::{Outcome, ResultKind};
