use std::process::Command;
use tracing::{info, warn, error};

/// Stream information
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub url: String,
    pub stream_type: StreamType,
    pub content_type: Option<String>,
}

/// Stream type detection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    M3u8,      // HLS
    Dash,      // MPEG-DASH
    Ts,        // MPEG-TS
    Rtmp,      // RTMP
    Http,      // HTTP progressive
    Unknown,
}

/// Streamer for handling media playback
pub struct Streamer {
    external_player: Option<String>,
}

impl Streamer {
    /// Create a new streamer
    pub fn new(_hardware_acceleration: bool, external_player: Option<String>) -> Self {
        Self {
            external_player,
        }
    }

    /// Probe a stream URL to determine its type
    pub async fn probe_stream(&self, url: &str) -> anyhow::Result<StreamInfo> {
        // Check URL extension first
        let url_lower = url.to_lowercase();
        
        if url_lower.ends_with(".m3u8") {
            return Ok(StreamInfo {
                url: url.to_string(),
                stream_type: StreamType::M3u8,
                content_type: Some("application/vnd.apple.mpegurl".to_string()),
            });
        }
        
        if url_lower.ends_with(".mpd") {
            return Ok(StreamInfo {
                url: url.to_string(),
                stream_type: StreamType::Dash,
                content_type: Some("application/dash+xml".to_string()),
            });
        }
        
        if url_lower.ends_with(".ts") {
            return Ok(StreamInfo {
                url: url.to_string(),
                stream_type: StreamType::Ts,
                content_type: Some("video/mp2t".to_string()),
            });
        }
        
        if url_lower.starts_with("rtmp://") {
            return Ok(StreamInfo {
                url: url.to_string(),
                stream_type: StreamType::Rtmp,
                content_type: None,
            });
        }

        // For HTTP URLs, we could do a HEAD request to check content-type
        // For now, default to HTTP
        Ok(StreamInfo {
            url: url.to_string(),
            stream_type: StreamType::Http,
            content_type: None,
        })
    }

    /// Launch an external player
    pub fn launch_external_player(&self, url: &str) -> anyhow::Result<()> {
        let player_path = match &self.external_player {
            Some(p) => p.clone(),
            None => {
                // Try to auto-detect common players
                if cfg!(target_os = "windows") {
                    // Check for VLC, MPC-HC, PotPlayer
                    if which::which("vlc").is_ok() {
                        "vlc".to_string()
                    } else if which::which("mpv").is_ok() {
                        "mpv".to_string()
                    } else {
                        return Err(anyhow::anyhow!("No external player configured and none found"));
                    }
                } else if cfg!(target_os = "macos") {
                    if which::which("vlc").is_ok() {
                        "vlc".to_string()
                    } else {
                        return Err(anyhow::anyhow!("No external player configured and none found"));
                    }
                } else {
                    // Linux
                    if which::which("vlc").is_ok() {
                        "vlc".to_string()
                    } else if which::which("mpv").is_ok() {
                        "mpv".to_string()
                    } else {
                        return Err(anyhow::anyhow!("No external player configured and none found"));
                    }
                }
            }
        };

        let status = Command::new(&player_path)
            .arg(url)
            .status()
            .map_err(|e| {
                error!("Failed to launch external player: {}", e);
                anyhow::anyhow!("Failed to launch external player: {}", e)
            })?;

        if status.success() {
            info!("Launched external player: {}", player_path);
        } else {
            warn!("External player exited with status: {:?}", status);
        }

        Ok(())
    }
}

/// Stream type detection from content
impl StreamType {
    /// Detect stream type from content (for future use with libmpv)
    pub fn from_content(content: &[u8]) -> Self {
        // Check for MPEG-TS sync byte (0x47)
        if content.len() >= 188 && content[0] == 0x47 {
            return StreamType::Ts;
        }
        
        // Check for HLS playlist
        if content.starts_with(b"#EXTM3U") {
            return StreamType::M3u8;
        }
        
        // Check for DASH manifest
        if content.starts_with(b"<?xml") && content.windows(4).any(|w| w == b"MPD>") {
            return StreamType::Dash;
        }
        
        StreamType::Unknown
    }
}