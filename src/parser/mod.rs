use std::io::BufRead;
use regex::Regex;
use tracing::info;
use tokio::io::{AsyncRead, AsyncBufReadExt};

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

/// Mutable accumulator for the streaming line processor.
#[derive(Default)]
struct M3uParseState {
    count: usize,
    current_name: Option<String>,
    current_group: Option<String>,
    current_tvg_id: Option<String>,
    current_tvg_name: Option<String>,
    current_tvg_logo: Option<String>,
    current_tvg_country: Option<String>,
    current_tvg_language: Option<String>,
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

    /// Feed a single (already-trimmed) line into the parser state machine.
    fn feed_line(
        &self,
        state: &mut M3uParseState,
        trimmed: &str,
        callback: &mut impl FnMut(M3uEntry) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        // Non-comment, non-empty lines are media URLs.
        if trimmed.is_empty() || !trimmed.starts_with('#') {
            if !trimmed.is_empty() {
                let name = state
                    .current_name
                    .clone()
                    .or_else(|| state.current_tvg_name.clone())
                    .unwrap_or_else(|| trimmed.to_string());
                let entry = M3uEntry {
                    name,
                    url: trimmed.to_string(),
                    icon: state.current_tvg_logo.clone(),
                    group: state.current_group.clone(),
                    tvg_id: state.current_tvg_id.clone(),
                    tvg_name: state.current_tvg_name.clone(),
                    tvg_logo: state.current_tvg_logo.clone(),
                    tvg_country: state.current_tvg_country.clone(),
                    tvg_language: state.current_tvg_language.clone(),
                };
                callback(entry)?;
                state.count += 1;

                // Reset per-entry state
                state.current_name = None;
                state.current_tvg_id = None;
                state.current_tvg_name = None;
                state.current_tvg_logo = None;
                state.current_tvg_country = None;
                state.current_tvg_language = None;
            }
            return Ok(());
        }

        // #EXTGRP: sets the group for subsequent entries (alternative to group-title)
        if trimmed.starts_with("#EXTGRP") {
            state.current_group = trimmed.splitn(2, ':').nth(1).map(|s| s.trim().to_string());
            return Ok(());
        }

        // #EXTINF: channel metadata line
        if trimmed.starts_with("#EXTINF") {
            state.current_name = trimmed.rfind(',').map(|idx| trimmed[idx + 1..].trim().to_string());

            state.current_tvg_id = self.extract_attr(&self.tvg_id_re, trimmed);
            state.current_tvg_name = self.extract_attr(&self.tvg_name_re, trimmed);
            state.current_tvg_logo = self.extract_attr(&self.tvg_logo_re, trimmed);
            state.current_tvg_country = self.extract_attr(&self.tvg_country_re, trimmed);
            state.current_tvg_language = self.extract_attr(&self.tvg_language_re, trimmed);
            // Only override the group if this entry specifies one (so #EXTGRP is preserved)
            if let Some(g) = self.extract_attr(&self.group_re, trimmed) {
                state.current_group = Some(g);
            }
        }

        Ok(())
    }

    /// Parse an M3U file from a synchronous reader, calling the callback for each entry
    pub fn parse<R: BufRead>(
        &self,
        reader: R,
        mut callback: impl FnMut(M3uEntry) -> anyhow::Result<()>,
    ) -> anyhow::Result<usize> {
        let mut state = M3uParseState::default();
        for line_result in reader.lines() {
            let line = line_result?;
            self.feed_line(&mut state, line.trim(), &mut callback)?;
        }
        info!("Parsed {} M3U entries", state.count);
        Ok(state.count)
    }

    /// Parse an M3U stream from an async reader (e.g. a network response), calling the
    /// callback for each entry as it is read. Avoids holding the whole document in memory.
    pub async fn parse_stream<R: AsyncRead + Unpin>(
        &self,
        reader: R,
        mut callback: impl FnMut(M3uEntry) -> anyhow::Result<()>,
    ) -> anyhow::Result<usize> {
        let mut state = M3uParseState::default();
        let mut lines = tokio::io::BufReader::new(reader).lines();
        while let Some(line) = lines.next_line().await? {
            self.feed_line(&mut state, line.trim(), &mut callback)?;
        }
        info!("Parsed {} M3U entries (streamed)", state.count);
        Ok(state.count)
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

    #[tokio::test]
    async fn test_parse_stream() {
        let content = "#EXTM3U\n#EXTINF:-1,Chan A\nhttp://example.com/a\n#EXTINF:-1,Chan B\nrtmp://example.com/b\n";
        let parser = M3uParser::new().unwrap();
        let mut entries = Vec::new();
        let count = parser
            .parse_stream(content.as_bytes(), |entry| {
                entries.push(entry);
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!(count, 2);
        assert_eq!(entries[0].name, "Chan A");
        assert_eq!(entries[0].url, "http://example.com/a");
        assert_eq!(entries[1].name, "Chan B");
        assert_eq!(entries[1].url, "rtmp://example.com/b");
    }
}