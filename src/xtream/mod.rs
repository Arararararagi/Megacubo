use anyhow::{anyhow, Result};
use serde::Deserialize;
use tracing::info;

/// Credentials extracted from an Xtream Codes URL.
#[derive(Debug, Clone)]
pub struct XtreamCredentials {
    pub base_url: String,
    pub username: String,
    pub password: String,
}

/// A live channel resolved from an Xtream provider.
#[derive(Debug, Clone)]
pub struct XtreamChannel {
    pub name: String,
    pub url: String,
    pub icon: Option<String>,
    pub group: Option<String>,
    pub tvg_id: Option<String>,
    pub catchup_source: Option<String>,
    pub catchup_days: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthResponse {
    #[serde(default)]
    user_info: Option<UserInfo>,
    #[serde(default)]
    server_info: Option<ServerInfo>,
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    username: String,
    password: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    auth: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ServerInfo {
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "server_protocol")]
    protocol: Option<String>,
    #[serde(default, rename = "port")]
    port: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Category {
    category_id: String,
    category_name: String,
}

#[derive(Debug, Deserialize)]
struct LiveStream {
    stream_id: i64,
    name: String,
    #[serde(default)]
    stream_icon: String,
    #[serde(default)]
    category_id: String,
    #[serde(default)]
    epg_channel_id: Option<String>,
    #[serde(default)]
    tv_archive: Option<String>,
    #[serde(default)]
    tv_archive_duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VodStream {
    #[serde(default)]
    stream_id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    stream_icon: String,
    #[serde(default)]
    category_id: String,
    #[serde(default)]
    container_extension: String,
    #[serde(default)]
    rating_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeriesItem {
    #[serde(default)]
    series_id: i64,
    #[allow(dead_code)]
    #[serde(default)]
    name: String,
    #[serde(default)]
    cover: String,
    #[allow(dead_code)]
    #[serde(default)]
    category_id: String,
}

#[derive(Debug, Deserialize)]
struct SeriesInfo {
    #[allow(dead_code)]
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    seasons: std::collections::HashMap<String, Vec<Episode>>,
}

#[derive(Debug, Deserialize)]
struct Episode {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    container_extension: String,
}

/// Client for the Xtream Codes `player_api.php` API.
pub struct XtreamClient {
    creds: XtreamCredentials,
    http: reqwest::Client,
}

impl XtreamCredentials {
    /// Parse an Xtream Codes URL into credentials.
    ///
    /// Supports the common shapes:
    /// - `http://host:port/username/password`
    /// - `http://host:port/c/username/password`
    /// - `http://host:port/player_api.php?username=X&password=Y`
    /// - `http://host:port/get.php?username=X&password=Y`
    pub fn from_url(raw: &str) -> Result<XtreamCredentials> {
        let raw = raw.trim();
        let url = url::Url::parse(raw)
            .map_err(|e| anyhow!("Invalid Xtream URL: {} ({})", raw, e))?;

        let query = url.query_pairs();
        let mut q_user = None;
        let mut q_pass = None;
        for (k, v) in query {
            match k.as_ref() {
                "username" => q_user = Some(v.to_string()),
                "password" => q_pass = Some(v.to_string()),
                _ => {}
            }
        }

        if let (Some(u), Some(p)) = (q_user, q_pass) {
            // Normalise the base to scheme://host[:port]
            let base = format!(
                "{}://{}",
                url.scheme(),
                url.host_str().ok_or_else(|| anyhow!("Missing host in Xtream URL"))?
            );
            let base = if let Some(port) = url.port() {
                format!("{}:{}", base, port)
            } else {
                base
            };
            return Ok(XtreamCredentials {
                base_url: base,
                username: u,
                password: p,
            });
        }

        // Otherwise expect path-based: /[c/]username/password
        let segs: Vec<&str> = url
            .path_segments()
            .map(|s| s.filter(|x| !x.is_empty()).collect())
            .unwrap_or_default();

        let (user, pass) = match segs.as_slice() {
            [user, pass] => (user.to_string(), pass.to_string()),
            ["c", user, pass] => (user.to_string(), pass.to_string()),
            _ => {
                return Err(anyhow!(
                    "Could not extract Xtream username/password from URL: {}",
                    raw
                ))
            }
        };

        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("Missing host in Xtream URL"))?;
        let base = if let Some(port) = url.port() {
            format!("{}://{}:{}", url.scheme(), host, port)
        } else {
            format!("{}://{}", url.scheme(), host)
        };

        Ok(XtreamCredentials {
            base_url: base,
            username: user,
            password: pass,
        })
    }
}

impl XtreamClient {
    /// Authenticate against the provider and return refreshed credentials
    /// (uses the canonical server URL when the provider reports one).
    pub async fn authenticate(&self) -> Result<XtreamCredentials> {
        let endpoint = format!(
            "{}/player_api.php?username={}&password={}",
            self.creds.base_url, self.creds.username, self.creds.password
        );
        let resp: AuthResponse = self
            .http
            .get(&endpoint)
            .send()
            .await
            .map_err(|e| anyhow!("Xtream auth request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Xtream auth response invalid: {}", e))?;

        if let Some(ui) = resp.user_info {
            if let Some(auth) = ui.auth {
                if auth != 1 {
                    return Err(anyhow!(
                        "Xtream authentication failed (status: {:?})",
                        ui.status
                    ));
                }
            }
            // Server may report a different canonical URL; prefer it.
            if let Some(si) = resp.server_info {
                if let Some(server_url) = si.url {
                    let proto = si.protocol.unwrap_or_else(|| "http".to_string());
                    let base = match si.port {
                        Some(port) if !port.is_empty() => {
                            format!("{}://{}:{}", proto, server_url, port)
                        }
                        _ => format!("{}://{}", proto, server_url),
                    };
                    return Ok(XtreamCredentials {
                        base_url: base,
                        username: ui.username,
                        password: ui.password,
                    });
                }
            }
            return Ok(XtreamCredentials {
                base_url: self.creds.base_url.clone(),
                username: ui.username,
                password: ui.password,
            });
        }

        Err(anyhow!("Xtream authentication returned no user info"))
    }

    /// Fetch all live categories and live streams, returning playable channels.
    pub async fn get_live_streams(&self) -> Result<Vec<XtreamChannel>> {
        let base = &self.creds.base_url;
        let user = &self.creds.username;
        let pass = &self.creds.password;

        let cats: Vec<Category> = self
            .http
            .get(format!(
                "{}/player_api.php?username={}&password={}&action=get_live_categories",
                base, user, pass
            ))
            .send()
            .await
            .map_err(|e| anyhow!("Xtream categories request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Xtream categories response invalid: {}", e))?;

        let cat_name: std::collections::HashMap<String, String> = cats
            .into_iter()
            .map(|c| (c.category_id, c.category_name))
            .collect();

        let streams: Vec<LiveStream> = self
            .http
            .get(format!(
                "{}/player_api.php?username={}&password={}&action=get_live_streams",
                base, user, pass
            ))
            .send()
            .await
            .map_err(|e| anyhow!("Xtream streams request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Xtream streams response invalid: {}", e))?;

        let mut channels = Vec::with_capacity(streams.len());
        for s in streams {
            let url = format!("{}/live/{}/{}/{}.m3u8", base, user, pass, s.stream_id);
            let archive_on = matches!(s.tv_archive.as_deref(), Some("1") | Some("true"));
            let (catchup_source, catchup_days) = if archive_on {
                (
                    Some("?utc={utc}&lutc={lutc}".to_string()),
                    s.tv_archive_duration
                        .filter(|v| !v.is_empty())
                        .or_else(|| Some("7".to_string())),
                )
            } else {
                (None, None)
            };
            channels.push(XtreamChannel {
                name: s.name,
                url,
                icon: if s.stream_icon.is_empty() {
                    None
                } else {
                    Some(s.stream_icon)
                },
                group: cat_name.get(&s.category_id).cloned(),
                tvg_id: s.epg_channel_id.filter(|v| !v.is_empty()),
                catchup_source,
                catchup_days,
            });
        }

        info!("Xtream: resolved {} live streams", channels.len());
        Ok(channels)
    }

    /// Build a VOD (movie) direct-play URL.
    pub fn vod_url(&self, stream_id: i64, ext: &str) -> String {
        format!(
            "{}/vod/{}/{}/{}.{}",
            self.creds.base_url, self.creds.username, self.creds.password, stream_id, ext
        )
    }

    /// Build a Series episode direct-play URL.
    pub fn series_url(&self, episode_id: i64, ext: &str) -> String {
        format!(
            "{}/series/{}/{}/{}.{}",
            self.creds.base_url, self.creds.username, self.creds.password, episode_id, ext
        )
    }

    /// Fetch VOD (movie) categories + streams, returning playable channels.
    pub async fn get_vod_streams(&self) -> Result<Vec<XtreamChannel>> {
        let base = &self.creds.base_url;
        let user = &self.creds.username;
        let pass = &self.creds.password;

        let cats: Vec<Category> = self
            .http
            .get(format!(
                "{}/player_api.php?username={}&password={}&action=get_vod_categories",
                base, user, pass
            ))
            .send()
            .await
            .map_err(|e| anyhow!("Xtream VOD categories request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Xtream VOD categories response invalid: {}", e))?;

        let cat_name: std::collections::HashMap<String, String> = cats
            .into_iter()
            .map(|c| (c.category_id, c.category_name))
            .collect();

        let streams: Vec<VodStream> = self
            .http
            .get(format!(
                "{}/player_api.php?username={}&password={}&action=get_vod_streams",
                base, user, pass
            ))
            .send()
            .await
            .map_err(|e| anyhow!("Xtream VOD request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Xtream VOD response invalid: {}", e))?;

        let mut channels = Vec::with_capacity(streams.len());
        for s in streams {
            let ext = if s.container_extension.is_empty() {
                "mp4".to_string()
            } else {
                s.container_extension.clone()
            };
            channels.push(XtreamChannel {
                name: s.name,
                url: self.vod_url(s.stream_id, &ext),
                icon: if s.stream_icon.is_empty() {
                    None
                } else {
                    Some(s.stream_icon)
                },
                group: cat_name.get(&s.category_id).cloned(),
                tvg_id: s.rating_key.filter(|v| !v.is_empty()),
                catchup_source: None,
                catchup_days: None,
            });
        }

        info!("Xtream: resolved {} VOD streams", channels.len());
        Ok(channels)
    }

    /// Fetch Series and all of their episodes, returning playable channels.
    pub async fn get_series_episodes(&self) -> Result<Vec<XtreamChannel>> {
        let base = &self.creds.base_url;
        let user = &self.creds.username;
        let pass = &self.creds.password;

        let series: Vec<SeriesItem> = self
            .http
            .get(format!(
                "{}/player_api.php?username={}&password={}&action=get_series",
                base, user, pass
            ))
            .send()
            .await
            .map_err(|e| anyhow!("Xtream series request failed: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow!("Xtream series response invalid: {}", e))?;

        let mut channels = Vec::new();
        for sr in series {
            let info: SeriesInfo = self
                .http
                .get(format!(
                    "{}/player_api.php?username={}&password={}&action=get_series_info&series_id={}",
                    base, user, pass, sr.series_id
                ))
                .send()
                .await
                .map_err(|e| anyhow!("Xtream series info request failed: {}", e))?
                .json()
                .await
                .map_err(|e| anyhow!("Xtream series info response invalid: {}", e))?;

            for (_season, eps) in info.seasons {
                for ep in eps {
                    let ext = if ep.container_extension.is_empty() {
                        "mp4".to_string()
                    } else {
                        ep.container_extension.clone()
                    };
                    channels.push(XtreamChannel {
                        name: format!("{} — {}", sr.name, ep.title),
                        url: self.series_url(ep.id, &ext),
                        icon: if sr.cover.is_empty() { None } else { Some(sr.cover.clone()) },
                        group: Some(sr.name.clone()),
                        tvg_id: None,
                        catchup_source: None,
                        catchup_days: None,
                    });
                }
            }
        }

        info!("Xtream: resolved {} series episodes", channels.len());
        Ok(channels)
    }

    /// Build the XMLTV EPG URL for this provider (commonly `/xmltv.php`).
    pub fn epg_url(&self) -> String {
        format!(
            "{}/xmltv.php?username={}&password={}",
            self.creds.base_url, self.creds.username, self.creds.password
        )
    }
}

impl XtreamCredentials {
    /// Create a client for these credentials.
    pub fn client(&self) -> XtreamClient {
        XtreamClient {
            creds: self.clone(),
            http: reqwest::Client::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_url_path() {
        let c = XtreamCredentials::from_url("http://host:8080/myuser/mypass").unwrap();
        assert_eq!(c.base_url, "http://host:8080");
        assert_eq!(c.username, "myuser");
        assert_eq!(c.password, "mypass");
    }

    #[test]
    fn test_from_url_c_prefix() {
        let c = XtreamCredentials::from_url("http://host:8080/c/myuser/mypass/").unwrap();
        assert_eq!(c.base_url, "http://host:8080");
        assert_eq!(c.username, "myuser");
        assert_eq!(c.password, "mypass");
    }

    #[test]
    fn test_from_url_query() {
        let c = XtreamCredentials::from_url(
            "http://host:8080/player_api.php?username=foo&password=bar",
        )
        .unwrap();
        assert_eq!(c.base_url, "http://host:8080");
        assert_eq!(c.username, "foo");
        assert_eq!(c.password, "bar");
    }

    #[test]
    fn test_from_url_invalid() {
        assert!(XtreamCredentials::from_url("http://host:8080/justone").is_err());
        assert!(XtreamCredentials::from_url("not a url").is_err());
    }

    #[test]
    fn test_vod_and_series_urls() {
        let c = XtreamCredentials {
            base_url: "http://host:8080".into(),
            username: "u".into(),
            password: "p".into(),
        }
        .client();
        assert_eq!(c.vod_url(7, "mkv"), "http://host:8080/vod/u/p/7.mkv");
        assert_eq!(c.series_url(9, "mp4"), "http://host:8080/series/u/p/9.mp4");
    }
}
