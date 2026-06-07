mod config;
mod db;
mod worker;

use db::DatabaseManager;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use tauri::Emitter;
use worker::{SyncWorker, UiMessage, WorkerCommand};

// --- Tauri managed state ---
struct AppState {
    tx: Mutex<Sender<WorkerCommand>>,
    db: Mutex<DatabaseManager>,
}

// --- DTOs for commands ---
#[derive(Serialize)]
struct MatrixEntry {
    canal: String,
    hora: u8,
    estado: String,
    cantidad: u32,
}

// --- Tauri commands ---

#[tauri::command]
fn get_config(state: tauri::State<AppState>) -> Result<config::AppConfig, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_config().map_err(|e| e.to_string())
}

#[tauri::command]
fn save_config(state: tauri::State<AppState>, config: String) -> Result<(), String> {
    let new_config: config::AppConfig = serde_json::from_str(&config).map_err(|e| e.to_string())?;
    let db = state.db.lock().map_err(|e| e.to_string())?;

    // Detectar cambio de rutas de destino → resetear estado de sincronización
    if let Ok(current_config) = db.get_config() {
        let dest_changed = current_config.ruta_copia_normal != new_config.ruta_copia_normal
            || current_config.ruta_copia_tar != new_config.ruta_copia_tar;
        if dest_changed {
            let _ = db.reset_destino_sync();
            let _ = db.insert_log(
                "WARN", "DEST_CHANGED",
                "Ruta de destino modificada. Re-sincronizando horas completadas anteriormente.",
                None, None,
            );
        }
    }

    db.save_config(&new_config).map_err(|e| e.to_string())?;
    let _ = db.insert_log("INFO", "CONFIG_SAVE", "Configuracion guardada.", None, None);
    let tx = state.tx.lock().map_err(|e| e.to_string())?;
    let _ = tx.send(WorkerCommand::ReloadConfig);
    Ok(())
}

#[tauri::command]
fn get_logs(state: tauri::State<AppState>, limit: u32) -> Result<Vec<db::LogEntry>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_recent_logs(limit).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_dates(state: tauri::State<AppState>) -> Result<Vec<String>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_all_dates().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_matrix(state: tauri::State<AppState>, date: String) -> Result<Vec<MatrixEntry>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let rows = db.get_registro_horas_por_fecha(&date).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(canal, hora, estado, cantidad)| MatrixEntry {
            canal,
            hora,
            estado,
            cantidad,
        })
        .collect())
}

#[tauri::command]
fn add_canal(state: tauri::State<AppState>, code: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut config = db.get_config().map_err(|e| e.to_string())?;
    if config.canales.contains(&code) {
        return Ok(());
    }
    config.canales.push(code);
    db.save_config(&config).map_err(|e| e.to_string())?;
    let tx = state.tx.lock().map_err(|e| e.to_string())?;
    let _ = tx.send(WorkerCommand::ReloadConfig);
    Ok(())
}

#[tauri::command]
fn remove_canal(state: tauri::State<AppState>, code: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut config = db.get_config().map_err(|e| e.to_string())?;
    config.canales.retain(|c| c != &code);
    db.save_config(&config).map_err(|e| e.to_string())?;
    let tx = state.tx.lock().map_err(|e| e.to_string())?;
    let _ = tx.send(WorkerCommand::ReloadConfig);
    Ok(())
}

#[tauri::command]
fn trigger_sync(state: tauri::State<AppState>) -> Result<(), String> {
    let tx = state.tx.lock().map_err(|e| e.to_string())?;
    tx.send(WorkerCommand::TriggerSync).map_err(|e| e.to_string())
}

#[tauri::command]
fn write_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, &content).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

// --- Main ---
fn main() {
    let db_path = PathBuf::from("copiador.db");
    let (worker_tx, worker_rx) = channel();
    let (ui_tx, ui_rx) = channel();

    // Spawn background worker
    let worker_db_path = db_path.clone();
    std::thread::spawn(move || {
        let mut worker = SyncWorker::new(worker_db_path, worker_rx, ui_tx);
        worker.run_loop();
    });

    // Conexión singleton para comandos Tauri
    let db = DatabaseManager::new(&db_path)
        .expect("Fallo al inicializar la base de datos");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            tx: Mutex::new(worker_tx),
            db: Mutex::new(db),
        })
        .setup(|app| {
            let handle = app.handle().clone();
            // Forward UI messages from worker to Tauri events
            std::thread::spawn(move || {
                forward_ui_messages(ui_rx, handle);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            get_logs,
            get_dates,
            get_matrix,
            add_canal,
            remove_canal,
            trigger_sync,
            write_file,
            read_file,
        ])
        .run(tauri::generate_context!())
        .expect("error al ejecutar Heimdall Copiador");
}

fn forward_ui_messages(rx: Receiver<UiMessage>, handle: tauri::AppHandle) {
    while let Ok(msg) = rx.recv() {
        match msg {
            UiMessage::SyncStarted => {
                let _ = handle.emit("sync-started", ());
            }
            UiMessage::SyncFinished => {
                let _ = handle.emit("sync-finished", ());
            }
            UiMessage::StatusUpdated {
                canal,
                fecha,
                hora,
                status,
                count,
            } => {
                let _ = handle.emit(
                    "status-updated",
                    serde_json::json!({
                        "canal": canal,
                        "fecha": fecha,
                        "hora": hora,
                        "status": status,
                        "count": count,
                    }),
                );
            }
            UiMessage::LogAdded => {
                let _ = handle.emit("log-added", ());
            }
        }
    }
}
