use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde::Serialize;
use tracing::info;

const PLEX_PRODUCT: &str = "Megacubo";
const PLEX_VERSION: &str = env!("CARGO_PKG_VERSION");
const PLEX_DEVICE: &str = "desktop";
const PLEX_TV: &str = "https://plex.tv";

/// A connected Plex client (authenticated against a chosen server).
#[derive(Debug, Clone)]
pub struct PlexClient {
    pub base_url: String,
    pub token: String,
    pub client_id: String,
    http: reqwest::Client,
}

/// A Plex library/section (Movies, TV Shows, …).
#[derive(Debug, Clone, Serialize)]
pub struct PlexLibrary {
    pub key: String,
    pub title: String,
    pub kind: String,
}

/// A browsable item (movie, series, season or episode).
#[derive(Debug, Clone, Serialize)]
pub struct PlexItem {
    pub rating_key: String,
    pub title: String,
    pub year: Option<i64>,
    pub thumb: Option<String>,
    pub kind: String,
    pub index: Option<i64>,
    pub parent_title: Option<String>,
}

/// A playable item: resolved direct-play URL + metadata.
#[derive(Debug, Clone, Serialize)]
pub struct PlexPlayable {
    pub url: String,
    pub title: String,
    pub thumb: Option<String>,
    pub kind: String,
}

/// A discovered Plex Media Server.
#[derive(Debug, Clone, Serialize)]
pub struct PlexServer {
    pub name: String,
    pub url: String,
}

/// A login PIN returned to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct PlexPin {
    pub id: String,
    pub code: String,
    pub link_url: String,
}

impl PlexClient {
    pub fn new(base_url: String, token: String, client_id: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            client_id,
            http: reqwest::Client::new(),
        }
    }

    fn auth_headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("X-Plex-Client-Identifier", self.client_id.clone()),
            ("X-Plex-Product", PLEX_PRODUCT.into()),
            ("X-Plex-Version", PLEX_VERSION.into()),
            ("X-Plex-Device", PLEX_DEVICE.into()),
            ("X-Plex-Platform", std::env::consts::OS.into()),
            ("Accept", "application/json".into()),
        ]
    }

    fn req(&self, path: &str) -> reqwest::RequestBuilder {
        let mut b = self.http.get(format!("{}{}", self.base_url, path));
        for (k, v) in self.auth_headers() {
            b = b.header(k, v);
        }
        b.header("X-Plex-Token", &self.token)
    }

    /// List libraries/sections on the server.
    pub async fn libraries(&self) -> Result<Vec<PlexLibrary>> {
        let r: MediaContainer = self
            .req("/library/sections")
            .send()
            .await
            .map_err(|e| anyhow!("Plex libraries request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Plex libraries response invalid: {}", e))?;
        Ok(r.directory.into_iter().map(PlexLibrary::from).collect())
    }

    /// List items (movies or series) in a section.
    pub async fn browse(&self, section: &str) -> Result<Vec<PlexItem>> {
        let r: MediaContainer = self
            .req(&format!("/library/sections/{}/all", section))
            .send()
            .await
            .map_err(|e| anyhow!("Plex browse request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Plex browse response invalid: {}", e))?;
        Ok(r.metadata.into_iter().map(|m| self.to_item(m)).collect())
    }

    /// List seasons of a series.
    pub async fn seasons(&self, rating_key: &str) -> Result<Vec<PlexItem>> {
        self.children_of(rating_key, "season").await
    }

    /// List episodes of a season.
    pub async fn episodes(&self, rating_key: &str) -> Result<Vec<PlexItem>> {
        self.children_of(rating_key, "episode").await
    }

    async fn children_of(&self, rating_key: &str, want: &str) -> Result<Vec<PlexItem>> {
        let r: MediaContainer = self
            .req(&format!("/library/metadata/{}/children", rating_key))
            .send()
            .await
            .map_err(|e| anyhow!("Plex children request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Plex children response invalid: {}", e))?;
        Ok(r
            .metadata
            .into_iter()
            .filter(|m| m.kind == want)
            .map(|m| self.to_item(m))
            .collect())
    }

    /// Resolve the direct-play URL + metadata for a movie/episode.
    pub async fn playable(&self, rating_key: &str) -> Result<PlexPlayable> {
        let r: MediaContainer = self
            .req(&format!("/library/metadata/{}", rating_key))
            .send()
            .await
            .map_err(|e| anyhow!("Plex metadata request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Plex metadata response invalid: {}", e))?;

        let meta = r
            .metadata
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Plex item not found: {}", rating_key))?;

        let url = self
            .direct_play_url(&meta)
            .ok_or_else(|| anyhow!("No playable media part for {}", rating_key))?;

        Ok(PlexPlayable {
            url,
            title: meta.title,
            thumb: self.thumb_url(&meta.thumb),
            kind: meta.kind,
        })
    }

    /// Build the absolute thumbnail URL (token already included).
    pub fn thumb_url(&self, thumb: &str) -> Option<String> {
        if thumb.is_empty() {
            None
        } else {
            Some(format!(
                "{}{}?X-Plex-Token={}",
                self.base_url, thumb, self.token
            ))
        }
    }

    fn direct_play_url(&self, meta: &Metadata) -> Option<String> {
        let part = meta.media.iter().flat_map(|m| m.part.iter()).next()?;
        let path = if part.key.starts_with("/library/parts") {
            part.key.clone()
        } else if part.id > 0 {
            format!("/library/parts/{}/file", part.id)
        } else {
            return None;
        };
        Some(format!("{}{}?X-Plex-Token={}", self.base_url, path, self.token))
    }

    fn to_item(&self, m: Metadata) -> PlexItem {
        PlexItem {
            rating_key: m.ratingKey,
            title: m.title,
            year: m.year,
            thumb: self.thumb_url(&m.thumb),
            kind: m.kind,
            index: m.index,
            parent_title: m.parentTitle,
        }
    }
}

impl PlexLibrary {
    fn from(d: Directory) -> Self {
        Self {
            key: d.key,
            title: d.title,
            kind: d.kind,
        }
    }
}

/// Generate a stable, device-unique client identifier (no external crate).
pub fn generate_client_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let raw = format!("{:x}{:x}", nanos, pid);
    format!("megacubo-{}", raw)
}

/// Start a Plex login PIN. Returns the pin id, the short code and the
/// auth URL the user must open in a browser.
pub async fn create_pin(client_id: &str) -> Result<PlexPin> {
    let r: PinResponse = reqwest::Client::new()
        .post(format!("{}/api/v2/pins?strong=true", PLEX_TV))
        .header("X-Plex-Client-Identifier", client_id)
        .header("X-Plex-Product", PLEX_PRODUCT)
        .header("X-Plex-Version", PLEX_VERSION)
        .header("X-Plex-Device", PLEX_DEVICE)
        .header("X-Plex-Platform", std::env::consts::OS)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| anyhow!("Plex pin request failed: {}", e))?
        .json()
        .await
        .map_err(|e| anyhow!("Plex pin response invalid: {}", e))?;

    let link_url = format!("https://app.plex.tv/auth#?clientID={}&code={}", client_id, r.code);
    Ok(PlexPin {
        id: r.id.to_string(),
        code: r.code,
        link_url,
    })
}

/// Poll a PIN; returns `Some(token)` once the user authenticated.
pub async fn poll_pin(id: &str, client_id: &str) -> Result<Option<String>> {
    let r: PinResponse = reqwest::Client::new()
        .get(format!("{}/api/v2/pins/{}", PLEX_TV, id))
        .header("X-Plex-Client-Identifier", client_id)
        .header("X-Plex-Product", PLEX_PRODUCT)
        .header("X-Plex-Version", PLEX_VERSION)
        .header("X-Plex-Device", PLEX_DEVICE)
        .header("X-Plex-Platform", std::env::consts::OS)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| anyhow!("Plex pin poll failed: {}", e))?
        .json()
        .await
        .map_err(|e| anyhow!("Plex pin poll response invalid: {}", e))?;

    Ok(r.authToken.filter(|t| !t.is_empty()))
}

/// Discover the user's Plex Media Servers from plex.tv.
pub async fn fetch_servers(token: &str, client_id: &str) -> Result<Vec<PlexServer>> {
    let r: ResourceContainer = reqwest::Client::new()
        .get(format!(
            "{}/api/v2/resources?includeHttps=1&includeRelays=1",
            PLEX_TV
        ))
        .header("X-Plex-Token", token)
        .header("X-Plex-Client-Identifier", client_id)
        .header("X-Plex-Product", PLEX_PRODUCT)
        .header("X-Plex-Version", PLEX_VERSION)
        .header("X-Plex-Device", PLEX_DEVICE)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| anyhow!("Plex resources request failed: {}", e))?
        .json()
        .await
        .map_err(|e| anyhow!("Plex resources response invalid: {}", e))?;

    let mut servers = Vec::new();
    for dev in r.media_container.device {
        let is_server = dev
            .provides
            .split(',')
            .any(|p| p.trim().eq_ignore_ascii_case("server"));
        if !is_server {
            continue;
        }
        if let Some(url) = pick_connection(dev.connections) {
            servers.push(PlexServer {
                name: dev.name,
                url,
            });
        }
    }
    info!("Plex: discovered {} server(s)", servers.len());
    Ok(servers)
}

/// Choose the best connection URI (local + https preferred).
fn pick_connection(conns: Vec<Connection>) -> Option<String> {
    let mut best: Option<(u8, String)> = None;
    for c in conns {
        let score = match (c.local, c.protocol.as_str()) {
            (true, "https") => 0,
            (true, _) => 1,
            (false, "https") => 2,
            _ => 3,
        };
        if best.as_ref().map(|(s, _)| score < *s).unwrap_or(true) {
            best = Some((score, c.uri));
        }
    }
    best.map(|(_, u)| u.trim_end_matches('/').to_string())
}

// ---------------------------------------------------------------------------
// Plex JSON response shapes (Plex returns camelCase JSON)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct MediaContainer {
    #[serde(default)]
    directory: Vec<Directory>,
    #[serde(default)]
    metadata: Vec<Metadata>,
    #[serde(default)]
    device: Vec<Device>,
}

#[derive(Debug, Deserialize)]
struct Directory {
    #[serde(default)]
    key: String,
    #[serde(default)]
    title: String,
    #[serde(rename = "type", default)]
    kind: String,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct Metadata {
    #[serde(default)]
    ratingKey: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    year: Option<i64>,
    #[serde(default)]
    thumb: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    index: Option<i64>,
    #[serde(default)]
    parentTitle: Option<String>,
    #[serde(default)]
    media: Vec<Media>,
}

#[derive(Debug, Deserialize)]
struct Media {
    #[serde(default)]
    part: Vec<Part>,
}

#[derive(Debug, Deserialize)]
struct Part {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    key: String,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct PinResponse {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    code: String,
    #[serde(default)]
    authToken: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResourceContainer {
    #[serde(default)]
    media_container: MediaContainer,
}

#[derive(Debug, Deserialize)]
struct Device {
    #[serde(default)]
    name: String,
    #[serde(default)]
    provides: String,
    #[serde(default)]
    connections: Vec<Connection>,
}

#[derive(Debug, Deserialize)]
struct Connection {
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    uri: String,
    #[serde(default)]
    local: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_connection_prefers_local_https() {
        let conns = vec![
            Connection { protocol: "http".into(), uri: "http://10.0.0.5:32400".into(), local: false },
            Connection { protocol: "https".into(), uri: "https://192.168.1.10:32400".into(), local: true },
            Connection { protocol: "https".into(), uri: "https://remote.plex.direct:32400".into(), local: false },
        ];
        assert_eq!(pick_connection(conns).unwrap(), "https://192.168.1.10:32400");
    }

    #[test]
    fn test_pick_connection_falls_back() {
        let conns = vec![
            Connection { protocol: "http".into(), uri: "http://x:32400".into(), local: false },
        ];
        assert_eq!(pick_connection(conns).unwrap(), "http://x:32400");
    }

    #[test]
    fn test_direct_play_url_from_part_key() {
        let client = PlexClient::new(
            "http://192.168.1.10:32400".into(),
            "TOK".into(),
            "cid".into(),
        );
        let meta = Metadata {
            ratingKey: "123".into(),
            title: "Movie".into(),
            year: Some(2020),
            thumb: "/library/metadata/123/thumb".into(),
            kind: "movie".into(),
            index: None,
            parentTitle: None,
            media: vec![Media {
                part: vec![Part { id: 9, key: "/library/parts/9/file.mkv".into() }],
            }],
        };
        let url = client.direct_play_url(&meta).unwrap();
        assert_eq!(url, "http://192.168.1.10:32400/library/parts/9/file.mkv?X-Plex-Token=TOK");

        let thumb = client.thumb_url(&meta.thumb).unwrap();
        assert_eq!(thumb, "http://192.168.1.10:32400/library/metadata/123/thumb?X-Plex-Token=TOK");
    }

    #[test]
    fn test_client_id_format() {
        let id = generate_client_id();
        assert!(id.starts_with("megacubo-"));
    }
}
