use std::io::BufRead;
use regex::Regex;
use tracing::info;

/// M3U entry representing a channel
#[derive(Debug, Clone)]
pub struct M3uEntry {
    pub name: String,
    pub url: String,
    pub icon: Option<String>,
    pub group: Option<String>,
    pub tvg_id: Option<String>,
    pub tvg_name: Option<String>,
    pub tvg_logo: Option<String>,
    pub tvg_country: Option<String>,
    pub tvg_language: Option<String>,
}

/// M3U parser with streaming support
pub struct M3uParser {
    // Regex patterns for attribute extraction
    tvg_id_re: Regex,
    tvg_name_re: Regex,
    tvg_logo_re: Regex,
    tvg_country_re: Regex,
    tvg_language_re: Regex,
    group_re: Regex,
}

impl M3uParser {
    /// Create a new M3U parser
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            tvg_id_re: Regex::new(r#"tvg-id="([^"]*)""#)?,
            tvg_name_re: Regex::new(r#"tvg-name="([^"]*)""#)?,
            tvg_logo_re: Regex::new(r#"tvg-logo="([^"]*)""#)?,
            tvg_country_re: Regex::new(r#"tvg-country="([^"]*)""#)?,
            tvg_language_re: Regex::new(r#"tvg-language="([^"]*)""#)?,
            group_re: Regex::new(r#"group-title="([^"]*)""#)?,
        })
    }

    /// Extract attribute value from a line using regex
    fn extract_attr<'a>(&self, re: &Regex, line: &'a str) -> Option<String> {
        re.captures(line).map(|caps| caps[1].to_string())
    }

    /// Parse an M3U file from a reader, calling the callback for each entry
    pub fn parse<R: BufRead>(&self, reader: R, mut callback: impl FnMut(M3uEntry) -> anyhow::Result<()>) -> anyhow::Result<usize> {
        let mut count = 0;
        let mut current_name: Option<String> = None;
        let mut current_group: Option<String> = None;
        let mut current_tvg_id: Option<String> = None;
        let mut current_tvg_name: Option<String> = None;
        let mut current_tvg_logo: Option<String> = None;
        let mut current_tvg_country: Option<String> = None;
        let mut current_tvg_language: Option<String> = None;

        for line_result in reader.lines() {
            let line = line_result?;
            let trimmed = line.trim();

            // Skip empty lines and comments
            if trimmed.is_empty() || !trimmed.starts_with('#') {
                // This is a URL line - create an entry for it
                if !trimmed.is_empty() {
                    let name = current_name
                        .clone()
                        .or_else(|| current_tvg_name.clone())
                        .unwrap_or_else(|| trimmed.to_string());
                    let entry = M3uEntry {
                        name,
                        url: trimmed.to_string(),
                        icon: current_tvg_logo.clone(),
                        group: current_group.clone(),
                        tvg_id: current_tvg_id.clone(),
                        tvg_name: current_tvg_name.clone(),
                        tvg_logo: current_tvg_logo.clone(),
                        tvg_country: current_tvg_country.clone(),
                        tvg_language: current_tvg_language.clone(),
                    };
                    callback(entry)?;
                    count += 1;

                    // Reset for next entry
                    current_name = None;
                    current_tvg_id = None;
                    current_tvg_name = None;
                    current_tvg_logo = None;
                    current_tvg_country = None;
                    current_tvg_language = None;
                }
                continue;
            }

            // #EXTGRP: sets the group for subsequent entries (alternative to group-title)
            if trimmed.starts_with("#EXTGRP") {
                current_group = trimmed.splitn(2, ':').nth(1).map(|s| s.trim().to_string());
                continue;
            }

            // Parse EXTINF line
            if trimmed.starts_with("#EXTINF") {
                // The display name is everything after the last comma
                current_name = trimmed.rfind(',').map(|idx| trimmed[idx + 1..].trim().to_string());

                // Extract attributes
                current_tvg_id = self.extract_attr(&self.tvg_id_re, trimmed);
                current_tvg_name = self.extract_attr(&self.tvg_name_re, trimmed);
                current_tvg_logo = self.extract_attr(&self.tvg_logo_re, trimmed);
                current_tvg_country = self.extract_attr(&self.tvg_country_re, trimmed);
                current_tvg_language = self.extract_attr(&self.tvg_language_re, trimmed);
                // Only override the group if this entry specifies one (so #EXTGRP is preserved)
                if let Some(g) = self.extract_attr(&self.group_re, trimmed) {
                    current_group = Some(g);
                }
            }
        }

        info!("Parsed {} M3U entries", count);
        Ok(count)
    }

    /// Parse M3U content from a string
    pub fn parse_string(&self, content: &str, callback: impl FnMut(M3uEntry) -> anyhow::Result<()>) -> anyhow::Result<usize> {
        let reader = std::io::Cursor::new(content);
        self.parse(reader, callback)
    }
}

impl Default for M3uParser {
    fn default() -> Self {
        Self::new().expect("Failed to create M3U parser - invalid regex")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_m3u() {
        let content = r#"#EXTM3U
#EXTINF:-1 tvg-id="1" tvg-name="Test Channel" tvg-logo="http://example.com/logo.png" group-title="News",Test Channel
http://example.com/stream.m3u8
"#;
        let parser = M3uParser::new().unwrap();
        let mut entries = Vec::new();
        let count = parser.parse_string(content, |entry| {
            entries.push(entry);
            Ok(())
        }).unwrap();
        
        assert_eq!(count, 1);
        assert_eq!(entries[0].name, "Test Channel");
        assert_eq!(entries[0].url, "http://example.com/stream.m3u8");
        assert_eq!(entries[0].group, Some("News".to_string()));
    }

    #[test]
    fn test_parse_extgrp_and_rtmp() {
        let content = r#"#EXTM3U
#EXTGRP:Sports
#EXTINF:-1 tvg-id="2",Live Match
rtmp://example.com/live/match
"#;
        let parser = M3uParser::new().unwrap();
        let mut entries = Vec::new();
        let count = parser.parse_string(content, |entry| {
            entries.push(entry);
            Ok(())
        }).unwrap();

        assert_eq!(count, 1);
        assert_eq!(entries[0].name, "Live Match");
        assert_eq!(entries[0].url, "rtmp://example.com/live/match");
        assert_eq!(entries[0].group, Some("Sports".to_string()));
    }
}