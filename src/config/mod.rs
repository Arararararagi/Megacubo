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
}