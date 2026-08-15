//! Curated "community" playlist sources shown in the Discover tab.
//!
//! These are public, openly-distributed IPTV playlists. They are bundled in the
//! app so the user can one-click add them; the actual channel data is fetched at
//! runtime from each source's URL (cloud fetch).

use serde::Serialize;

/// A community playlist a user can add directly.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoverySource {
    pub name: String,
    pub url: String,
    pub category: String,
    pub description: String,
}

const BASE: &str = "https://iptv-org.github.io/iptv";

/// Return the bundled set of community sources.
pub fn sources() -> Vec<DiscoverySource> {
    let mut v = Vec::new();

    v.push(DiscoverySource {
        name: "iptv-org — All Channels".to_string(),
        url: format!("{}/index.m3u", BASE),
        category: "General".to_string(),
        description: "The full public iptv-org index (thousands of channels worldwide).".to_string(),
    });

    for (cat, name) in [
        ("news", "News"),
        ("sports", "Sports"),
        ("movies", "Movies"),
        ("music", "Music"),
        ("documentary", "Documentary"),
        ("kids", "Kids"),
        ("education", "Education"),
        ("travel", "Travel"),
    ] {
        v.push(DiscoverySource {
            name: format!("iptv-org — {}", name),
            url: format!("{}/categories/{}.m3u", BASE, cat),
            category: "By category".to_string(),
            description: format!("Curated {} channels from iptv-org.", name.to_lowercase()),
        });
    }

    for (cc, name) in [("us", "United States"), ("uk", "United Kingdom"), ("br", "Brazil")] {
        v.push(DiscoverySource {
            name: format!("iptv-org — {}", name),
            url: format!("{}/countries/{}.m3u", BASE, cc),
            category: "By country".to_string(),
            description: format!("Public channels broadcast in {}.", name),
        });
    }

    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sources_nonempty_and_https() {
        let s = sources();
        assert!(!s.is_empty());
        for src in &s {
            assert!(src.url.starts_with("https://"), "source must be https: {}", src.url);
            assert!(!src.name.is_empty());
        }
    }
}
