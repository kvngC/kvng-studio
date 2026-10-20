//! Runs plugins in separate helper processes.
//!
//! A crashing plugin then only takes down its helper, and Intel-only plugins can
//! run in an Intel helper (under Rosetta) while KVNG Studio stays native. Control
//! messages go over a Unix socket; audio is exchanged per block through shared
//! memory, with named semaphores for signalling.

pub mod client;
pub mod protocol;
pub mod scan;
pub mod server;
mod shm;

pub use client::{EditorHandle, Helper, Info, RemotePlugin, bundle_supports_this_arch};
