use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, FromRow};
use tracing::info;

/// List type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ListType {
    M3u,
    Xtream,
    Mag,
}

/// Playlist list metadata
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct List {
    pub id: Option<i64>,
    pub url: String,
    pub name: Option<String>,
    #[sqlx(rename = "type")]
    pub list_type: String,  // Store as string in DB
    pub epg_url: Option<String>,
    pub status: String,  // Store as string in DB
    pub last_updated: Option<i64>,
    pub error: Option<String>,
}

impl List {
    /// Create a new list
    pub fn new(url: String, list_type: ListType) -> Self {
        Self {
            id: None,
            url,
            name: None,
            list_type: match list_type {
                ListType::M3u => "m3u".to_string(),
                ListType::Xtream => "xtream".to_string(),
                ListType::Mag => "mag".to_string(),
            },
            epg_url: None,
            status: "loading".to_string(),
            last_updated: None,
            error: None,
        }
    }

    /// Get the list type
    pub fn get_type(&self) -> ListType {
        match self.list_type.as_str() {
            "m3u" => ListType::M3u,
            "xtream" => ListType::Xtream,
            "mag" => ListType::Mag,
            _ => ListType::M3u,
        }
    }

    /// Get the status
    pub fn get_status(&self) -> ListStatus {
        match self.status.as_str() {
            "loaded" => ListStatus::Loaded,
            "error" => ListStatus::Error,
            _ => ListStatus::Loading,
        }
    }
}

/// List status
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum ListStatus {
    #[default]
    Loading,
    Loaded,
    Error,
}

/// List manager for handling M3U/Xtream/MAG playlists
pub struct ListManager {
    pool: SqlitePool,
}

impl ListManager {
    /// Create a new list manager
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Add a new list
    pub async fn add_list(&self, list: &List) -> anyhow::Result<i64> {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query_as::<_, (i64,)>(
            r#"
            INSERT INTO lists (url, name, type, epg_url, status, last_updated, error)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&list.url)
        .bind(&list.name)
        .bind(&list.list_type)
        .bind(&list.epg_url)
        .bind(&list.status)
        .bind(list.last_updated.unwrap_or(now))
        .bind(&list.error)
        .fetch_one(&self.pool)
        .await?;

        info!("Added list: {}", list.url);
        Ok(result.0)
    }

    /// Get all lists
    pub async fn get_all(&self) -> anyhow::Result<Vec<List>> {
        let lists = sqlx::query_as::<_, List>("SELECT * FROM lists ORDER BY name")
            .fetch_all(&self.pool)
            .await?;

        Ok(lists)
    }

    /// Update list status
    pub async fn update_status(&self, url: &str, status: ListStatus, error: Option<&str>) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "UPDATE lists SET status = ?, last_updated = ?, error = ? WHERE url = ?",
        )
        .bind(match status {
            ListStatus::Loading => "loading",
            ListStatus::Loaded => "loaded",
            ListStatus::Error => "error",
        })
        .bind(now)
        .bind(error)
        .bind(url)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Delete a list
    pub async fn delete(&self, url: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM lists WHERE url = ?")
            .bind(url)
            .execute(&self.pool)
            .await?;

        info!("Deleted list: {}", url);
        Ok(())
    }
}

/// Discovery entry for community lists
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DiscoveryEntry {
    pub id: Option<i64>,
    pub url: String,
    pub name: Option<String>,
    #[sqlx(rename = "type")]
    pub entry_type: String,  // Store as string in DB
    pub image: Option<String>,
    pub health: Option<f64>,
    pub last_seen: Option<i64>,
}

impl DiscoveryEntry {
    /// Get the discovery type
    pub fn get_type(&self) -> DiscoveryType {
        match self.entry_type.as_str() {
            "community" => DiscoveryType::Community,
            "iptv-org" => DiscoveryType::IptvOrg,
            _ => DiscoveryType::Public,
        }
    }
}

/// Discovery type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DiscoveryType {
    Public,
    Community,
    IptvOrg,
}

/// Discovery manager for community lists
pub struct DiscoveryManager {
    pool: SqlitePool,
}

impl DiscoveryManager {
    /// Create a new discovery manager
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Get all discovery entries
    pub async fn get_all(&self) -> anyhow::Result<Vec<DiscoveryEntry>> {
        let entries = sqlx::query_as::<_, DiscoveryEntry>(
            "SELECT * FROM discovery ORDER BY health DESC NULLS LAST"
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(entries)
    }

    /// Add a discovery entry
    pub async fn add(&self, entry: &DiscoveryEntry) -> anyhow::Result<i64> {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query_as::<_, (i64,)>(
            r#"
            INSERT INTO discovery (url, name, type, image, health, last_seen)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(url) DO UPDATE SET
                name = excluded.name,
                health = excluded.health,
                last_seen = excluded.last_seen
            "#,
        )
        .bind(&entry.url)
        .bind(&entry.name)
        .bind(&entry.entry_type)
        .bind(&entry.image)
        .bind(entry.health)
        .bind(entry.last_seen.unwrap_or(now))
        .fetch_one(&self.pool)
        .await?;

        Ok(result.0)
    }
}