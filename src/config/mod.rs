use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::info;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub language: String,
    pub theme: String,
    pub hardware_acceleration: bool,
    pub external_player: Option<String>,
    pub auto_update_epg: bool,
    pub epg_update_interval_secs: u64,
    pub plex: Option<PlexConfig>,
}

/// Persisted Plex connection (token + chosen server).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlexConfig {
    pub client_id: String,
    pub auth_token: String,
    pub server_url: String,
    pub server_name: String,
}

/// User-facing settings (excludes the secret Plex token).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub language: String,
    pub theme: String,
    pub hardware_acceleration: bool,
    pub external_player: Option<String>,
    pub auto_update_epg: bool,
    pub epg_update_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            theme: "dark".to_string(),
            hardware_acceleration: true,
            external_player: None,
            auto_update_epg: true,
            epg_update_interval_secs: 1800,
            plex: None,
        }
    }
}

impl Config {
    /// Load configuration from file
    pub async fn load() -> anyhow::Result<Self> {
        let config_path = Self::config_path()?;
        
        if config_path.exists() {
            let content = tokio::fs::read_to_string(&config_path).await?;
            let config: Config = serde_json::from_str(&content)?;
            Ok(config)
        } else {
            let config = Config::default();
            config.save().await?;
            Ok(config)
        }
    }

    /// Save configuration to file
    pub async fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_path()?;
        
        if let Some(parent) = config_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(&config_path, content).await?;
        
        info!("Configuration saved to {:?}", config_path);
        Ok(())
    }

    /// Get the configuration file path
    pub fn config_path() -> anyhow::Result<PathBuf> {
        // Use a simple default path
        Ok(PathBuf::from(".megacubo").join("config.json"))
    }

    /// Return a safe, token-free view of the user settings.
    pub fn settings(&self) -> Settings {
        Settings {
            language: self.language.clone(),
            theme: self.theme.clone(),
            hardware_acceleration: self.hardware_acceleration,
            external_player: self.external_player.clone(),
            auto_update_epg: self.auto_update_epg,
            epg_update_interval_secs: self.epg_update_interval_secs,
        }
    }

    /// Apply user settings, preserving the (secret) Plex connection.
    pub fn apply_settings(&mut self, s: Settings) {
        self.language = s.language;
        self.theme = s.theme;
        self.hardware_acceleration = s.hardware_acceleration;
        self.external_player = s.external_player;
        self.auto_update_epg = s.auto_update_epg;
        self.epg_update_interval_secs = s.epg_update_interval_secs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_excludes_plex_token() {
        let mut cfg = Config::default();
        cfg.plex = Some(PlexConfig {
            client_id: "cid".into(),
            auth_token: "SECRET".into(),
            server_url: "http://x:32400".into(),
            server_name: "NAS".into(),
        });
        let s = cfg.settings();
        // The token must not leak into the settings view.
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("SECRET"));
        assert_eq!(s.theme, "dark");
    }

    #[test]
    fn test_apply_settings_preserves_plex() {
        let mut cfg = Config::default();
        cfg.plex = Some(PlexConfig {
            client_id: "cid".into(),
            auth_token: "SECRET".into(),
            server_url: "http://x:32400".into(),
            server_name: "NAS".into(),
        });
        cfg.apply_settings(Settings {
            language: "pt".into(),
            theme: "light".into(),
            hardware_acceleration: false,
            external_player: Some("vlc".into()),
            auto_update_epg: false,
            epg_update_interval_secs: 60,
        });
        assert_eq!(cfg.theme, "light");
        assert_eq!(cfg.external_player.as_deref(), Some("vlc"));
        assert_eq!(cfg.plex.as_ref().unwrap().auth_token, "SECRET");
    }
}