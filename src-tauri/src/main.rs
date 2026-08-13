#![cfg_attr(
  all(not(debug_assertions), target_os = "windows"),
  windows_subsystem = "windows"
)]

#[cfg(feature = "desktop")]
mod app {
    use tauri::Manager;
    use megacubo::{
        Database, default_db_path, M3uParser,
        ListManager, Streamer,
        db::Channel, epg::{EpgManager, EpgProgramme},
        lists::{List, ListType, ListStatus}, parser::M3uEntry,
    };

    pub fn run() {
        tauri::Builder::default()
            .setup(|app| {
                let _ = tracing_subscriber::fmt::try_init();
                let db_path = default_db_path()?;
                let db = tauri::async_runtime::block_on(Database::new(db_path.clone()))?;
                app.manage(db);
                println!("Megacubo initialized with database at: {:?}", db_path);
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                add_m3u_list,
                get_lists,
                get_channels,
                get_epg_programme,
                launch_external_player,
            ])
            .run(tauri::generate_context!())
            .expect("error while running tauri application");
    }

    /// Add an M3U playlist: download, parse and store its channels
    #[tauri::command]
    async fn add_m3u_list(url: String, db: tauri::State<'_, Database>) -> Result<String, String> {
        let list_manager = ListManager::new(db.pool().clone());
        let _ = list_manager
            .add_list(&List::new(url.clone(), ListType::M3u))
            .await;

        let response = reqwest::get(&url).await.map_err(|e| e.to_string())?;
        let content = response.text().await.map_err(|e| e.to_string())?;

        let parser = M3uParser::new().map_err(|e| e.to_string())?;
        let mut entries: Vec<M3uEntry> = Vec::new();
        parser
            .parse_string(&content, |entry| {
                entries.push(entry);
                Ok(())
            })
            .map_err(|e| e.to_string())?;

        let count = entries.len();
        for entry in &entries {
            let _ = db.insert_channel(entry, &url).await;
        }

        let status = if count > 0 { ListStatus::Loaded } else { ListStatus::Error };
        let err = if count == 0 {
            Some("no channels parsed".to_string())
        } else {
            None
        };
        let _ = list_manager.update_status(&url, status, err.as_deref()).await;

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

    /// Get the current EPG programme for a channel
    #[tauri::command]
    async fn get_epg_programme(
        channel_name: String,
        db: tauri::State<'_, Database>,
    ) -> Result<Option<EpgProgramme>, String> {
        EpgManager::new(db.pool().clone())
            .get_current_programme(&channel_name)
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
}

#[cfg(feature = "desktop")]
fn main() {
    app::run();
}

#[cfg(not(feature = "desktop"))]
fn main() {
    println!("Megacubo library built. Run with --features desktop to launch the GUI.");
}
