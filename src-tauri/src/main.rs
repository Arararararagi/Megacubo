#![cfg_attr(
  all(not(debug_assertions), target_os = "windows"),
  windows_subsystem = "windows"
)]

#[cfg(feature = "desktop")]
mod app {
    use tauri::Manager;
    use futures_util::TryStreamExt;
    use megacubo::{
        Database, default_db_path, M3uParser,
        ListManager, Streamer,
        db::Channel, epg::{EpgManager, EpgProgramme},
        lists::{List, ListType, ListStatus}, parser::M3uEntry,
    };

    pub fn run() {
        let mut builder = tauri::Builder::default()
            .setup(|app| {
                let _ = tracing_subscriber::fmt::try_init();
                let db_path = default_db_path()?;
                let db = tauri::async_runtime::block_on(Database::new(db_path.clone()))?;
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
                get_lists,
                get_channels,
                search_channels,
                add_bookmark,
                get_bookmarks,
                add_history,
                get_history,
                refresh_epg,
                get_epg_schedule,
                launch_external_player,
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
                get_lists,
                get_channels,
                search_channels,
                add_bookmark,
                get_bookmarks,
                add_history,
                get_history,
                refresh_epg,
                get_epg_schedule,
                launch_external_player,
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

    /// Get all configured lists
    #[tauri::command]
    async fn get_lists(db: tauri::State<'_, Database>) -> Result<Vec<List>, String> {
        ListManager::new(db.pool().clone())
            .get_all()
            .await
            .map_err(|e| e.to_string())
    }

    /// Get channels for a list (first page)
    #[tauri::command]
    async fn get_channels(
        list_url: String,
        db: tauri::State<'_, Database>,
    ) -> Result<Vec<Channel>, String> {
        db.get_channels(&list_url, 100, 0)
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
    async fn refresh_epg(
        list_url: String,
        epg_url: String,
        db: tauri::State<'_, Database>,
    ) -> Result<String, String> {
        let list_manager = ListManager::new(db.pool().clone());
        let _ = list_manager.set_epg_url(&list_url, &epg_url).await;

        let response = reqwest::get(&epg_url).await.map_err(|e| e.to_string())?;
        let content = response.text().await.map_err(|e| e.to_string())?;

        let epg = EpgManager::new(db.pool().clone());
        let (channels, programmes) = epg
            .parse_and_store(&content, &epg_url)
            .await
            .map_err(|e| e.to_string())?;

        Ok(format!(
            "Stored {} channels and {} programmes from EPG",
            channels, programmes
        ))
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
        Streamer::new(false, None)
            .launch_external_player(&url)
            .map_err(|e| e.to_string())
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
