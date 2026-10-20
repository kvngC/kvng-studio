//! Reader for FL Studio project files (`.flp`).

pub mod event;
mod project;

pub use project::*;

use std::fmt;

#[derive(Debug)]
pub enum Error {
    BadMagic,
    Truncated(usize),
    BadVarint(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadMagic => write!(f, "not an FL Studio project (bad chunk magic)"),
            Error::Truncated(at) => write!(f, "file ends unexpectedly at byte {at}"),
            Error::BadVarint(at) => write!(f, "invalid length prefix at byte {at}"),
        }
    }
}

impl std::error::Error for Error {}

/// Reads and parses an `.flp` file from disk.
pub fn read(path: impl AsRef<std::path::Path>) -> Result<Project, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    Ok(parse(&bytes)?)
}
