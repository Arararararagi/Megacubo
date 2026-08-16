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
        self.request(reqwest::Method::GET, path)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut b = self.http.request(method, format!("{}{}", self.base_url, path));
        for (k, v) in self.auth_headers() {
            b = b.header(k, v);
        }
        b.header("X-Plex-Token", &self.token)
    }

    /// Mark an item (movie, episode, season or show) watched/unwatched.
    pub async fn set_watched(&self, rating_key: &str, watched: bool) -> Result<()> {
        let resp = self
            .request(reqwest::Method::PUT, &scrobble_path(rating_key, watched))
            .send()
            .await
            .map_err(|e| anyhow!("Plex scrobble request failed: {}", e))?;
        if !resp.status().is_success() {
            return Err(anyhow!("Plex scrobble failed: HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// Refresh an item's metadata from its agent.
    pub async fn refresh(&self, rating_key: &str) -> Result<()> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/library/metadata/{}/refresh?force=1", rating_key),
            )
            .send()
            .await
            .map_err(|e| anyhow!("Plex refresh request failed: {}", e))?;
        if !resp.status().is_success() {
            return Err(anyhow!("Plex refresh failed: HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// List libraries/sections on the server.
    pub async fn libraries(&self) -> Result<Vec<PlexLibrary>> {
        let r: Root = self
            .req("/library/sections")
            .send()
            .await
            .map_err(|e| anyhow!("Plex libraries request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Plex libraries response invalid: {}", e))?;
        Ok(r.container.directory.into_iter().map(PlexLibrary::from).collect())
    }

    /// List items (movies or series) in a section.
    pub async fn browse(&self, section: &str) -> Result<Vec<PlexItem>> {
        let r: Root = self
            .req(&format!("/library/sections/{}/all", section))
            .send()
            .await
            .map_err(|e| anyhow!("Plex browse request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Plex browse response invalid: {}", e))?;
        Ok(r.container.metadata.into_iter().map(|m| self.to_item(m)).collect())
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
        let r: Root = self
            .req(&format!("/library/metadata/{}/children", rating_key))
            .send()
            .await
            .map_err(|e| anyhow!("Plex children request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Plex children response invalid: {}", e))?;
        Ok(r
            .container
            .metadata
            .into_iter()
            .filter(|m| m.kind == want)
            .map(|m| self.to_item(m))
            .collect())
    }

    /// Resolve the direct-play URL + metadata for a movie/episode.
    pub async fn playable(&self, rating_key: &str) -> Result<PlexPlayable> {
        let r: Root = self
            .req(&format!("/library/metadata/{}", rating_key))
            .send()
            .await
            .map_err(|e| anyhow!("Plex metadata request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Plex metadata response invalid: {}", e))?;

        let meta = r
            .container
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

/// Build the scrobble/unscrobble path for an item.
fn scrobble_path(rating_key: &str, watched: bool) -> String {
    format!(
        "/:/{}?key={}&identifier=com.plexapp.plugins.library",
        if watched { "scrobble" } else { "unscrobble" },
        rating_key
    )
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
    let r: Root = reqwest::Client::new()
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
    for dev in r.container.device {
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
    #[serde(default, rename = "Directory")]
    directory: Vec<Directory>,
    #[serde(default, rename = "Metadata")]
    metadata: Vec<Metadata>,
    #[serde(default, rename = "Device")]
    device: Vec<Device>,
}

/// Top-level wrapper returned by Plex (`{"MediaContainer": {...}}`).
#[derive(Debug, Deserialize)]
struct Root {
    #[serde(rename = "MediaContainer")]
    container: MediaContainer,
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
    #[serde(default, rename = "Media")]
    media: Vec<Media>,
}

#[derive(Debug, Deserialize)]
struct Media {
    #[serde(default, rename = "Part")]
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
struct Device {
    #[serde(default)]
    name: String,
    #[serde(default)]
    provides: String,
    #[serde(default, rename = "Connections")]
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

    #[test]
    fn test_plex_pin_serializes_link_url() {
        let pin = PlexPin {
            id: "1".into(),
            code: "ABC".into(),
            link_url: "https://app.plex.tv/auth#?clientID=x&code=ABC".into(),
        };
        let json = serde_json::to_string(&pin).unwrap();
        // Tauri serializes structs as snake_case, so the UI must read `link_url`.
        assert!(json.contains("\"link_url\""), "expected snake_case key, got: {}", json);
        assert!(json.contains("https://app.plex.tv/auth"), "got: {}", json);
    }

    #[test]
    fn test_scrobble_path() {
        assert_eq!(
            scrobble_path("123", true),
            "/:/scrobble?key=123&identifier=com.plexapp.plugins.library"
        );
        assert_eq!(
            scrobble_path("123", false),
            "/:/unscrobble?key=123&identifier=com.plexapp.plugins.library"
        );
    }

    #[test]
    fn test_parse_sections_json() {
        let json = r#"{
            "MediaContainer": {
                "Directory": [
                    {"key": "1", "title": "Movies", "type": "movie"},
                    {"key": "2", "title": "TV Shows", "type": "show"}
                ]
            }
        }"#;
        let c: Root = serde_json::from_str(json).unwrap();
        assert_eq!(c.container.directory.len(), 2);
        assert_eq!(c.container.directory[0].key, "1");
        assert_eq!(c.container.directory[0].kind, "movie");
        assert_eq!(c.container.directory[1].kind, "show");
    }

    #[test]
    fn test_parse_resources_json_picks_server() {
        let json = r#"{
            "MediaContainer": {
                "Device": [
                    {"name": "NAS", "provides": "server", "Connections": [
                        {"protocol": "https", "uri": "https://192.168.1.10:32400", "local": true}
                    ]},
                    {"name": "Client", "provides": "player", "Connections": []}
                ]
            }
        }"#;
        let r: Root = serde_json::from_str(json).unwrap();
        let servers: Vec<_> = r
            .container
            .device
            .into_iter()
            .filter(|d| d.provides.split(',').any(|p| p.trim().eq_ignore_ascii_case("server")))
            .filter_map(|d| pick_connection(d.connections))
            .collect();
        assert_eq!(servers, vec!["https://192.168.1.10:32400".to_string()]);
    }

    #[test]
    fn test_parse_metadata_json_and_play_url() {
        let json = r#"{
            "MediaContainer": {
                "Metadata": [
                    {"ratingKey": "555", "title": "Big Movie", "year": 2021,
                     "thumb": "/library/metadata/555/thumb", "type": "movie",
                     "Media": [{"Part": [{"id": 42, "key": "/library/parts/42/file.mp4"}]}]}
                ]
            }
        }"#;
        let c: Root = serde_json::from_str(json).unwrap();
        let meta = &c.container.metadata[0];
        assert_eq!(meta.ratingKey, "555");
        let client = PlexClient::new("http://h:32400".into(), "T".into(), "cid".into());
        let url = client.direct_play_url(meta).unwrap();
        assert_eq!(url, "http://h:32400/library/parts/42/file.mp4?X-Plex-Token=T");
    }
}
