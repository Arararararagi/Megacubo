#![cfg_attr(
  all(not(debug_assertions), target_os = "windows"),
  windows_subsystem = "windows"
)]

#[cfg(feature = "desktop")]
mod app {
    use tauri::Manager;
    use tauri::Emitter;
    use futures_util::TryStreamExt;
    use serde::Serialize;
    use tracing::info;
    use megacubo::{
        Database, default_db_path, M3uParser,
        ListManager, Streamer,
        config::{Config, PlexConfig, Settings},
        db::Channel, epg::{EpgManager, EpgProgramme},
        lists::{List, ListType, ListStatus, DiscoveryManager, DiscoveryEntry}, parser::M3uEntry,
        xtream::{XtreamCredentials, XtreamChannel},
        plex::{PlexClient, PlexPin, PlexServer, PlexLibrary, PlexItem, PlexPlayable,
               generate_client_id, create_pin, poll_pin, fetch_servers},
    };

    pub fn run() {
        let mut builder = tauri::Builder::default()
            .setup(|app| {
                let _ = tracing_subscriber::fmt::try_init();
                let db_path = default_db_path()?;
                let db = tauri::async_runtime::block_on(Database::new(db_path.clone()))?;

                // Refresh EPG for any lists that already have a guide wired up,
                // so the schedule is populated on launch without manual clicks
                // (respecting the user's "auto-update EPG" setting), and keep it
                // fresh on a periodic interval thereafter.
                let handle = app.handle().clone();
                let startup_db = db.clone();
                tauri::async_runtime::spawn(async move {
                    let auto = Config::load()
                        .await
                        .map(|c| c.auto_update_epg)
                        .unwrap_or(true);
                    if auto {
                        refresh_all_epg(&startup_db, &handle).await;
                    }
                });

                // Periodic EPG refresh loop, driven by the configured interval.
                let loop_db = db.clone();
                let loop_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        let secs = Config::load()
                            .await
                            .map(|c| c.epg_update_interval_secs)
                            .unwrap_or(1800)
                            .max(60) as u64;
                        tokio::time::sleep(tokio::time::Duration::from_secs(secs)).await;
                        let auto = Config::load()
                            .await
                            .map(|c| c.auto_update_epg)
                            .unwrap_or(true);
                        if auto {
                            refresh_all_epg(&loop_db, &loop_handle).await;
                        }
                    }
                });

                // Seed the Discovery (community) table with curated public
                // sources on first launch, so the Discover tab is populated.
                let seed_db = db.clone();
                tauri::async_runtime::spawn(async move {
                    let dm = DiscoveryManager::new(seed_db.pool().clone());
                    if let Ok(existing) = dm.get_all().await {
                        if existing.is_empty() {
                            for s in megacubo::discovery::sources() {
                                let _ = dm
                                    .add(&DiscoveryEntry {
                                        id: None,
                                        url: s.url,
                                        name: Some(s.name),
                                        entry_type: "iptv-org".to_string(),
                                        image: None,
                                        health: None,
                                        last_seen: None,
                                    })
                                    .await;
                            }
                            info!("Seeded {} discovery sources", megacubo::discovery::sources().len());
                        }
                    }
                });

                app.manage(db);

                #[cfg(feature = "media")]
                {
                    let streamer = Streamer::new(true, None);
                    app.manage(playback::PlayerState {
                        streamer: std::sync::Mutex::new(streamer),
                    });
                }

                println!("Megacubo initialized with database at: {:?}", db_path);
                Ok(())
            });

        #[cfg(feature = "media")]
        {
            builder = builder.invoke_handler(tauri::generate_handler![
                add_m3u_list,
                add_xtream_list,
                get_lists,
                remove_list,
                get_channels,
                search_channels,
                add_bookmark,
                get_bookmarks,
                add_history,
                get_history,
                refresh_epg,
                get_epg_schedule,
                catchup_url,
                get_discovery,
                launch_external_player,
                plex_login_start,
                plex_login_poll,
                plex_servers,
                plex_save_server,
                plex_libraries,
                plex_browse,
                plex_seasons,
                plex_episodes,
                plex_item_url,
                plex_set_watched,
                plex_refresh,
                get_settings,
                set_settings,
                playback::init_player,
                playback::play_in_app,
                playback::pause_in_app,
                playback::resume_in_app,
                playback::stop_in_app,
                playback::set_volume,
                playback::get_time,
                playback::get_duration,
                playback::seek,
            ]);
        }

        #[cfg(not(feature = "media"))]
        {
            builder = builder.invoke_handler(tauri::generate_handler![
                add_m3u_list,
                add_xtream_list,
                get_lists,
                remove_list,
                get_channels,
                search_channels,
                add_bookmark,
                get_bookmarks,
                add_history,
                get_history,
                refresh_epg,
                get_epg_schedule,
                catchup_url,
                get_discovery,
                launch_external_player,
                plex_login_start,
                plex_login_poll,
                plex_servers,
                plex_save_server,
                plex_libraries,
                plex_browse,
                plex_seasons,
                plex_episodes,
                plex_item_url,
                plex_set_watched,
                plex_refresh,
                get_settings,
                set_settings,
            ]);
        }

        builder
            .run(tauri::generate_context!())
            .expect("error while running tauri application");
    }

    /// Add an M3U playlist: stream-download, parse and store its channels.
    /// An optional `epg_url` links an XMLTV guide to the list.
    #[tauri::command]
    async fn add_m3u_list(
        url: String,
        epg_url: Option<String>,
        db: tauri::State<'_, Database>,
    ) -> Result<String, String> {
        let list_manager = ListManager::new(db.pool().clone());
        let _ = list_manager.add_list(&List::new(url.clone(), ListType::M3u)).await;
        if let Some(epg) = &epg_url {
            let _ = list_manager.set_epg_url(&url, epg).await;
        }

        // Derive the base URL to resolve relative media URLs.
        let base = url
            .rsplit_once('/')
            .map(|(p, _)| p.to_string())
            .unwrap_or_else(|| url.clone());

        let response = reqwest::get(&url).await.map_err(|e| e.to_string())?;
        let parser = M3uParser::new()
            .map_err(|e| e.to_string())?
            .with_base_url(base);

        // Stream the response body and parse it incrementally to avoid loading
        // the whole playlist into memory.
        let stream = response
            .bytes_stream()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
        let reader = tokio_util::io::StreamReader::new(Box::pin(stream));

        let mut entries: Vec<M3uEntry> = Vec::new();
        parser
            .parse_stream(reader, |entry| {
                entries.push(entry);
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())?;

        let count = entries.len();
        for entry in &entries {
            let _ = db.insert_channel(entry, &url).await;
        }

        let status = if count > 0 { ListStatus::Loaded } else { ListStatus::Error };
        let err = if count == 0 { Some("no channels parsed") } else { None };
        let _ = list_manager.update_status(&url, status, err).await;

        Ok(format!("Parsed {} channels from {}", count, url))
    }

    /// Add an Xtream Codes playlist: authenticate, fetch live categories +
    /// streams, and store them as channels. The EPG URL is auto-derived.
    #[tauri::command]
    async fn add_xtream_list(
        url: String,
        db: tauri::State<'_, Database>,
    ) -> Result<String, String> {
        let creds = XtreamCredentials::from_url(&url).map_err(|e| e.to_string())?;
        let client = creds.client();
        let auth = client.authenticate().await.map_err(|e| e.to_string())?;
        let client = auth.client();

        let list_manager = ListManager::new(db.pool().clone());
        let _ = list_manager
            .add_list(&List::new(auth.base_url.clone(), ListType::Xtream))
            .await;

        // Auto-wire the provider's XMLTV EPG so "Load EPG" works.
        let epg_url = client.epg_url();
        let _ = list_manager.set_epg_url(&auth.base_url, &epg_url).await;

        let mut streams: Vec<XtreamChannel> = client
            .get_live_streams()
            .await
            .map_err(|e| e.to_string())?;

        if let Ok(vod) = client.get_vod_streams().await {
            let n = vod.len();
            streams.extend(vod);
            info!("Xtream: added {} VOD titles", n);
        }
        if let Ok(series) = client.get_series_episodes().await {
            let n = series.len();
            streams.extend(series);
            info!("Xtream: added {} series episodes", n);
        }

        let count = streams.len();
        for s in &streams {
            let entry = M3uEntry {
                name: s.name.clone(),
                url: s.url.clone(),
                icon: s.icon.clone(),
                group: s.group.clone(),
                tvg_id: s.tvg_id.clone(),
                catchup: s.catchup_source.clone().map(|_| "default".to_string()),
                catchup_source: s.catchup_source.clone(),
                catchup_days: s.catchup_days.clone(),
                tvg_name: None,
                tvg_logo: s.icon.clone(),
                tvg_country: None,
                tvg_language: None,
                tvg_shift: None,
            };
            let _ = db.insert_channel(&entry, &auth.base_url).await;
        }

        let status = if count > 0 { ListStatus::Loaded } else { ListStatus::Error };
        let err = if count == 0 { Some("no channels returned") } else { None };
        let _ = list_manager.update_status(&auth.base_url, status, err).await;

        Ok(format!("Loaded {} Xtream channels from {}", count, auth.base_url))
    }

    /// Get all configured lists
    #[tauri::command]
    async fn get_lists(db: tauri::State<'_, Database>) -> Result<Vec<List>, String> {
        ListManager::new(db.pool().clone())
            .get_all()
            .await
            .map_err(|e| e.to_string())
    }

    /// Remove a playlist and all of its stored channels.
    #[tauri::command]
    async fn remove_list(
        list_url: String,
        db: tauri::State<'_, Database>,
    ) -> Result<String, String> {
        let lists = ListManager::new(db.pool().clone());
        let _ = db.delete_channels_by_list(&list_url).await;
        lists
            .delete(&list_url)
            .await
            .map_err(|e| e.to_string())?;
        Ok(format!("Removed playlist {}", list_url))
    }

    /// Build a catch-up (time-shifted) playback URL from a channel's
    /// `catchup-source` template and the chosen window (Unix timestamps).
    #[tauri::command]
    fn catchup_url(
        channel_url: String,
        catchup_source: String,
        start_utc: i64,
        end_utc: Option<i64>,
    ) -> String {
        megacubo::parser::build_catchup_url(&channel_url, &catchup_source, start_utc, end_utc)
    }

    /// Return the curated community (Discovery) sources.
    #[tauri::command]
    async fn get_discovery(
        db: tauri::State<'_, Database>,
    ) -> Result<Vec<megacubo::lists::DiscoveryEntry>, String> {
        DiscoveryManager::new(db.pool().clone())
            .get_all()
            .await
            .map_err(|e| e.to_string())
    }

    // ----- Plex -----

    /// Ensure a stable client id exists in the config; return it.
    async fn ensure_plex_client_id() -> Result<String, String> {
        let mut cfg = Config::load().await.map_err(|e| e.to_string())?;
        if let Some(p) = &cfg.plex {
            if !p.client_id.is_empty() {
                return Ok(p.client_id.clone());
            }
        }
        let id = generate_client_id();
        cfg.plex = Some(PlexConfig {
            client_id: id.clone(),
            auth_token: String::new(),
            server_url: String::new(),
            server_name: String::new(),
        });
        cfg.save().await.map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Start a Plex login PIN (returns code + auth URL to show the user).
    #[tauri::command]
    async fn plex_login_start() -> Result<PlexPin, String> {
        let client_id = ensure_plex_client_id().await?;
        create_pin(&client_id).await.map_err(|e| e.to_string())
    }

    /// Poll a Plex login PIN; returns the token once authenticated.
    #[tauri::command]
    async fn plex_login_poll(pin_id: String) -> Result<Option<String>, String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        let client_id = cfg
            .plex
            .map(|p| p.client_id)
            .unwrap_or_else(|| "megacubo".to_string());
        poll_pin(&pin_id, &client_id).await.map_err(|e| e.to_string())
    }

    /// Discover the user's Plex Media Servers for the given token.
    #[tauri::command]
    async fn plex_servers(token: String) -> Result<Vec<PlexServer>, String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        let client_id = cfg
            .plex
            .map(|p| p.client_id)
            .unwrap_or_else(|| "megacubo".to_string());
        fetch_servers(&token, &client_id).await.map_err(|e| e.to_string())
    }

    /// Persist the chosen server + token, completing the login.
    #[tauri::command]
    async fn plex_save_server(
        server_url: String,
        name: String,
        token: String,
    ) -> Result<String, String> {
        let mut cfg = Config::load().await.map_err(|e| e.to_string())?;
        let client_id = match &cfg.plex {
            Some(p) if !p.client_id.is_empty() => p.client_id.clone(),
            _ => generate_client_id(),
        };
        cfg.plex = Some(PlexConfig {
            client_id,
            auth_token: token,
            server_url,
            server_name: name,
        });
        cfg.save().await.map_err(|e| e.to_string())?;
        Ok("Plex server saved".to_string())
    }

    /// List libraries/sections (requires a saved server).
    #[tauri::command]
    async fn plex_libraries() -> Result<Vec<PlexLibrary>, String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        let p = cfg.plex.ok_or_else(|| "Plex not configured".to_string())?;
        PlexClient::new(p.server_url, p.auth_token, p.client_id)
            .libraries()
            .await
            .map_err(|e| e.to_string())
    }

    /// List items (movies or series) in a section.
    #[tauri::command]
    async fn plex_browse(section: String) -> Result<Vec<PlexItem>, String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        let p = cfg.plex.ok_or_else(|| "Plex not configured".to_string())?;
        PlexClient::new(p.server_url, p.auth_token, p.client_id)
            .browse(&section)
            .await
            .map_err(|e| e.to_string())
    }

    /// List seasons of a series.
    #[tauri::command]
    async fn plex_seasons(rating_key: String) -> Result<Vec<PlexItem>, String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        let p = cfg.plex.ok_or_else(|| "Plex not configured".to_string())?;
        PlexClient::new(p.server_url, p.auth_token, p.client_id)
            .seasons(&rating_key)
            .await
            .map_err(|e| e.to_string())
    }

    /// List episodes of a season.
    #[tauri::command]
    async fn plex_episodes(rating_key: String) -> Result<Vec<PlexItem>, String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        let p = cfg.plex.ok_or_else(|| "Plex not configured".to_string())?;
        PlexClient::new(p.server_url, p.auth_token, p.client_id)
            .episodes(&rating_key)
            .await
            .map_err(|e| e.to_string())
    }

    /// Resolve the direct-play URL + metadata for a movie/episode.
    #[tauri::command]
    async fn plex_item_url(rating_key: String) -> Result<PlexPlayable, String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        let p = cfg.plex.ok_or_else(|| "Plex not configured".to_string())?;
        PlexClient::new(p.server_url, p.auth_token, p.client_id)
            .playable(&rating_key)
            .await
            .map_err(|e| e.to_string())
    }

    /// Mark a Plex item (movie/episode/season/show) watched or unwatched.
    #[tauri::command]
    async fn plex_set_watched(rating_key: String, watched: bool) -> Result<(), String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        let p = cfg.plex.ok_or_else(|| "Plex not configured".to_string())?;
        PlexClient::new(p.server_url, p.auth_token, p.client_id)
            .set_watched(&rating_key, watched)
            .await
            .map_err(|e| e.to_string())
    }

    /// Refresh a Plex item's metadata from its agent.
    #[tauri::command]
    async fn plex_refresh(rating_key: String) -> Result<(), String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        let p = cfg.plex.ok_or_else(|| "Plex not configured".to_string())?;
        PlexClient::new(p.server_url, p.auth_token, p.client_id)
            .refresh(&rating_key)
            .await
            .map_err(|e| e.to_string())
    }

    /// Get channels for a list (first page)
    #[tauri::command]
    async fn get_channels(
        list_url: String,
        db: tauri::State<'_, Database>,
    ) -> Result<Vec<Channel>, String> {
        db.get_channels(&list_url, 500, 0)
            .await
            .map_err(|e| e.to_string())
    }

    /// Search channels by name or group
    #[tauri::command]
    async fn search_channels(
        query: String,
        db: tauri::State<'_, Database>,
    ) -> Result<Vec<Channel>, String> {
        db.search_channels(&query, 100).await.map_err(|e| e.to_string())
    }

    /// Add a bookmark for a channel
    #[tauri::command]
    async fn add_bookmark(
        channel_name: String,
        channel_url: String,
        icon: Option<String>,
        db: tauri::State<'_, Database>,
    ) -> Result<(), String> {
        db.add_bookmark(&channel_name, &channel_url, icon.as_deref())
            .await
            .map_err(|e| e.to_string())
    }

    /// Get all bookmarks
    #[tauri::command]
    async fn get_bookmarks(
        db: tauri::State<'_, Database>,
    ) -> Result<Vec<(String, String, Option<String>)>, String> {
        db.get_bookmarks().await.map_err(|e| e.to_string())
    }

    /// Record a played channel in history
    #[tauri::command]
    async fn add_history(
        channel_name: String,
        channel_url: String,
        icon: Option<String>,
        duration: Option<i64>,
        db: tauri::State<'_, Database>,
    ) -> Result<(), String> {
        db.add_history(&channel_name, &channel_url, icon.as_deref(), duration)
            .await
            .map_err(|e| e.to_string())
    }

    /// Get recent history
    #[tauri::command]
    async fn get_history(
        limit: i64,
        db: tauri::State<'_, Database>,
    ) -> Result<Vec<(String, String, Option<String>, i64)>, String> {
        db.get_history(limit).await.map_err(|e| e.to_string())
    }

    /// Fetch an XMLTV EPG URL, parse it and store it for a list
    #[tauri::command]
    /// Fetch an XMLTV EPG and store it (channels + programmes).
    async fn refresh_epg_for(
        db: &Database,
        list_url: &str,
        epg_url: &str,
    ) -> Result<String, String> {
        let list_manager = ListManager::new(db.pool().clone());
        let _ = list_manager.set_epg_url(list_url, epg_url).await;

        let response = reqwest::get(epg_url).await.map_err(|e| e.to_string())?;
        let content = response.text().await.map_err(|e| e.to_string())?;

        let epg = EpgManager::new(db.pool().clone());
        let (channels, programmes) = epg
            .parse_and_store(&content, epg_url)
            .await
            .map_err(|e| e.to_string())?;

        Ok(format!(
            "Stored {} channels and {} programmes from EPG",
            channels, programmes
        ))
    }

    #[tauri::command]
    async fn refresh_epg(
        list_url: String,
        epg_url: String,
        db: tauri::State<'_, Database>,
        app: tauri::AppHandle,
    ) -> Result<String, String> {
        emit_epg(&app, "list", &list_url, &format!("Refreshing {}…", list_url));
        let res = refresh_epg_for(&db, &list_url, &epg_url).await;
        match &res {
            Ok(msg) => emit_epg(&app, "list-done", &list_url, msg),
            Err(e) => emit_epg(&app, "error", &list_url, &format!("{}: {}", list_url, e)),
        }
        res
    }

    /// Progress payload emitted to the frontend during EPG refresh.
    #[derive(Clone, Serialize)]
    pub struct EpgProgress {
        pub phase: String,
        pub list_url: String,
        pub message: String,
    }

    fn emit_epg(handle: &tauri::AppHandle, phase: &str, list_url: &str, message: &str) {
        let _ = handle.emit(
            "epg-progress",
            EpgProgress {
                phase: phase.to_string(),
                list_url: list_url.to_string(),
                message: message.to_string(),
            },
        );
    }

    /// Refresh the XMLTV guide for every list that has an `epg_url` wired,
    /// emitting progress events as each list is processed.
    async fn refresh_all_epg(db: &Database, handle: &tauri::AppHandle) {
        emit_epg(handle, "start", "", "Starting EPG refresh…");
        let lists = match ListManager::new(db.pool().clone()).get_all().await {
            Ok(l) => l,
            Err(e) => {
                emit_epg(handle, "error", "", &format!("Failed to list playlists: {}", e));
                return;
            }
        };
        let mut done = 0;
        for l in lists {
            if let Some(epg) = &l.epg_url {
                if !epg.is_empty() {
                    emit_epg(handle, "list", &l.url, &format!("Refreshing {}…", l.url));
                    match refresh_epg_for(db, &l.url, epg).await {
                        Ok(msg) => {
                            emit_epg(handle, "list-done", &l.url, &msg);
                            done += 1;
                        }
                        Err(e) => emit_epg(handle, "error", &l.url, &format!("{}: {}", l.url, e)),
                    }
                }
            }
        }
        emit_epg(handle, "done", "", &format!("EPG refresh complete ({} lists)", done));
    }

    /// Get the upcoming EPG schedule (current + future programmes) for a channel
    #[tauri::command]
    async fn get_epg_schedule(
        channel_id: String,
        limit: i64,
        db: tauri::State<'_, Database>,
    ) -> Result<Vec<EpgProgramme>, String> {
        EpgManager::new(db.pool().clone())
            .get_schedule(&channel_id, limit)
            .await
            .map_err(|e| e.to_string())
    }

    /// Launch an external player for a stream URL
    #[tauri::command]
    async fn launch_external_player(url: String) -> Result<(), String> {
        let cfg = Config::load().await.map_err(|e| e.to_string())?;
        Streamer::new(false, cfg.external_player)
            .launch_external_player(&url)
            .map_err(|e| e.to_string())
    }

    /// Return the current user settings (no secret Plex token).
    #[tauri::command]
    async fn get_settings() -> Result<Settings, String> {
        Ok(Config::load().await.map_err(|e| e.to_string())?.settings())
    }

    /// Persist user settings.
    #[tauri::command]
    async fn set_settings(settings: Settings) -> Result<(), String> {
        let mut cfg = Config::load().await.map_err(|e| e.to_string())?;
        cfg.apply_settings(settings);
        cfg.save().await.map_err(|e| e.to_string())
    }

    /// In-app playback commands (require `media` feature)
    #[cfg(feature = "media")]
    mod playback {
        use super::*;
        use std::sync::Mutex;
        use tauri::State;

        /// Managed MPV player state
        pub struct PlayerState {
            pub streamer: Mutex<Streamer>,
        }

        /// Initialize the in-app player
        #[tauri::command]
        pub async fn init_player(state: State<'_, PlayerState>) -> Result<(), String> {
            let mut streamer = state.streamer.lock().map_err(|e| e.to_string())?;
            *streamer = Streamer::new(true, None);
            Ok(())
        }

        /// Play a stream in-app
        #[tauri::command]
        pub async fn play_in_app(url: String, state: State<'_, PlayerState>) -> Result<(), String> {
            let streamer = state.streamer.lock().map_err(|e| e.to_string())?;
            streamer.play_in_app(&url).map_err(|e| e.to_string())
        }

        /// Pause in-app playback
        #[tauri::command]
        pub async fn pause_in_app(state: State<'_, PlayerState>) -> Result<(), String> {
            let streamer = state.streamer.lock().map_err(|e| e.to_string())?;
            streamer.pause_in_app().map_err(|e| e.to_string())
        }

        /// Resume in-app playback
        #[tauri::command]
        pub async fn resume_in_app(state: State<'_, PlayerState>) -> Result<(), String> {
            let streamer = state.streamer.lock().map_err(|e| e.to_string())?;
            streamer.resume_in_app().map_err(|e| e.to_string())
        }

        /// Stop in-app playback
        #[tauri::command]
        pub async fn stop_in_app(state: State<'_, PlayerState>) -> Result<(), String> {
            let streamer = state.streamer.lock().map_err(|e| e.to_string())?;
            streamer.stop_in_app().map_err(|e| e.to_string())
        }

        /// Set volume (0.0 - 100.0)
        #[tauri::command]
        pub async fn set_volume(volume: f64, state: State<'_, PlayerState>) -> Result<(), String> {
            let streamer = state.streamer.lock().map_err(|e| e.to_string())?;
            if let Some(ref mpv) = streamer.mpv {
                mpv.set_volume(volume).map_err(|e| e.to_string())
            } else {
                Err("In-app playback not available".to_string())
            }
        }

        /// Get current playback time
        #[tauri::command]
        pub async fn get_time(state: State<'_, PlayerState>) -> Result<Option<f64>, String> {
            let streamer = state.streamer.lock().map_err(|e| e.to_string())?;
            if let Some(ref mpv) = streamer.mpv {
                mpv.get_time_pos().map_err(|e| e.to_string())
            } else {
                Ok(None)
            }
        }

        /// Get total duration
        #[tauri::command]
        pub async fn get_duration(state: State<'_, PlayerState>) -> Result<Option<f64>, String> {
            let streamer = state.streamer.lock().map_err(|e| e.to_string())?;
            if let Some(ref mpv) = streamer.mpv {
                mpv.get_duration().map_err(|e| e.to_string())
            } else {
                Ok(None)
            }
        }

        /// Seek to position (seconds)
        #[tauri::command]
        pub async fn seek(pos: f64, state: State<'_, PlayerState>) -> Result<(), String> {
            let streamer = state.streamer.lock().map_err(|e| e.to_string())?;
            if let Some(ref mpv) = streamer.mpv {
                mpv.set_time_pos(pos).map_err(|e| e.to_string())
            } else {
                Err("In-app playback not available".to_string())
            }
        }
    }
}

#[cfg(feature = "desktop")]
fn main() {
    app::run();
}

#[cfg(not(feature = "desktop"))]
fn main() {
    println!("Megacubo library built. Run with --features desktop to launch the GUI.");
}
