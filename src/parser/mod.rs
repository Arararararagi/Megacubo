use std::io::BufRead;
use regex::Regex;
use tracing::info;
use tokio::io::{AsyncRead, AsyncBufReadExt};
use url::Url;

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
    pub catchup: Option<String>,
    pub catchup_source: Option<String>,
    pub catchup_days: Option<String>,
    pub tvg_shift: Option<String>,
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
    current_catchup: Option<String>,
    current_catchup_source: Option<String>,
    current_catchup_days: Option<String>,
    current_tvg_shift: Option<String>,
}

impl M3uParseState {
    fn reset_entry(&mut self) {
        self.current_name = None;
        self.current_tvg_id = None;
        self.current_tvg_name = None;
        self.current_tvg_logo = None;
        self.current_tvg_country = None;
        self.current_tvg_language = None;
        self.current_catchup = None;
        self.current_catchup_source = None;
        self.current_catchup_days = None;
        self.current_tvg_shift = None;
    }
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
    catchup_re: Regex,
    catchup_source_re: Regex,
    catchup_days_re: Regex,
    tvg_shift_re: Regex,
    hls_name_re: Regex,
    hls_resolution_re: Regex,
    /// Base URL used to resolve relative media URLs.
    base_url: Option<String>,
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
            catchup_re: Regex::new(r#"catchup="([^"]*)""#)?,
            catchup_source_re: Regex::new(r#"catchup-source="([^"]*)""#)?,
            catchup_days_re: Regex::new(r#"catchup-days="([^"]*)""#)?,
            tvg_shift_re: Regex::new(r#"tvg-shift="([^"]*)""#)?,
            hls_name_re: Regex::new(r#"NAME="([^"]*)""#)?,
            hls_resolution_re: Regex::new(r#"RESOLUTION=([0-9]+x[0-9]+)"#)?,
            base_url: None,
        })
    }

    /// Set the playlist base URL used to resolve relative media URLs.
    pub fn with_base_url(mut self, base: String) -> Self {
        self.base_url = Some(base);
        self
    }

    /// Extract attribute value from a line using regex
    fn extract_attr<'a>(&self, re: &Regex, line: &'a str) -> Option<String> {
        re.captures(line).map(|caps| caps[1].to_string())
    }

    /// Resolve a (possibly relative) media URL against the playlist base URL.
    fn resolve_url(&self, raw: &str) -> String {
        let is_absolute = raw.starts_with("http://")
            || raw.starts_with("https://")
            || raw.starts_with("rtmp://")
            || raw.starts_with("rtmps://")
            || raw.starts_with("rtsp://")
            || raw.starts_with("mms://")
            || raw.starts_with("rtp://")
            || raw.starts_with("udp://")
            || raw.starts_with("srt://");
        if is_absolute {
            return raw.to_string();
        }
        if let Some(base) = &self.base_url {
            if let Ok(b) = Url::parse(base) {
                if let Ok(joined) = b.join(raw) {
                    return joined.to_string();
                }
            }
        }
        raw.to_string()
    }

    /// Feed a single (already-trimmed) line into the parser state machine.
    fn feed_line(
        &self,
        state: &mut M3uParseState,
        trimmed_in: &str,
        callback: &mut impl FnMut(M3uEntry) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        // Strip a leading UTF-8 BOM (some editors emit one on the first line).
        let trimmed = trimmed_in.trim_start_matches('\u{feff}');

        // Empty lines carry no information.
        if trimmed.is_empty() {
            return Ok(());
        }

        // Non-comment lines are media URLs.
        if !trimmed.starts_with('#') {
            let url = self.resolve_url(trimmed);
            let name = state
                .current_name
                .clone()
                .or_else(|| state.current_tvg_name.clone())
                .unwrap_or_else(|| trimmed.to_string());
            let entry = M3uEntry {
                name,
                url,
                icon: state.current_tvg_logo.clone(),
                group: state.current_group.clone(),
                tvg_id: state.current_tvg_id.clone(),
                tvg_name: state.current_tvg_name.clone(),
                tvg_logo: state.current_tvg_logo.clone(),
                tvg_country: state.current_tvg_country.clone(),
                tvg_language: state.current_tvg_language.clone(),
                catchup: state.current_catchup.clone(),
                catchup_source: state.current_catchup_source.clone(),
                catchup_days: state.current_catchup_days.clone(),
                tvg_shift: state.current_tvg_shift.clone(),
            };
            callback(entry)?;
            state.count += 1;
            state.reset_entry();
            return Ok(());
        }

        // #EXTGRP: sets the group for subsequent entries (alternative to group-title)
        if trimmed.starts_with("#EXTGRP") {
            state.current_group = trimmed.splitn(2, ':').nth(1).map(|s| s.trim().to_string());
            return Ok(());
        }

        // #EXTINF: channel metadata line
        if trimmed.starts_with("#EXTINF") {
            state.current_name =
                trimmed.rfind(',').map(|idx| trimmed[idx + 1..].trim().to_string());

            state.current_tvg_id = self.extract_attr(&self.tvg_id_re, trimmed);
            state.current_tvg_name = self.extract_attr(&self.tvg_name_re, trimmed);
            state.current_tvg_logo = self.extract_attr(&self.tvg_logo_re, trimmed);
            state.current_tvg_country = self.extract_attr(&self.tvg_country_re, trimmed);
            state.current_tvg_language = self.extract_attr(&self.tvg_language_re, trimmed);
            // Only override the group if this entry specifies one (so #EXTGRP is preserved)
            if let Some(g) = self.extract_attr(&self.group_re, trimmed) {
                state.current_group = Some(g);
            }
            state.current_catchup = self.extract_attr(&self.catchup_re, trimmed);
            state.current_catchup_source = self.extract_attr(&self.catchup_source_re, trimmed);
            state.current_catchup_days = self.extract_attr(&self.catchup_days_re, trimmed);
            state.current_tvg_shift = self.extract_attr(&self.tvg_shift_re, trimmed);
            return Ok(());
        }

        // #EXT-X-STREAM-INF: HLS master playlist variant — the following line is the
        // (relative) variant URL, so treat it as a channel with a derived name.
        if trimmed.starts_with("#EXT-X-STREAM-INF") {
            let name = self
                .extract_attr(&self.hls_name_re, trimmed)
                .or_else(|| self.extract_attr(&self.hls_resolution_re, trimmed));
            state.current_name = name;
            return Ok(());
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

    #[test]
    fn test_parse_catchup_and_shift() {
        let content = r#"#EXTM3U
#EXTINF:-1 tvg-id="3" tvg-shift="-2" catchup="append" catchup-days="7" catchup-source="?utc=${start}" group-title="Catchup",Catchup Chan
http://example.com/c.m3u8
"#;
        let parser = M3uParser::new().unwrap();
        let mut entries = Vec::new();
        parser.parse_string(content, |entry| {
            entries.push(entry);
            Ok(())
        }).unwrap();

        assert_eq!(entries[0].tvg_shift, Some("-2".to_string()));
        assert_eq!(entries[0].catchup, Some("append".to_string()));
        assert_eq!(entries[0].catchup_days, Some("7".to_string()));
        assert_eq!(entries[0].catchup_source, Some("?utc=${start}".to_string()));
    }

    #[test]
    fn test_parse_bom_and_relative_urls() {
        let content = "\u{feff}#EXTM3U\n#EXTINF:-1,Rel Chan\nplaylist/rel.m3u8\n";
        let base = "http://example.com/tv/playlist.m3u";
        let parser = M3uParser::new().unwrap().with_base_url(base.to_string());
        let mut entries = Vec::new();
        parser.parse_string(content, |entry| {
            entries.push(entry);
            Ok(())
        }).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Rel Chan");
        assert_eq!(entries[0].url, "http://example.com/tv/playlist/rel.m3u8");
    }

    #[test]
    fn test_parse_hls_stream_inf() {
        let content = r#"#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=640x360
http://example.com/low/index.m3u8
#EXT-X-STREAM-INF:NAME="HD 1080",BANDWIDTH=5000000,RESOLUTION=1920x1080
http://example.com/hd/index.m3u8
"#;
        let parser = M3uParser::new().unwrap();
        let mut entries = Vec::new();
        parser.parse_string(content, |entry| {
            entries.push(entry);
            Ok(())
        }).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].url, "http://example.com/low/index.m3u8");
        assert_eq!(entries[0].name, "640x360");
        assert_eq!(entries[1].url, "http://example.com/hd/index.m3u8");
        assert_eq!(entries[1].name, "HD 1080");
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

/// Build a catch-up (time-shifted) playback URL for a channel.
///
/// `catchup_source` is the provider-supplied suffix, e.g. `?utc={utc}&lutc={lutc}`
/// (type `append`/`default`) or `?utc={utc}` (type `shift`). The `{utc}` and
/// `{lutc}` tokens are replaced with Unix timestamps (seconds); the result is
/// appended directly to `channel_url`. `end_utc` is used for `{lutc}` and falls
/// back to `start_utc` when absent (single-timestamp shift).
pub fn build_catchup_url(
    channel_url: &str,
    catchup_source: &str,
    start_utc: i64,
    end_utc: Option<i64>,
) -> String {
    if catchup_source.is_empty() {
        return channel_url.to_string();
    }
    let end = end_utc.unwrap_or(start_utc);
    let suffix = catchup_source
        .replace("{utc}", &start_utc.to_string())
        .replace("{lutc}", &end.to_string());
    format!("{}{}", channel_url, suffix)
}

#[cfg(test)]
mod catchup_tests {
    use super::*;

    #[test]
    fn test_build_catchup_append() {
        let url = build_catchup_url(
            "http://h/live/1",
            "?utc={utc}&lutc={lutc}",
            1_700_000_000,
            Some(1_700_003_600),
        );
        assert_eq!(url, "http://h/live/1?utc=1700000000&lutc=1700003600");
    }

    #[test]
    fn test_build_catchup_shift_falls_back_to_start() {
        let url = build_catchup_url("http://h/live/1", "?utc={utc}", 1_700_000_000, None);
        assert_eq!(url, "http://h/live/1?utc=1700000000");
    }

    #[test]
    fn test_build_catchup_empty_source() {
        assert_eq!(
            build_catchup_url("http://h/live/1", "", 1, None),
            "http://h/live/1"
        );
    }
}
