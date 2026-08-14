use std::process::Command;
#[cfg(feature = "media")]
use std::sync::Arc;
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

/// In-app player state (for libmpv)
#[cfg(feature = "media")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

#[cfg(feature = "media")]
pub struct MpvPlayer {
    ctx: Arc<libmpv::Mpv>,
}

#[cfg(feature = "media")]
impl MpvPlayer {
    pub fn new() -> anyhow::Result<Self> {
        let ctx = libmpv::Mpv::new().map_err(|e| anyhow::anyhow!("{}", e))?;
        ctx.set_property("vo", "gpu").map_err(|e| anyhow::anyhow!("{}", e))?;
        ctx.set_property("hwdec", "auto").map_err(|e| anyhow::anyhow!("{}", e))?;
        ctx.set_property("keep-open", "yes").map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(Self { ctx: Arc::new(ctx) })
    }

    pub fn play(&self, url: &str) -> anyhow::Result<()> {
        self.ctx.command("loadfile", &[url]).map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(())
    }

    pub fn pause(&self) -> anyhow::Result<()> {
        self.ctx.set_property("pause", true).map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(())
    }

    pub fn resume(&self) -> anyhow::Result<()> {
        self.ctx.set_property("pause", false).map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(())
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        self.ctx.command("stop", &[]).map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(())
    }

    pub fn set_volume(&self, volume: f64) -> anyhow::Result<()> {
        self.ctx.set_property("volume", volume).map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(())
    }

    pub fn get_time_pos(&self) -> anyhow::Result<Option<f64>> {
        match self.ctx.get_property::<f64>("time-pos") {
            Ok(v) => Ok(Some(v)),
            Err(_) => Ok(None),
        }
    }

    pub fn get_duration(&self) -> anyhow::Result<Option<f64>> {
        match self.ctx.get_property::<f64>("duration") {
            Ok(v) => Ok(Some(v)),
            Err(_) => Ok(None),
        }
    }

    pub fn set_time_pos(&self, pos: f64) -> anyhow::Result<()> {
        self.ctx.set_property("time-pos", pos).map_err(|e| anyhow::anyhow!("{}", e))?;
        Ok(())
    }
}

/// Streamer for handling media playback
pub struct Streamer {
    external_player: Option<String>,
    #[cfg(feature = "media")]
    pub mpv: Option<MpvPlayer>,
}

impl Streamer {
    /// Create a new streamer
    pub fn new(_hardware_acceleration: bool, external_player: Option<String>) -> Self {
        #[cfg(feature = "media")]
        let mpv = if _hardware_acceleration {
            MpvPlayer::new().ok()
        } else {
            None
        };

        Self {
            external_player,
            #[cfg(feature = "media")]
            mpv,
        }
    }

    /// Probe a stream URL to determine its type
    pub async fn probe_stream(&self, url: &str) -> anyhow::Result<StreamInfo> {
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

        Ok(StreamInfo {
            url: url.to_string(),
            stream_type: StreamType::Http,
            content_type: None,
        })
    }

    /// Play a stream in-app (requires `media` feature)
    #[cfg(feature = "media")]
    pub fn play_in_app(&self, url: &str) -> anyhow::Result<()> {
        if let Some(ref mpv) = self.mpv {
            mpv.play(url)?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("In-app playback not available (media feature disabled)"))
        }
    }

    /// Pause in-app playback
    #[cfg(feature = "media")]
    pub fn pause_in_app(&self) -> anyhow::Result<()> {
        if let Some(ref mpv) = self.mpv {
            mpv.pause()
        } else {
            Err(anyhow::anyhow!("In-app playback not available"))
        }
    }

    /// Resume in-app playback
    #[cfg(feature = "media")]
    pub fn resume_in_app(&self) -> anyhow::Result<()> {
        if let Some(ref mpv) = self.mpv {
            mpv.resume()
        } else {
            Err(anyhow::anyhow!("In-app playback not available"))
        }
    }

    /// Stop in-app playback
    #[cfg(feature = "media")]
    pub fn stop_in_app(&self) -> anyhow::Result<()> {
        if let Some(ref mpv) = self.mpv {
            mpv.stop()
        } else {
            Err(anyhow::anyhow!("In-app playback not available"))
        }
    }

    /// Launch an external player
    pub fn launch_external_player(&self, url: &str) -> anyhow::Result<()> {
        let player_path = match &self.external_player {
            Some(p) => p.clone(),
            None => {
                if cfg!(target_os = "windows") {
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
        if content.len() >= 188 && content[0] == 0x47 {
            return StreamType::Ts;
        }
        
        if content.starts_with(b"#EXTM3U") {
            return StreamType::M3u8;
        }
        
        if content.starts_with(b"<?xml") && content.windows(4).any(|w| w == b"MPD>") {
            return StreamType::Dash;
        }

        StreamType::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_content() {
        assert_eq!(StreamType::from_content(b"#EXTM3U x"), StreamType::M3u8);
        assert_eq!(
            StreamType::from_content(b"<?xml version=\"1.0\"?><MPD>"),
            StreamType::Dash
        );
        // MPEG-TS starts with sync byte 0x47
        let mut ts = vec![0u8; 188];
        ts[0] = 0x47;
        assert_eq!(StreamType::from_content(&ts), StreamType::Ts);
        assert_eq!(StreamType::from_content(b"random bytes"), StreamType::Unknown);
    }

    #[tokio::test]
    async fn test_probe_stream() {
        let s = Streamer::new(false, None);
        assert_eq!(
            s.probe_stream("http://x/foo.m3u8").await.unwrap().stream_type,
            StreamType::M3u8
        );
        assert_eq!(
            s.probe_stream("http://x/foo.mpd").await.unwrap().stream_type,
            StreamType::Dash
        );
        assert_eq!(
            s.probe_stream("http://x/foo.ts").await.unwrap().stream_type,
            StreamType::Ts
        );
        assert_eq!(
            s.probe_stream("rtmp://x/foo").await.unwrap().stream_type,
            StreamType::Rtmp
        );
        assert_eq!(
            s.probe_stream("http://x/foo.mp4").await.unwrap().stream_type,
            StreamType::Http
        );
    }
}