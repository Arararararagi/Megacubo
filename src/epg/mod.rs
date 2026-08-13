use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, FromRow};
use tracing::info;
use chrono::{DateTime, Utc, NaiveDateTime, NaiveDate, NaiveTime, FixedOffset, TimeZone};
use quick_xml::events::Event;
use quick_xml::reader::Reader;

/// EPG programme entry
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EpgProgramme {
    pub id: Option<i64>,
    pub channel_name: String,
    pub start_time: i64,
    pub end_time: i64,
    pub title: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub categories: Option<String>,
    pub terms: Option<String>,
    pub age: Option<i32>,
    pub lang: Option<String>,
    pub country: Option<String>,
    pub rating: Option<String>,
    pub parental: Option<String>,
    pub content_type: Option<String>,
    pub epg_url: Option<String>,
    pub created_at: Option<i64>,
}

/// EPG channel metadata
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EpgChannel {
    pub id: Option<i64>,
    pub name: String,
    pub icon: Option<String>,
    pub terms: Option<String>,
    pub updated_at: Option<i64>,
}

/// EPG manager for handling XMLTV data
pub struct EpgManager {
    pool: SqlitePool,
}

impl EpgManager {
    /// Create a new EPG manager
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Get programmes for a channel within a time range
    pub async fn get_programmes(
        &self,
        channel_name: &str,
        start_time: i64,
        end_time: i64,
    ) -> anyhow::Result<Vec<EpgProgramme>> {
        let programmes = sqlx::query_as::<_, EpgProgramme>(
            "SELECT * FROM epg_programmes 
             WHERE channel_name = ? AND start_time <= ? AND end_time >= ?
             ORDER BY start_time",
        )
        .bind(channel_name)
        .bind(end_time)
        .bind(start_time)
        .fetch_all(&self.pool)
        .await?;

        Ok(programmes)
    }

    /// Get current programme for a channel
    pub async fn get_current_programme(&self, channel_name: &str) -> anyhow::Result<Option<EpgProgramme>> {
        let now = Utc::now().timestamp();
        let programme = sqlx::query_as::<_, EpgProgramme>(
            "SELECT * FROM epg_programmes 
             WHERE channel_name = ? AND start_time <= ? AND end_time >= ?
             ORDER BY start_time DESC LIMIT 1",
        )
        .bind(channel_name)
        .bind(now)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;

        Ok(programme)
    }

    /// Parse an XMLTV document and store its channels and programmes.
    /// Returns the number of `(channels, programmes)` stored. Existing data for
    /// `epg_url` is cleared first to avoid duplicates on refresh.
    pub async fn parse_and_store(&self, content: &str, epg_url: &str) -> anyhow::Result<(usize, usize)> {
        let (channels, programmes) = parse_xmltv(content)?;
        self.clear_epg(epg_url).await?;

        let now = Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;

        for ch in &channels {
            sqlx::query(
                "INSERT INTO epg_channels (name, icon, terms, updated_at) \
                 VALUES (?, ?, ?, ?) \
                 ON CONFLICT(name) DO UPDATE SET icon = excluded.icon, updated_at = excluded.updated_at",
            )
            .bind(&ch.id)
            .bind(&ch.icon)
            .bind(Option::<String>::None)
            .bind(now)
            .execute(&mut tx)
            .await?;
        }

        for p in &programmes {
            let start = match p.start {
                Some(s) => s,
                None => continue, // a programme without a start time is unusable
            };
            let stop = p.stop.unwrap_or(start);
            sqlx::query(
                "INSERT INTO epg_programmes \
                 (channel_name, start_time, end_time, title, description, icon, categories, epg_url, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&p.channel)
            .bind(start)
            .bind(stop)
            .bind(&p.title)
            .bind(&p.desc)
            .bind(&p.icon)
            .bind(Some(p.categories.join("; ")))
            .bind(epg_url)
            .bind(now)
            .execute(&mut tx)
            .await?;
        }

        tx.commit().await?;
        info!(
            "Stored {} channels and {} programmes from {}",
            channels.len(),
            programmes.len(),
            epg_url
        );
        Ok((channels.len(), programmes.len()))
    }

    /// Insert a programme into the database
    pub async fn insert_programme(&self, programme: &EpgProgramme) -> anyhow::Result<i64> {
        let now = Utc::now().timestamp();
        let result = sqlx::query_as::<_, (i64,)>(
            r#"
            INSERT INTO epg_programmes (
                channel_name, start_time, end_time, title, description, icon,
                categories, terms, age, lang, country, rating, parental,
                content_type, epg_url, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&programme.channel_name)
        .bind(programme.start_time)
        .bind(programme.end_time)
        .bind(&programme.title)
        .bind(&programme.description)
        .bind(&programme.icon)
        .bind(&programme.categories)
        .bind(&programme.terms)
        .bind(programme.age)
        .bind(&programme.lang)
        .bind(&programme.country)
        .bind(&programme.rating)
        .bind(&programme.parental)
        .bind(&programme.content_type)
        .bind(&programme.epg_url)
        .bind(programme.created_at.unwrap_or(now))
        .fetch_one(&self.pool)
        .await?;

        Ok(result.0)
    }

    /// Clear EPG data for a specific URL
    pub async fn clear_epg(&self, epg_url: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM epg_programmes WHERE epg_url = ?")
            .bind(epg_url)
            .execute(&self.pool)
            .await?;

        sqlx::query("DELETE FROM epg_channels WHERE name LIKE ?",)
            .bind(format!("%{}%", epg_url))
            .execute(&self.pool)
            .await?;

        info!("Cleared EPG data for: {}", epg_url);
        Ok(())
    }
}

/// Parse XMLTV time format to Unix timestamp
pub fn parse_xmltv_time(time_str: &str) -> anyhow::Result<i64> {
    // XMLTV format: YYYYMMDDHHMMSS +/-HHMM
    // Example: 20240101120000 +0000
    let time_part = &time_str[0..14];
    let sign = &time_str[15..16];
    let tz_hour: i32 = time_str[16..18].parse()?;
    let tz_min: i32 = time_str[18..20].parse()?;

    let year: i32 = time_part[0..4].parse()?;
    let month: u32 = time_part[4..6].parse()?;
    let day: u32 = time_part[6..8].parse()?;
    let hour: u32 = time_part[8..10].parse()?;
    let min: u32 = time_part[10..12].parse()?;
    let sec: u32 = time_part[12..14].parse()?;

    let tz_offset_secs = if sign == "+" {
        tz_hour * 3600 + tz_min * 60
    } else {
        -((tz_hour * 3600 + tz_min * 60))
    };

    // Create naive datetime and convert to timestamp
    let dt = NaiveDateTime::new(
        NaiveDate::from_ymd_opt(year, month, day)
            .ok_or_else(|| anyhow::anyhow!("Invalid date"))?,
        NaiveTime::from_hms_opt(hour, min, sec)
            .ok_or_else(|| anyhow::anyhow!("Invalid time"))?,
    );

    let offset = FixedOffset::east_opt(tz_offset_secs)
        .ok_or_else(|| anyhow::anyhow!("Invalid timezone offset"))?;
    
    // Convert to UTC timestamp
    let datetime: DateTime<Utc> = offset.from_utc_datetime(&dt).with_timezone(&Utc);

    Ok(datetime.timestamp())
}

/// A channel parsed from an XMLTV `<channel>` element.
#[derive(Debug, Clone)]
pub struct XmltvChannel {
    pub id: String,
    pub display_name: Option<String>,
    pub icon: Option<String>,
}

/// A programme parsed from an XMLTV `<programme>` element.
#[derive(Debug, Clone)]
pub struct XmltvProgramme {
    pub channel: String,
    pub start: Option<i64>,
    pub stop: Option<i64>,
    pub title: Option<String>,
    pub desc: Option<String>,
    pub categories: Vec<String>,
    pub icon: Option<String>,
}

fn xml_attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == key {
            return Some(String::from_utf8_lossy(&a.value).into_owned());
        }
    }
    None
}

/// Parse an XMLTV document into channels and programmes using a streaming,
/// allocation-light reader. Returns `(channels, programmes)`.
pub fn parse_xmltv(content: &str) -> anyhow::Result<(Vec<XmltvChannel>, Vec<XmltvProgramme>)> {
    let mut reader = Reader::from_str(content);
    reader.trim_text(true);
    let mut buf = Vec::new();

    let mut channels = Vec::new();
    let mut programmes = Vec::new();

    let mut cur_channel: Option<XmltvChannel> = None;
    let mut cur_prog: Option<XmltvProgramme> = None;
    let mut in_display = false;
    let mut in_title = false;
    let mut in_desc = false;
    let mut in_category = false;
    let mut text = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"channel" => {
                    let id = xml_attr(e, b"id").unwrap_or_default();
                    cur_channel = Some(XmltvChannel { id, display_name: None, icon: None });
                }
                b"programme" => {
                    let channel = xml_attr(e, b"channel").unwrap_or_default();
                    let start = xml_attr(e, b"start").and_then(|s| parse_xmltv_time(&s).ok());
                    let stop = xml_attr(e, b"stop").and_then(|s| parse_xmltv_time(&s).ok());
                    cur_prog = Some(XmltvProgramme {
                        channel,
                        start,
                        stop,
                        title: None,
                        desc: None,
                        categories: Vec::new(),
                        icon: None,
                    });
                }
                b"icon" => {
                    let src = xml_attr(e, b"src");
                    if let Some(ch) = cur_channel.as_mut() {
                        ch.icon = src.clone();
                    }
                    if let Some(p) = cur_prog.as_mut() {
                        p.icon = src;
                    }
                }
                b"display-name" => {
                    in_display = true;
                    text.clear();
                }
                b"title" => {
                    in_title = true;
                    text.clear();
                }
                b"desc" => {
                    in_desc = true;
                    text.clear();
                }
                b"category" => {
                    in_category = true;
                    text.clear();
                }
                _ => {}
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"channel" => {
                    let id = xml_attr(e, b"id").unwrap_or_default();
                    channels.push(XmltvChannel { id, display_name: None, icon: xml_attr(e, b"src") });
                }
                b"programme" => {
                    let channel = xml_attr(e, b"channel").unwrap_or_default();
                    let start = xml_attr(e, b"start").and_then(|s| parse_xmltv_time(&s).ok());
                    let stop = xml_attr(e, b"stop").and_then(|s| parse_xmltv_time(&s).ok());
                    programmes.push(XmltvProgramme {
                        channel,
                        start,
                        stop,
                        title: None,
                        desc: None,
                        categories: Vec::new(),
                        icon: xml_attr(e, b"src"),
                    });
                }
                b"icon" => {
                    let src = xml_attr(e, b"src");
                    if let Some(ch) = cur_channel.as_mut() {
                        ch.icon = src.clone();
                    }
                    if let Some(p) = cur_prog.as_mut() {
                        p.icon = src;
                    }
                }
                _ => {}
            },
            Ok(Event::Text(ref t)) => {
                if in_display || in_title || in_desc || in_category {
                    text.push_str(t.unescape().map_err(|e| anyhow::anyhow!("{}", e))?.as_ref());
                }
            }
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"channel" => {
                    if let Some(ch) = cur_channel.take() {
                        channels.push(ch);
                    }
                }
                b"programme" => {
                    if let Some(p) = cur_prog.take() {
                        programmes.push(p);
                    }
                }
                b"display-name" => {
                    if let Some(ch) = cur_channel.as_mut() {
                        ch.display_name = Some(text.trim().to_string());
                    }
                    in_display = false;
                    text.clear();
                }
                b"title" => {
                    if let Some(p) = cur_prog.as_mut() {
                        p.title = Some(text.trim().to_string());
                    }
                    in_title = false;
                    text.clear();
                }
                b"desc" => {
                    if let Some(p) = cur_prog.as_mut() {
                        p.desc = Some(text.trim().to_string());
                    }
                    in_desc = false;
                    text.clear();
                }
                b"category" => {
                    if let Some(p) = cur_prog.as_mut() {
                        if !text.trim().is_empty() {
                            p.categories.push(text.trim().to_string());
                        }
                    }
                    in_category = false;
                    text.clear();
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(anyhow::anyhow!("XMLTV parse error: {}", e)),
        }
    }

    Ok((channels, programmes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <channel id="C1">
    <display-name>Channel One</display-name>
    <icon src="http://example.com/c1.png"/>
  </channel>
  <programme start="20240101120000 +0000" stop="20240101130000 +0000" channel="C1">
    <title>News</title>
    <desc>Daily news</desc>
    <category>News</category>
    <category>Live</category>
  </programme>
</tv>"#;

    #[test]
    fn test_parse_xmltv() {
        let (channels, programmes) = parse_xmltv(SAMPLE).unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].id, "C1");
        assert_eq!(channels[0].display_name.as_deref(), Some("Channel One"));
        assert_eq!(channels[0].icon.as_deref(), Some("http://example.com/c1.png"));

        assert_eq!(programmes.len(), 1);
        let p = &programmes[0];
        assert_eq!(p.channel, "C1");
        assert_eq!(p.start, Some(1704110400));
        assert_eq!(p.stop, Some(1704114000));
        assert_eq!(p.title.as_deref(), Some("News"));
        assert_eq!(p.desc.as_deref(), Some("Daily news"));
        assert_eq!(p.categories, vec!["News".to_string(), "Live".to_string()]);
    }

    #[tokio::test]
    async fn test_parse_and_store() {
        let path = std::env::temp_dir().join(format!("megacubo_epg_test_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let db = Database::new(path.clone()).await.unwrap();
        let epg = EpgManager::new(db.pool().clone());

        let (c, p) = epg.parse_and_store(SAMPLE, "http://example.com/epg.xml").await.unwrap();
        assert_eq!((c, p), (1, 1));

        let progs = epg.get_programmes("C1", 0, i64::MAX).await.unwrap();
        assert_eq!(progs.len(), 1);
        assert_eq!(progs[0].title, "News");
        assert_eq!(progs[0].categories.as_deref(), Some("News; Live"));

        let _ = std::fs::remove_file(&path);
    }
}