# AGENTS.md — copiador

## Project

Rust desktop GUI (eframe/egui) that copies channel frame files from a network source to local destinations.  
Single crate, no workspace.

## Build & Run

```powershell
cargo run
```

No lint, format, or typecheck config exists.

## Tests

```powershell
cargo test
```

Tests live in `src/db.rs` (inline `#[cfg(test)]` module). They create/clean up a `test_copiador.db` file in the working directory.  
Run from repo root so the temp DB lands alongside `copiador.db`.

## Key Architecture

| Module | Role |
|---|---|
| `main.rs` | GUI app with 4 tabs (Dashboard, Matrix, Settings, Channels) |
| `config.rs` | `AppConfig` struct, JSON serialize/deserialize |
| `db.rs` | SQLite via rusqlite, WAL mode, schema init on connect |
| `worker.rs` | Background sync thread, communicates via mpsc channels |

- DB connection opened fresh per operation (each method opens its own `DatabaseManager`).
- Worker runs a 20s sync cycle; UI has a "Sync Now" button that sends `WorkerCommand::TriggerSync`.
- Dates stored as `YYMMDD` strings (lexicographically sortable).
- Status colors: Verde (3600 files), Azul (incomplete), Naranja (copying), Rojo (error).
- Origin is read-only; destination folders can be purged reactively based on `dias_persistencia`.
- OS Error 32 (file locked) triggers a 30 min cooldown per channel.
- ZIP/RAR presence delays retry by 5 min.
- Stability check waits 5 min between two file counts before copying.
- Max 2 retry attempts per hour slot, then skipped.
- TAR packing runs after successful copy if `habilitar_tar` is on.

## DB Schema

- `CONFIGURACION` — single-row config (id=1)
- `CANAL` — channel codes
- `FECHA_SINCRONIZACION` — date YYMMDD
- `REGISTRO_HORA` — per-channel per-hour status (cascading deletes)
- `LOG_SISTEMA` — audit log

## Edition

`edition = "2024"` in Cargo.toml — requires Rust 1.85+.
