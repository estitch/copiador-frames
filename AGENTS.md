# AGENTS.md — copiador

## Project

Rust/Tauri desktop app that copies channel frame files from a network source to local destinations.  
Single crate, no workspace. Package name `heimdall-copiador`.

## Build, lint, test

```powershell
npm run tauri:dev     # Vite hot-reload (port 1420) + Tauri window
npm run tauri:build   # Production build
npm run lint          # cargo clippy -- -D warnings
npm run fmt           # cargo fmt --check
npm run fmt:fix       # cargo fmt
cargo test            # run all (inline tests only)
```

Tests live in `src/db.rs` (`#[cfg(test)]`). They create/clean `test_copiador.db` — run from repo root.

**CI** runs on `windows-latest` only (GitHub Actions). No cross-platform checks.

**Build environment**: `.cargo/config.toml` hardcodes MSVC 2022 Build Tools paths — builds require Visual Studio Build Tools.

## Key modules

| Module | Role |
|---|---|
| `main.rs` | Tauri entrypoint, 10 commands, event forwarding |
| `lib.rs` | Re-exports config, db, worker |
| `config.rs` | `AppConfig` — serialized as JSON for IPC |
| `db.rs` | SQLite via rusqlite (WAL, FK, 5s busy timeout). Fresh connection per op. |
| `worker.rs` | Background sync (20s cycle); uses rayon pool + mpsc to UI |
| `frontend/` | Vanilla JS + Vite (root set to `frontend/` in vite.config.js) |
| `src-tauri/` | Tauri build script, icons, capabilities |

## Architecture notes

- **IPC**: JSON-serialized `AppConfig` crosses the invoke boundary. Config fields stored openly in DB — no secrets.
- **Tauri capabilities**: `src-tauri/capabilities/default.json` — `core:default`, `dialog:default`, `dialog:allow-open`, `dialog:allow-save`. An identical copy lives at `capabilities/default.json` — edit both.
- **Events** (frontend listens): `sync-started`, `sync-finished`, `status-updated`, `file-progress`, `log-added`, `config-changed`.
- **Worker sync cycle**: 20s loop. `TriggerSync` forces immediate scan. Tasks are filesystem-discovered (`YYMMDD/canal/HH`), filtered by DB state, copied via `CopyFileExW` (Windows) with `COPY_FILE_OPEN_SOURCE`.
- **Retry rules**: max 2 attempts per hour slot, then skipped until reset. OS Error 32 → 30min cooldown per channel. ZIP/RAR → 5min skip. Stability check waits 5min between file counts.
- **Destination changed** (`save_config` detects diff in normal/tar paths) → resets all "Verde" to "Azul" to force re-copy.
- **Purge**: reactive, based on `dias_persistencia`. Deletes old date dirs from destination + DB records.
- **TAR packing**: if `habilitar_tar`, dispatched to dedicated `TarWorker` threads after successful copy.
- **Bundle**: `tauri.conf.json` has `"active": false` — no bundling configured. `.taurignore` excludes `*.db*` files from bundle.

## DB schema

- `CONFIGURACION` (id=1, single row) — paths, thresholds, flags
- `CANAL` — channel codes
- `FECHA_SINCRONIZACION` — YYMMDD dates
- `REGISTRO_HORA` — per-channel per-hour status, `intentos`, `ultimo_intento` (cascading deletes)
- `LOG_SISTEMA` — audit log (paginated, 50 per page frontend default)

## Edition

`edition = "2024"` in both `Cargo.toml` and `rustfmt.toml` — requires Rust 1.85+.
