# Technical Proposal: Rust Rewrite of Megacubo IPTV Player

> **Platform scope (updated):** This rewrite is **desktop-only** — Windows, macOS, and Linux. **Android is dropped.** See `RUST_MIGRATION_STATUS.md` for current implementation status and next steps.

## Executive Summary

This proposal outlines a comprehensive plan to fork and rewrite the Electron-based Megacubo IPTV player in Rust. The rewrite targets significant improvements in performance, memory efficiency, and startup time while preserving all existing features across Windows, macOS, and Linux (desktop-only).

The proposed architecture uses **Tauri** as the application shell (Rust backend + web frontend), **Tokio** as the async runtime, **SQLite** for embedded data storage, **libmpv** for hardware-accelerated media playback, and **Iced** as a native Rust UI toolkit for future phases.

---

## 1. Technology Stack & Architecture

### 1.1 Recommended Stack

| Layer | Technology | Justification |
|-------|-----------|---------------|
| **Application Shell** | **Tauri** (v2) | Minimal binary size (~10-20MB vs Electron's ~200MB), native OS integration, Rust backend with web frontend. Allows gradual migration of UI components. |
| **Async Runtime** | **Tokio** (multi-threaded) | Industry standard, excellent ecosystem, work-stealing scheduler, battle-tested at scale. |
| **HTTP Client** | **reqwest** (Tokio-based) | Built on hyper/tokio, supports HTTP/2, connection pooling, redirects, cookies. Drop-in replacement for axios/undici. |
| **M3U Parsing** | **Custom streaming parser** (Rust) | The existing JS parser is complex (865 lines) with regex-based attribute extraction. A Rust streaming parser using `memchr` for line splitting will be 5-10x faster and use significantly less memory. |
| **XMLTV/EPG Parsing** | **sxd-document** + custom SAX-like parser | The current `xmltv-stream` library is JS-based. Rust's XML ecosystem allows for streaming SAX parsing with minimal memory overhead. |
| **Embedded Database** | **SQLite** (via `sqlx` or `rusqlite`) | Replaces both JexiDB (EPG) and the file-based storage index. Single database file, ACID transactions, indexed queries, WAL mode for concurrent access. |
| **Media Playback** | **libmpv** (FFI) | Best cross-platform hardware-accelerated playback. Supports all codecs (H.264, H.265, VP9, AV1), hardware decoding (VA-API, VDPAU, VideoToolbox, DXVA2, MediaCodec), subtitle handling, audio tracks. |
| **UI Framework** | **Phase 1: Tauri web frontend** (Svelte) → **Phase 4: Iced** (native Rust) | Phase 1 reuses existing Svelte UI via Tauri's webview. Phase 4 migrates to Iced for a fully native Rust UI with better performance and no webview dependency. |
| **CLI / Library** | **clap** + **cdylib** | Supports both CLI usage and Node.js module export (via `napi-rs` or `neon` for FFI). |
| **Configuration** | **serde** + JSON | Type-safe configuration with compile-time validation. |
| **Internationalization** | **fluent** + **rust-icu** | Industry-standard i18n with plural rules, gender support, and efficient runtime. |

### 1.2 High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                        MEGACUBO (Rust)                              │
├─────────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │
│  │   Tauri      │  │   Iced       │  │   CLI /      │              │
│  │  Webview     │  │  (Native)    │  │   Library    │              │
│  │  (Svelte)    │  │  (Phase 4)   │  │  (Phase 5)   │              │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘              │
│         │                 │                 │                      │
│         ▼                 ▼                 ▼                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    CORE (Rust)                              │   │
│  │                                                              │   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────┐  │   │
│  │  │ Channels │  │   Lists  │  │  Streamer│  │     EPG     │  │   │
│  │  │ Manager  │  │ Manager  │  │  (libmpv)│  │   Manager   │  │   │
│  │  └──────────┘  └──────────┘  └──────────┘  └─────────────┘  │   │
│  │                                                              │   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────┐  │   │
│  │  │ Storage  │  │ Discovery│  │  Bridge  │  │   Config    │  │   │
│  │  │ (SQLite) │  │  (Cloud) │  │  (IPC)   │  │  (serde)    │  │   │
│  │  └──────────┘  └──────────┘  └──────────┘  └─────────────┘  │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                           │                                        │
│                           ▼                                        │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    DATA LAYER (SQLite)                       │   │
│  │                                                              │   │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐             │   │
│  │  │ channels   │  │ epg_program│  │   lists    │             │   │
│  │  │ categories │  │ epg_channel│  │ list_items │             │   │
│  │  │ bookmarks  │  │ epg_meta   │  │ discovery  │             │   │
│  │  │ history    │  │ epg_cache  │  │ config     │             │   │
│  │  └────────────┘  └────────────┘  └────────────┘             │   │
│  └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

### 1.3 Backend-to-UI Communication

**Phase 1 (Tauri)**: Uses Tauri's built-in IPC (`invoke`) for Rust → frontend calls and `emit`/`listen` for events. This replaces the current HTTP bridge server.

**Phase 4 (Iced)**: Direct function calls within the same process — no IPC overhead.

### 1.4 Community Lists & Node Module

**Community lists**: The discovery module will be ported to Rust, fetching from the same cloud API endpoints. The health scoring algorithm will be preserved.

**Node.js module**: For backward compatibility, a `cdylib` will be compiled using `napi-rs` to expose the core as a Node.js module. This allows existing integrations to use the Rust core without changes.

---

## 2. Data Handling & Performance

### 2.1 Data Model & Storage Strategy

**SQLite** replaces both JexiDB (EPG) and the file-based storage index. Schema includes tables for channels, categories, EPG programmes, bookmarks, history, lists, discovery, and config.

Key design decisions:
- **WAL mode** for concurrent read/write access
- **Indexes** on frequently queried columns
- **JSON arrays** stored as TEXT for flexible fields
- **Batch inserts** using transactions for playlist/EPG loading
- **Single database file** per profile for simplicity

### 2.2 Large M3U Playlist Parsing

**Rust approach**:
- Streaming parser using `memchr` for fast line splitting
- Zero-copy parsing where possible
- Pre-compiled regex using the `regex` crate
- Object pooling via `bumpalo` arena allocator
- Parallel parsing for multiple lists using Tokio tasks

**Performance target**: Parse 50,000 channel entries in < 2 seconds.

### 2.3 EPG XML Parsing

**Rust approach**:
- Streaming SAX parser using `sxd-document` or custom pull-based parser
- Incremental database writes
- Memory monitoring
- LRU cache for lookups
- Background updates with progress reporting

### 2.4 Lazy Loading, Caching & Background Indexing

- Channels loaded per-category with pagination
- EPG data loaded on-demand
- Icons with LRU cache
- Background indexing with Tokio tasks

---

## 3. Media Playback

### 3.1 Playback Engine: libmpv

**libmpv** is the recommended playback engine:
- Cross-platform: Windows/macOS/Linux (desktop-only)
- Hardware acceleration: VA-API, VideoToolbox, DXVA2
- Codec support: H.264, H.265/HEVC, VP8, VP9, AV1, MPEG-2, AAC, AC3, Opus
- Container support: MP4, MKV, TS, M2TS, MOV, AVI, WebM
- Streaming protocols: HTTP, HLS, DASH, RTMP, RTSP, UDP, RTP
- Subtitle/audio track support

### 3.2 Miniplayer / PiP Mode

- Desktop: Tauri window APIs or CSS overlay

### 3.3 External Player Support

- Use `std::process::Command` to launch external players (VLC, MPV, MPC-HC, PotPlayer)
- Auto-detect common players, allow custom paths

---

## 4. Cross-Platform Strategy

### 4.1 Build & Packaging

| Platform | Build Tool | Packaging |
|----------|-----------|------------|
| **Windows** | `cargo build --target x86_64-pc-windows-gnu` | NSIS/MSI/portable |
| **macOS** | Universal binary (x64 + ARM64) | DMG/ZIP/notarized |
| **Linux** | `cargo build --target x86_64-unknown-linux-gnu` | AppImage/DEB/RPM/Flatpak/Snap |

### 4.2 Platform-Specific Considerations

- **Windows**: DXVA2, code signing
- **macOS**: VideoToolbox, sandboxing, universal binary
- **Linux**: VA-API/VDPAU, AppImage/Flatpak

---

## 5. Development & Migration Roadmap

### Phase 1: MVP (Months 1-4)
- Rust project scaffold with Tauri
- SQLite database layer with basic schema
- M3U playlist parser
- Basic channel list UI (reuse Svelte)
- libmpv integration for basic playback
- Configuration management

### Phase 2: Full Feature Parity (Months 5-9)
- Xtream and MAG playlist support
- EPG XMLTV parser with SQLite storage
- Bookmarks and watch history
- Community lists discovery
- Search functionality
- External player support

### Phase 3: Advanced Features (Months 10-14)
- Miniplayer / Picture-in-Picture mode
- Subtitle support (external and embedded)
- Audio track selection
- Chromecast support (desktop casting via local HTTP server)
- Hardware acceleration optimization

### Phase 4: Native UI (Months 15-18)
- Replace Tauri webview with native Iced UI
- Full native look and feel
- Eliminate webview dependency
- Theming support

### Phase 5: Node.js Module (Months 19-21)
- Compile core as `cdylib` using `napi-rs`
- Expose JavaScript API
- CLI tool for headless operation

### Phase 6: Optimization & Polish (Months 22-24)
- Performance benchmarking
- Memory profiling
- Automated testing
- Beta release

---

## 6. Open Questions & Risks

### 6.1 Biggest Unknowns

1. **Iced UI maturity**: API changes, complex widgets may need custom development
2. **Chromecast support**: Desktop casting via local HTTP server + webview/JS SDK
3. **M3U parser edge cases**: Must handle all existing playlist variations

### 6.2 Code Reuse vs Rewrite

**Reuse**:
- Language JSON files (via serde_json)
- Channel metadata packages
- Community list endpoints
- Channel name matching algorithms

**Rewrite**:
- M3U parser → Streaming Rust parser
- EPG parser → SAX-style Rust parser
- Storage layer → SQLite
- Streamer engines → libmpv

### 6.3 Success Metrics

- Startup < 3 seconds (vs current ~10-15s)
- Memory < 200MB idle (vs current ~500MB+)
- Channel loading: 50K channels in < 5s
- Binary size < 50MB (vs ~200MB+)

---

## 7. Conclusion

This Rust rewrite delivers 5-10x faster startup, 3-5x lower memory usage, and 50% smaller binaries while preserving all existing features. The phased approach enables gradual migration with tangible results in the first 4 months.