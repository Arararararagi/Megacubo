# Megacubo → Rust Migration: Status & Next Steps

> Companion to `RUST_REWRITE_PLAN.md` (the original technical proposal). This document tracks what is actually built, the decisions made, and the remaining work.

## 1. Goal
Rewrite the Electron/JS-based Megacubo IPTV player in Rust, using **Tauri v2** as the desktop shell, **Tokio** for async, **SQLite** (via `sqlx`) for storage, and **libmpv** for playback. **Desktop-only** — targets Windows, macOS, and Linux. Android is dropped. Phased rollout in §6.

## 2. Current Architecture
- **Package**: `megacubo-rs` (root `Cargo.toml`), edition 2021. Produces a `lib` (+ `cdylib`) crate `megacubo` and a `bin` `megacubo`.
- **Library source**: `src/` → `config`, `db`, `epg`, `lists`, `parser`, `streamer` (`src/lib.rs` re-exports the public API).
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
| EPG (planned) | `quick-xml` 0.31 (currently unused) |
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

### 3.4 EPG, storage & parser hardening (Sprint A progress)
- **EPG XMLTV parser**: added `epg::parse_xmltv` — a streaming `quick-xml` reader producing `XmltvChannel`/`XmltvProgramme` (was the only unused dependency). `EpgManager::parse_and_store` bulk-inserts channels + programmes in a transaction (clears the previous `epg_url` first). Previously only `parse_xmltv_time` + ad-hoc programme storage existed.
- **M3U parser hardening**: added `#EXTGRP:` group support (preserved across the following `#EXTINF` unless overridden by `group-title`); display name after the comma; non-`http` URLs already accepted.
- **Bookmarks / History storage**: `Database` gained `add_bookmark` / `get_bookmarks` / `add_history` / `get_history`, plus `search_channels` (LIKE on name/group).
- **Streaming download**: `M3uParser::parse_stream` parses an `AsyncRead` incrementally (used by `add_m3u_list` via a `StreamReader` over the reqwest byte stream) — avoids loading the whole playlist into memory.
- **Tauri commands wired**: `add_m3u_list` (streaming), `get_lists`, `get_channels`, `search_channels`, `add_bookmark`/`get_bookmarks`, `add_history`/`get_history`, `refresh_epg` (fetch + `EpgManager::parse_and_store`, sets list EPG URL), `get_epg_programme`, `launch_external_player`. `ListManager` gained `set_epg_url` / `get_by_url`.

## 4. Build & Run Status
| Command | Result |
|---|---|
| `cargo build` (default) | ✅ compiles; fallback bin prints a hint |
| `cargo check` | ✅ passes |
| `cargo build --features desktop` | ✅ compiles the GUI binary |
| `cargo test --lib` | ✅ 7 tests pass |
| `cargo run --features desktop` | launches the native window (placeholder page) |

**Constraint**: the `desktop` binary needs the `dist/` frontend to exist at build time (placeholder provided). Real UI is a later phase.

## 5. Known Gaps / Limitations
- **EPG**: `refresh_epg` exists but there's no scheduled/background auto-refresh or progress reporting yet.
- **M3U parser**: `#EXTGRP` done; still missing `catchup*`/`tvg-shift` attributes, HLS `#EXT-X-STREAM-INF` nested playlists, UTF-8 BOM/encoding handling, relative-URL resolution, entries without `#EXTINF`.
- **No Xtream / MAG** support (only M3U type in `ListManager`).
- **Discovery** has local CRUD only — no cloud fetch / health scoring.
- **Playback**: only URL probing + external-player launch; **no libmpv** integration (`media` feature unbuilt).
- **Android**: explicitly out of scope (desktop-only app).
- **UI**: placeholder page only; no Svelte/frontend wired (`devUrl`/`frontendDist` point at a stub).
- `libmpv` is gated behind `media` and would require the system `libmpv` C library to compile.

## 6. Next Steps (prioritized)

### Sprint A — Core ingestion & EPG (in progress)
- ✅ **EPG XMLTV streaming parser** with `quick-xml`, bulk-insert into `EpgManager`.
- ✅ **M3U parser `#EXTGRP`** + display-name/non-`http` handling.
- ✅ **Bookmarks/History** storage + Tauri commands.
- ✅ **Streaming download** (reqwest byte stream → `AsyncRead` → `parse_stream`).
- ✅ **EPG-to-list wiring** (`refresh_epg` command + `ListManager::set_epg_url`).
- ✅ **Search** (`search_channels` + Tauri command).
1. Finish **M3U edge cases** (`catchup*`, `#EXT-X-STREAM-INF`, BOM, relative URLs, entries without `#EXTINF`).

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
- **Frontend**: no Svelte scaffold yet; `tauri.conf.json` `devUrl`/`frontendDist` are stubs.
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
