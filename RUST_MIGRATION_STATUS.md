# Megacubo → Rust Migration: Status & Next Steps

> Companion to `RUST_REWRITE_PLAN.md` (the original technical proposal). This document tracks what is actually built, the decisions made, and the remaining work.

## 1. Goal
Rewrite the Electron/JS-based Megacubo IPTV player in Rust, using **Tauri v2** as the desktop shell, **Tokio** for async, **SQLite** (via `sqlx`) for storage, and **libmpv** for playback. **Desktop-only** — targets Windows, macOS, and Linux. Android is dropped. Phased rollout in §6.

## 2. Current Architecture
- **Package**: `megacubo-rs` (root `Cargo.toml`), edition 2021. Produces a `lib` (+ `cdylib`) crate `megacubo` and a `bin` `megacubo`.
- **Library source**: `src/` → `config`, `db`, `epg`, `lists`, `parser`, `streamer`, `xtream`, `plex` (`src/lib.rs` re-exports the public API).
- **Desktop entry**: `src-tauri/src/main.rs` (Tauri commands + setup).
- **Config/asset files** (repo root): `tauri.conf.json` (v2 schema), `build.rs`, `dist/index.html`, `icons/icon.png`.
- **Targets**: Windows, macOS, Linux (desktop-only; no Android).
- **Features**: `default = []`, `desktop = ["tauri"]`, `media = ["libmpv"]`, `node-module = ["napi","napi-derive"]`.

### Dependency stack (pinned in `Cargo.lock`)
| Concern | Crate |
|---|---|
| Async runtime | `tokio` 1 (full) |
| HTTP | `reqwest` 0.11 (rustls) |
| SQLite | `sqlx` 0.6 (sqlite, migrate, json) |
| Serialization | `serde`, `serde_json` |
| M3U parsing | `regex` (custom parser) |
| EPG (planned) | `quick-xml` 0.31 (XMLTV streaming parser) |
| Dates | `chrono` 0.4 |
| LRU / arena | `lru` 0.12, `bumpalo` 3 |
| External player detect | `which` 4 |
| Logging | `tracing`, `tracing-subscriber` |
| App data dir | `dirs` 5 |
| Desktop shell | `tauri` 2.11.5 (+ `tauri-build` 2.6.3, `wry` 0.55.1) |

**Toolchain**: Rust **1.97.1** (upgraded from 1.85.1 because `tauri-build` requires ≥1.88).

## 3. What Has Been Done

### 3.1 Build unblocked (this session)
- `Cargo.toml`: split `desktop` from a new debugging-friendly `media` feature so the GUI builds **without** system libmpv; added `dirs` + `tauri-build`.
- `src-tauri/src/main.rs`: entire Tauri entry gated behind `#[cfg(feature = "desktop")]` with a fallback `main()` for default builds; `add_m3u_list` now downloads → parses → **stores channels** (replacing the old `// TODO`); `get_lists`/`get_channels`/`get_epg_programme`/`launch_external_player` are real commands.
- `src/db/mod.rs`: `default_db_path()` now uses `dirs::data_dir()`; added `Channel` type + `insert_channel()` + `get_channels()` (paginated).
- `src/parser/mod.rs`: extracts the display name after the comma (not just `tvg-name`); accepts non-`http` URLs (e.g. `rtmp://`).
- Tauri: moved `tauri.conf.json` to repo root with **valid v2 schema**; added `build.rs`, `dist/index.html`, `icons/icon.png`.
- `.gitignore`: added `.megacubo/` and `/Megacubo/`.

### 3.2 Resolution fixes
- Relaxed the `url = "=2.4"` / `idna` / `home` pins. The original ICU-avoiding pins made a consistent Tauri impossible (`tauri-runtime-wry 2.1.2` is a broken release — declares `wry ^0.46` but uses 0.47 API). Locked a coherent set: `tauri 2.11.5`, `wry 0.55.1`, `url 2.5.8` (pulls `icu_*`). This is fine for the desktop-only targets (Windows/Linux/macOS).

### 3.3 Tests
- `parser::tests::test_parse_simple_m3u` (existing).
- `db::tests::test_insert_and_get_channel` (new) — validates the ingestion pipeline (insert → query).
- `epg::tests::test_parse_xmltv` — XMLTV parse (channels + programmes, times, categories).
- `epg::tests::test_parse_and_store` — XMLTV parse → SQLite store → query round-trip.
- `parser::tests::test_parse_extgrp_and_rtmp` — `#EXTGRP` group + non-`http` URL.
- `streamer::tests::test_from_content` — stream-type detection from bytes (TS / HLS / DASH / unknown).
- `streamer::tests::test_probe_stream` — URL-based probe (`.m3u8`/`.mpd`/`.ts`/rtmp/http).
- `xtream::tests::test_from_url_path` / `test_from_url_c_prefix` / `test_from_url_query` / `test_from_url_invalid` — Xtream URL parsing (path, `/c/` prefix, query, invalid).

### 3.4 EPG, storage & parser hardening (Sprint A progress)
- **EPG XMLTV parser**: added `epg::parse_xmltv` — a streaming `quick-xml` reader producing `XmltvChannel`/`XmltvProgramme`. `EpgManager::parse_and_store` bulk-inserts channels + programmes in a transaction (clears the previous `epg_url` first) and `get_schedule` returns the upcoming programme list for a channel.
- **M3U parser hardening**: `#EXTGRP:` group support; display name after the comma; non-`http` URLs accepted; **`catchup`/`catchup-source`/`catchup-days`/`tvg-shift` attributes** parsed into `M3uEntry` and persisted; **UTF-8 BOM** stripping; **relative-URL resolution** against the playlist base URL; **HLS `#EXT-X-STREAM-INF`** variants recorded as channels (with derived name from `NAME`/`RESOLUTION`).
- **Bookmarks / History / Search storage**: `Database` gained `add_bookmark`/`get_bookmarks`, `add_history`/`get_history`, and `search_channels` (LIKE on name/group). The channels schema also stores `catchup*`, `tvg_shift`, and the `tvg_*` metadata; a `migrate()` step adds missing columns on older DBs.
- **Streaming download**: `M3uParser::parse_stream` parses an `AsyncRead` incrementally (used by `add_m3u_list` via `tokio-util::StreamReader` over the reqwest byte stream) — avoids loading the whole playlist into memory.
- **Tauri commands wired**: `add_m3u_list` (streaming + accepts an optional EPG URL + resolves relative URLs), `get_lists`, `get_channels`, `search_channels`, `add_bookmark`/`get_bookmarks`, `add_history`/`get_history`, `refresh_epg`, `get_epg_schedule`, `launch_external_player`. `ListManager` gained `set_epg_url`/`get_by_url`.
- **In-app playback (libmpv)**: behind the `media` feature. `streamer::MpvPlayer` wraps `libmpv::Mpv` (a managed `PlayerState` in the Tauri app). Tauri commands `init_player`, `play_in_app`, `pause_in_app`, `resume_in_app`, `stop_in_app`, `set_volume`, `get_time`, `get_duration`, `seek` drive playback. Requires the system `libmpv` library at link time (see `.cargo/config.toml` for the macOS Homebrew path).
- **Xtream Codes support**: `xtream::XtreamClient` parses provider URLs (path `/user/pass`, `/c/user/pass`, `player_api.php?username=…&password=…`), authenticates, and fetches live categories + streams. `add_xtream_list` Tauri command stores channels (`{base}/live/{user}/{pass}/{id}.m3u8`) into the channels, auto-wires the provider's XMLTV EPG (`/xmltv.php`). UI has an M3U/Xtream type selector.
- **Plex support**: `plex::PlexClient` (full **PIN/OAuth login** via `clients.plex.tv` → server discovery → chosen connection) browses **Movies + TV Shows** (sections → series → seasons → episodes) and resolves **direct-play** URLs (`/library/parts/<id>/file?X-Plex-Token=…`). `PlexConfig` persists token + server in `config.json`. Tauri commands: `plex_login_start`, `plex_login_poll`, `plex_servers`, `plex_save_server`, `plex_libraries`, `plex_browse`, `plex_seasons`, `plex_episodes`, `plex_item_url`. Playback reuses the existing `play_in_app` / `launch_external_player` (mpv plays the Plex HTTP URL directly — **no transcoding** in v1). Dedicated **Plex tab** in the UI.
- **EPG auto-refresh**: on launch, the app refreshes the XMLTV guide for every list that has an `epg_url` wired (background task), so the schedule is populated without manual clicks. `remove_list` deletes a playlist and all its channels; UI has a **Remove** button.

### 3.5 Functional UI (`dist/index.html`)
- Replaced the placeholder with a self-contained vanilla-JS app (no build step) driven by `window.__TAURI__.core.invoke` (global Tauri enabled via `app.withGlobalTauri`).
- Features: **add playlist** (M3U URL + optional EPG URL), **channel grid** per list with **Play / ▶ App / Bookmark / EPG** actions, **search**, **bookmarks** and **history** lists, an **EPG schedule** view (current + upcoming programmes), and a dedicated **Plex tab** (sign in with Plex → libraries → movies/series → seasons → episodes → play in-app or external). The **▶ App** button (shown only when the binary was built with the `media` feature) plays in-app via libmpv and shows a player bar (play/pause/stop, seek, volume). External **Play** launches the OS default / VLC. Playback records history; bookmarking records bookmarks. Channels persist in SQLite so they survive restarts (reloaded on launch).
- `tauri.conf.json` embeds `dist/` at build time; `frontendDist` = `dist`, `withGlobalTauri` = `true`.

## 4. Build & Run Status
| Command | Result |
|---|---|
| `cargo build` (default) | ✅ compiles; fallback bin prints a hint |
| `cargo check` | ✅ passes |
| `cargo build --features desktop` | ✅ compiles the GUI binary (embeds `dist/` UI) |
| `cargo build --features desktop,media` | ✅ compiles with libmpv in-app playback (needs system `libmpv`; see `.cargo/config.toml`) |
| `cargo test --lib` | ✅ 20 tests pass |
| `cargo run --features desktop` | launches the native window (functional UI) |

The `desktop` binary embeds the `dist/` frontend at build time. GUI runtime requires a desktop session (cannot be exercised in a headless CI here), but the build, JS syntax, and all backend commands are verified by tests.

## 5. Known Gaps / Limitations
- **EPG**: `refresh_epg` exists but there's no scheduled/background auto-refresh or progress reporting yet. EPG is keyed by XMLTV `channel` id (matching `tvg-id`); channels without a `tvg-id` won't match EPG data.
- **M3U parser**: `catchup`/`tvg-shift`, `#EXTGRP`, HLS `#EXT-X-STREAM-INF`, BOM, and relative URLs are handled. Still missing: selecting the best HLS variant automatically, UTF-8 encoding detection beyond BOM, and entries lacking any `#EXTINF`.
- **Discovery** has local CRUD only — no cloud fetch / health scoring.
- **Playback**: external-player launch is fully wired. **In-app playback (libmpv)** is implemented behind the `media` feature and requires the system `libmpv` library at link time (`.cargo/config.toml` sets the macOS Homebrew path). When built without `media`, the UI gracefully hides the in-app buttons.
- **Xtream**: live TV categories/streams are supported (incl. auto EPG), but **VOD and Series** are not yet fetched/stored.
- **Plex**: Movies + TV Shows browse/play (direct-play) are supported. Not yet covered: **transcoding** (Plex direct-play URL only), music libraries, and "manage" operations (mark watched, refresh metadata, delete). MAG playlists are still unsupported.
- **Android**: explicitly out of scope (desktop-only app).
- **UI**: functional but minimal (vanilla JS, no framework, no video embedding inside the webview — libmpv opens its own native window; no settings, no catchup-playback UI). Intended as a usable baseline, not final design.

## 6. Next Steps (prioritized)

### Sprint A — Core ingestion & EPG (complete)
- ✅ EPG XMLTV streaming parser + store + schedule query.
- ✅ M3U parser `#EXTGRP`, `catchup*`/`tvg-shift`, HLS `#EXT-X-STREAM-INF`, BOM, relative-URL resolution.
- ✅ Bookmarks/History/Search storage + Tauri commands.
- ✅ Streaming download (reqwest byte stream → `AsyncRead` → `parse_stream`).
- ✅ EPG-to-list wiring (`refresh_epg` + `ListManager::set_epg_url`).
- ✅ Functional UI (`dist/index.html`) with add/play/search/bookmark/history/EPG.
- ✅ 10 unit tests; `cargo build --features desktop` green; UI JS syntax-checked.

### Sprint B — Phase 2 parity
5. **Xtream** (`player_api.php`) + **MAG** (portal) playlist support.
6. **Discovery** cloud fetch + health scoring.
7. **Search** across channels/lists.
8. External-player auto-detect + config persistence.

### Sprint C — Phase 3 (Playback)
9. **libmpv** integration (`media` feature), hardware accel, subtitles, audio tracks, PiP/miniplayer.
10. **Chromecast** (optional desktop feature — cast from a local HTTP server).

### Later phases (from original plan)
11. **Phase 4**: native **Iced** UI replacing the Tauri webview.
12. **Phase 5**: `napi` Node module + headless CLI.
13. **Phase 6**: benchmarking, memory profiling, beta.

## 7. Open Questions / Risks
- **Cross-compilation / packaging**: release builds for Windows & Linux from a single host need per-OS toolchains (NSIS/AppImage/DEB/RPM etc.).
- **Frontend**: a working vanilla-JS UI exists in `dist/index.html` (embedded at build). A future Svelte/React scaffold or the Iced native UI (Phase 4) can replace it; `tauri.conf.json` `devUrl`/`frontendDist` point at `dist`.
- **Directory layout**: lib at root `src/`, Tauri entry at `src-tauri/` — functional but unconventional; consider consolidating later.
- **Chromecast** likely needs a hybrid (webview + JS SDK).
- **M3U edge cases**: must still match all variations the JS parser handled.

## 8. How to Build
```sh
rustup update stable          # needs ≥ 1.88
cargo build                   # lib + fallback bin
cargo build --features desktop  # GUI (requires dist/ placeholder, present)
cargo test --lib              # unit tests
cargo run --features desktop  # launch window
# media feature (playback) additionally requires system libmpv:
cargo build --features desktop,media
```
