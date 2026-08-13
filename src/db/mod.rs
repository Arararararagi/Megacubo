use serde::{Serialize, Deserialize};
use sqlx::{SqlitePool, FromRow};
use sqlx::sqlite::SqlitePoolOptions;
use std::path::PathBuf;
use tracing::info;

use crate::parser::M3uEntry;

/// A channel stored in the database
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Channel {
    pub id: Option<i64>,
    pub list_url: String,
    pub name: String,
    pub url: String,
    pub icon: Option<String>,
    pub group_title: Option<String>,
}

/// Database manager for Megacubo
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Initialize the database with the given path
    pub async fn new(db_path: PathBuf) -> anyhow::Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Create connection pool
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect(&format!("sqlite://{}?mode=rwc", db_path.to_string_lossy()))
            .await?;

        // Enable WAL mode for better concurrent access
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&pool)
            .await?;

        // Create tables
        Self::create_tables(&pool).await?;

        info!("Database initialized at {:?}", db_path);

        Ok(Self { pool })
    }

    /// Create all required tables
    async fn create_tables(pool: &SqlitePool) -> anyhow::Result<()> {
        // Channels table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS channels (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                list_url TEXT NOT NULL,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                icon TEXT,
                group_title TEXT,
                tvg_id TEXT,
                tvg_name TEXT,
                tvg_logo TEXT,
                tvg_country TEXT,
                tvg_language TEXT,
                age INTEGER DEFAULT 0,
                rating TEXT,
                parental TEXT,
                content_type TEXT,
                created_at INTEGER,
                updated_at INTEGER,
                UNIQUE(list_url, url)
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Create indexes for channels
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_channels_list_url ON channels(list_url)")
            .execute(pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_channels_group ON channels(group_title)")
            .execute(pool)
            .await?;

        // EPG programmes table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS epg_programmes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_name TEXT NOT NULL,
                start_time INTEGER NOT NULL,
                end_time INTEGER NOT NULL,
                title TEXT NOT NULL,
                description TEXT,
                icon TEXT,
                categories TEXT,
                terms TEXT,
                age INTEGER,
                lang TEXT,
                country TEXT,
                rating TEXT,
                parental TEXT,
                content_type TEXT,
                epg_url TEXT,
                created_at INTEGER
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Create indexes for EPG
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_epg_channel_start ON epg_programmes(channel_name, start_time)")
            .execute(pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_epg_channel_end ON epg_programmes(channel_name, end_time)")
            .execute(pool)
            .await?;

        // EPG channels table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS epg_channels (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                icon TEXT,
                terms TEXT,
                updated_at INTEGER
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Bookmarks table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS bookmarks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_name TEXT NOT NULL,
                channel_url TEXT NOT NULL,
                icon TEXT,
                created_at INTEGER,
                UNIQUE(channel_name)
            )
            "#,
        )
        .execute(pool)
        .await?;

        // History table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_name TEXT NOT NULL,
                channel_url TEXT NOT NULL,
                icon TEXT,
                played_at INTEGER,
                duration INTEGER
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Lists metadata table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS lists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                url TEXT NOT NULL UNIQUE,
                name TEXT,
                type TEXT,
                epg_url TEXT,
                status TEXT,
                last_updated INTEGER,
                error TEXT
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Discovery table (community lists)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS discovery (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                url TEXT NOT NULL UNIQUE,
                name TEXT,
                type TEXT,
                image TEXT,
                health REAL,
                last_seen INTEGER
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Config table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS config (
                key TEXT PRIMARY KEY,
                value TEXT
            )
            "#,
        )
        .execute(pool)
        .await?;

        info!("Database tables created successfully");
        Ok(())
    }

    /// Get a reference to the connection pool
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Insert a parsed M3U entry as a channel for the given list.
    /// Uses `INSERT OR IGNORE` so re-adding a list does not error on duplicates.
    pub async fn insert_channel(&self, entry: &M3uEntry, list_url: &str) -> anyhow::Result<i64> {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO channels (
                list_url, name, url, icon, group_title,
                tvg_id, tvg_name, tvg_logo, tvg_country, tvg_language,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(list_url)
        .bind(&entry.name)
        .bind(&entry.url)
        .bind(&entry.icon)
        .bind(&entry.group)
        .bind(&entry.tvg_id)
        .bind(&entry.tvg_name)
        .bind(&entry.tvg_logo)
        .bind(&entry.tvg_country)
        .bind(&entry.tvg_language)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Get channels for a list with pagination
    pub async fn get_channels(&self, list_url: &str, limit: i64, offset: i64) -> anyhow::Result<Vec<Channel>> {
        let rows = sqlx::query_as::<_, Channel>(
            "SELECT id, list_url, name, url, icon, group_title \
             FROM channels WHERE list_url = ? ORDER BY name LIMIT ? OFFSET ?",
        )
        .bind(list_url)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Add or update a bookmark for a channel
    pub async fn add_bookmark(&self, channel_name: &str, channel_url: &str, icon: Option<&str>) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO bookmarks (channel_name, channel_url, icon, created_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(channel_name) DO UPDATE SET channel_url = excluded.channel_url, icon = excluded.icon",
        )
        .bind(channel_name)
        .bind(channel_url)
        .bind(icon)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get all bookmarks
    pub async fn get_bookmarks(&self) -> anyhow::Result<Vec<(String, String, Option<String>)>> {
        let rows = sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT channel_name, channel_url, icon FROM bookmarks ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Record a played channel in history
    pub async fn add_history(
        &self,
        channel_name: &str,
        channel_url: &str,
        icon: Option<&str>,
        duration: Option<i64>,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO history (channel_name, channel_url, icon, played_at, duration) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(channel_name)
        .bind(channel_url)
        .bind(icon)
        .bind(now)
        .bind(duration)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get recent history entries (most recent first)
    pub async fn get_history(&self, limit: i64) -> anyhow::Result<Vec<(String, String, Option<String>, i64)>> {
        let rows = sqlx::query_as::<_, (String, String, Option<String>, i64)>(
            "SELECT channel_name, channel_url, icon, played_at FROM history ORDER BY played_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }
}

/// Get the default database path
pub fn default_db_path() -> anyhow::Result<PathBuf> {
    let mut dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".megacubo"));
    dir.push("Megacubo");
    Ok(dir.join("megacubo.db"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::M3uEntry;

    #[tokio::test]
    async fn test_insert_and_get_channel() {
        let path = std::env::temp_dir().join(format!("megacubo_test_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let db = Database::new(path.clone()).await.unwrap();
        let entry = M3uEntry {
            name: "Test Channel".to_string(),
            url: "http://example.com/stream.m3u8".to_string(),
            icon: Some("http://example.com/logo.png".to_string()),
            group: Some("News".to_string()),
            tvg_id: None,
            tvg_name: None,
            tvg_logo: None,
            tvg_country: None,
            tvg_language: None,
        };
        let list_url = "http://example.com/playlist.m3u";

        let id = db.insert_channel(&entry, list_url).await.unwrap();
        assert!(id > 0);

        let channels = db.get_channels(list_url, 10, 0).await.unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name, "Test Channel");
        assert_eq!(channels[0].url, "http://example.com/stream.m3u8");
        assert_eq!(channels[0].group_title.as_deref(), Some("News"));

        let _ = std::fs::remove_file(&path);
    }
}