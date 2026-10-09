//! Runout engine: everything that touches audio. Shared by the desktop app and
//! the `runout` command-line tool.

pub mod audio;
pub mod batch;
pub mod auto;
pub mod chain;
pub mod dsp;
pub mod export;
pub mod level;
pub mod ops;
pub mod presets;
pub mod project;
pub mod render;
pub mod selftest;
pub mod tiles;

#[derive(Debug, Clone)]
pub enum Error {
    Io(String),
    Decode(String),
    Encode(String),
    Project(String),
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(s) => write!(f, "file error: {s}"),
            Error::Decode(s) => write!(f, "could not read audio: {s}"),
            Error::Encode(s) => write!(f, "could not write: {s}"),
            Error::Project(s) => write!(f, "{s}"),
            Error::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
