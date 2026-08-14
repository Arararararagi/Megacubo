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
    #[serde(default, rename = "epg_channel_id")]
    epg_channel_id: Option<String>,
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
            });
        }

        info!("Xtream: resolved {} live streams", channels.len());
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
}
