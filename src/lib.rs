//! Megacubo IPTV Player - Rust Implementation
//!
//! A high-performance, cross-platform IPTV player with support for M3U, Xtream, and MAG playlists.

pub mod config;
pub mod db;
pub mod epg;
pub mod lists;
pub mod parser;
pub mod streamer;

// Re-export main types
pub use db::{Database, default_db_path};
pub use parser::{M3uParser, M3uEntry};
pub use epg::{EpgProgramme, EpgChannel, XmltvChannel, XmltvProgramme, parse_xmltv, parse_xmltv_time};
pub use lists::{List, ListType, ListStatus, ListManager, DiscoveryEntry, DiscoveryType, DiscoveryManager};
pub use streamer::{Streamer, StreamInfo, StreamType};

/// Initialize the Megacubo library
pub async fn init() -> anyhow::Result<Database> {
    let db_path = default_db_path()?;
    let db = Database::new(db_path).await?;
    Ok(db)
}

/// Initialize with a custom database path
pub async fn init_with_path(db_path: std::path::PathBuf) -> anyhow::Result<Database> {
    let db = Database::new(db_path).await?;
    Ok(db)
}

/// Get the library version
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}