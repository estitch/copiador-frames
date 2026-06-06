use crate::config::AppConfig;
use crate::db::{DatabaseManager, LogBuffer};
use chrono::Timelike;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[allow(dead_code)]
pub enum WorkerCommand {
    TriggerSync,
    ReloadConfig,
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum UiMessage {
    SyncStarted,
    SyncFinished,
    StatusUpdated {
        canal: String,
        fecha: String,
        hora: u8,
        status: String,
        count: u32,
    },
    LogAdded,
}

enum TarMessage {
    AddFile {
        dest_path: PathBuf,
        tar_path: PathBuf,
    },
    HourComplete {
        canal: String,
        fecha: String,
        hora: u8,
        tar_path: PathBuf,
    },
}

pub struct WorkerState {
    pub cooldowns: HashMap<String, Instant>,
    pub last_checks: HashMap<(String, String, u8), (u32, Instant)>,
    pub zip_cooldowns: HashMap<(String, String, u8), Instant>,
}

impl WorkerState {
    pub fn new() -> Self {
        Self {
            cooldowns: HashMap::new(),
            last_checks: HashMap::new(),
            zip_cooldowns: HashMap::new(),
        }
    }
}

pub struct SyncWorker {
    db_path: PathBuf,
    rx: Receiver<WorkerCommand>,
    tx: Sender<UiMessage>,
    state: Arc<Mutex<WorkerState>>,
    tar_senders: Vec<Sender<TarMessage>>,
    tar_handles: Vec<std::thread::JoinHandle<()>>,
}

impl SyncWorker {
    pub fn new(db_path: PathBuf, rx: Receiver<WorkerCommand>, tx: Sender<UiMessage>) -> Self {
        let mut tar_senders = Vec::with_capacity(TAR_WORKER_COUNT);
        let mut tar_handles = Vec::with_capacity(TAR_WORKER_COUNT);

        for _ in 0..TAR_WORKER_COUNT {
            let (tx_inner, rx_inner) = std::sync::mpsc::channel();
            let tar_db_path = db_path.clone();
            let tar_ui_tx = tx.clone();
            let handle = std::thread::spawn(move || {
                TarWorker::run_loop(rx_inner, tar_db_path, tar_ui_tx);
            });
            tar_senders.push(tx_inner);
            tar_handles.push(handle);
        }

        Self {
            db_path,
            rx,
            tx,
            state: Arc::new(Mutex::new(WorkerState::new())),
            tar_senders,
            tar_handles,
        }
    }

    pub fn run_loop(&mut self) {
        let db = match DatabaseManager::new(&self.db_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Error al inicializar DatabaseManager en el worker: {:?}", e);
                return;
            }
        };

        let mut last_sync_time = Instant::now() - Duration::from_secs(300);
        let cycle_interval = Duration::from_secs(20);

        let _ = db.insert_log("INFO", "WORKER_START", "Motor de sincronización backend iniciado.", None, None);
        let _ = self.tx.send(UiMessage::LogAdded);

        loop {
            let mut trigger_now = false;
            let mut shutdown = false;
            while let Ok(cmd) = self.rx.try_recv() {
                match cmd {
                    WorkerCommand::TriggerSync => {
                        trigger_now = true;
                    }
                    WorkerCommand::ReloadConfig => {
                        let _ = db.insert_log("INFO", "CONFIG_RELOAD", "Configuración recargada en el backend.", None, None);
                        let _ = self.tx.send(UiMessage::LogAdded);
                    }
                    WorkerCommand::Shutdown => {
                        shutdown = true;
                    }
                }
            }

            if shutdown {
                self.tar_senders.clear();
                for handle in self.tar_handles.drain(..) {
                    let _ = handle.join();
                }
                let _ = db.insert_log("INFO", "WORKER_SHUTDOWN", "Motor de sincronización backend apagado.", None, None);
                let _ = self.tx.send(UiMessage::LogAdded);
                break;
            }

            let elapsed = last_sync_time.elapsed();
            if trigger_now || elapsed >= cycle_interval {
                let _ = self.tx.send(UiMessage::SyncStarted);
                if let Err(e) = self.execute_sync_cycle() {
                    let _ = db.insert_log("ERROR", "SYNC_CYCLE_FAIL", &format!("Fallo en ciclo de sincronización: {:?}", e), None, None);
                    let _ = self.tx.send(UiMessage::LogAdded);
                }
                let _ = self.tx.send(UiMessage::SyncFinished);
                last_sync_time = Instant::now();
            }

            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn execute_sync_cycle(&self) -> Result<(), rusqlite::Error> {
        let db = DatabaseManager::new(&self.db_path)?;
        let config = db.get_config()?;
        if config.ruta_origen.as_os_str().is_empty() || config.ruta_copia_normal.as_os_str().is_empty() {
            return Ok(());
        }

        let local_now = chrono::Local::now();
        let source_now = local_now + chrono::Duration::hours(config.horas_desface as i64);
        let source_date = source_now.date_naive();

        let discovered_tasks = self.discover_tasks(&config, &db, source_date);

        let mut by_channel: HashMap<String, Vec<(String, u8)>> = HashMap::new();
        for (fecha, canal, hora) in discovered_tasks {
            by_channel.entry(canal).or_default().push((fecha, hora));
        }

        if by_channel.is_empty() {
            return Ok(());
        }

        let channel_tasks: Vec<(String, Vec<(String, u8)>)> = by_channel.into_iter().collect();
        let max_threads = (config.max_hilos_copia.max(1) as usize).min(channel_tasks.len());

        let pool = match rayon::ThreadPoolBuilder::new()
            .num_threads(max_threads)
            .build()
        {
            Ok(p) => p,
            Err(e) => {
                let _ = db.insert_log("ERROR", "POOL_FAIL", &format!("Error creando pool de hilos: {}", e), None, None);
                rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap()
            }
        };

        let db_path = self.db_path.clone();
        let ui_tx = &self.tx;
        let tar_senders = &self.tar_senders;
        let state = &self.state;

        pool.scope(|s| {
            for (canal, tasks) in &channel_tasks {
                s.spawn(|_| {
                    Self::process_channel(canal, tasks, &config, &source_now, &db_path, ui_tx, tar_senders, state);
                });
            }
        });

        self.reactive_purge(&config, source_date, &db);
        Ok(())
    }

    fn process_channel(
        canal: &str,
        tasks: &[(String, u8)],
        config: &AppConfig,
        source_now: &chrono::DateTime<chrono::Local>,
        db_path: &Path,
        ui_tx: &Sender<UiMessage>,
        tar_senders: &[Sender<TarMessage>],
        state: &Mutex<WorkerState>,
    ) {
        let mut ch_db = match DatabaseManager::new(db_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Error DB para canal {}: {:?}", canal, e);
                return;
            }
        };
        let mut log_buffer = LogBuffer::new();

        for (fecha, hora) in tasks {
            let in_os32_cooldown = {
                let gs = state.lock().unwrap();
                gs.cooldowns.get(canal).map(|t| t.elapsed() < Duration::from_secs(30 * 60)).unwrap_or(false)
            };
            if in_os32_cooldown {
                continue;
            } else {
                let mut gs = state.lock().unwrap();
                gs.cooldowns.remove(canal);
            }

            let task_key = (fecha.clone(), canal.to_string(), *hora);

            let Some(task_time) = Self::parse_task_time(fecha, *hora) else {
                continue;
            };

            let diff = source_now.naive_local().signed_duration_since(task_time);
            if diff.num_seconds() < 3600 {
                continue;
            }

            if diff.num_seconds() < 7200 {
                let minute = source_now.minute();
                if config.margen_post_minutos > 0 && minute < config.margen_post_minutos {
                    continue;
                }
                if config.margen_pre_minutos > 0 && minute >= (60 - config.margen_pre_minutos) {
                    continue;
                }
            }

            let mut is_incomplete = false;
            let mut last_attempt_time = None;
            if let Ok(Some((intentos, ultimo_intento_str))) = ch_db.get_hora_intentos(canal, fecha, *hora) {
                if intentos >= 2 {
                    continue;
                }
                is_incomplete = true;
                if let Ok(last_time) = chrono::NaiveDateTime::parse_from_str(&ultimo_intento_str, "%Y-%m-%d %H:%M:%S") {
                    last_attempt_time = Some(last_time);
                }
            }

            if is_incomplete {
                if let Some(last_time) = last_attempt_time {
                    if chrono::Local::now().naive_local().signed_duration_since(last_time) < chrono::Duration::hours(1) {
                        continue;
                    }
                }
            }

            let in_zip_cooldown = {
                let gs = state.lock().unwrap();
                gs.zip_cooldowns.get(&task_key).map(|t| t.elapsed() < Duration::from_secs(5 * 60)).unwrap_or(false)
            };
            if in_zip_cooldown {
                continue;
            } else {
                let mut gs = state.lock().unwrap();
                gs.zip_cooldowns.remove(&task_key);
            }

            let origin_hour_path = config.ruta_origen.join(fecha).join(canal).join(hora.to_string());
            if !origin_hour_path.exists() {
                continue;
            }

            let current_count = Self::count_origin_files_superficial(&origin_hour_path);

            if is_incomplete {
                let local_count = Self::get_copied_file_count(&config.ruta_copia_normal, fecha, canal, *hora);
                if current_count == local_count {
                    let revert_status = if local_count > 0 { "Azul" } else { "Rojo" };
                    let _ = ch_db.update_hora_status(canal, fecha, *hora, revert_status, local_count);
                    continue;
                }
            }

            if Self::has_compressed_files(&origin_hour_path) {
                let _ = ch_db.insert_log(
                    "WARN", "COMPRESSED_EXCLUSION",
                    &format!("Omitiendo canal {} hora {} por archivos .zip/.rar.", canal, hora),
                    Some(canal), None,
                );
                let _ = ui_tx.send(UiMessage::LogAdded);
                {
                    let mut gs = state.lock().unwrap();
                    gs.zip_cooldowns.insert(task_key.clone(), Instant::now());
                    gs.last_checks.remove(&task_key);
                }
                continue;
            }

            if current_count >= 3600 {
                let mut gs = state.lock().unwrap();
                gs.last_checks.remove(&task_key);
            } else if let Some((prev_count, check_time)) = {
                let gs = state.lock().unwrap();
                gs.last_checks.get(&task_key).cloned()
            } {
                if check_time.elapsed() < Duration::from_secs(5 * 60) {
                    continue;
                }
                if current_count != prev_count {
                    let _ = ch_db.insert_log(
                        "WARN", "SIZE_UNSTABLE",
                        &format!("Tamaño inestable canal {} hora {} ({}→{}).", canal, hora, prev_count, current_count),
                        Some(canal), None,
                    );
                    let _ = ui_tx.send(UiMessage::LogAdded);
                    let mut gs = state.lock().unwrap();
                    gs.last_checks.insert(task_key, (current_count, Instant::now()));
                    continue;
                }
                let mut gs = state.lock().unwrap();
                gs.last_checks.remove(&task_key);
            } else {
                let mut gs = state.lock().unwrap();
                gs.last_checks.insert(task_key.clone(), (current_count, Instant::now()));
                let _ = ch_db.insert_log(
                    "INFO", "STABILITY_START",
                    &format!("Verificando estabilidad canal {} hora {} ({} archivos).", canal, hora, current_count),
                    Some(canal), None,
                );
                let _ = ui_tx.send(UiMessage::LogAdded);
                continue;
            }

            let dest_hour_path = config.ruta_copia_normal.join(fecha).join(canal).join(hora.to_string());
            let previous_stats = Self::get_copied_file_count(&config.ruta_copia_normal, fecha, canal, *hora);

            let _ = ch_db.update_hora_status(canal, fecha, *hora, "Naranja", previous_stats);
            let _ = ui_tx.send(UiMessage::StatusUpdated {
                canal: canal.to_string(),
                fecha: fecha.clone(),
                hora: *hora,
                status: "Naranja".to_string(),
                count: previous_stats,
            });

            match Self::execute_atomic_copy(&origin_hour_path, &dest_hour_path) {
                Ok((copied_count, copied_files)) => {
                    let final_status = if copied_count >= 3600 { "Verde" } else { "Azul" };
                    let _ = ch_db.update_hora_status(canal, fecha, *hora, final_status, copied_count);
                    let _ = ui_tx.send(UiMessage::StatusUpdated {
                        canal: canal.to_string(),
                        fecha: fecha.clone(),
                        hora: *hora,
                        status: final_status.to_string(),
                        count: copied_count,
                    });

                    if config.habilitar_tar && !tar_senders.is_empty() {
                        let tar_path = config.ruta_copia_tar.join(fecha).join(canal).join(format!("{}.tar", hora));
                        let idx = select_tar_worker(&tar_path, tar_senders.len());
                        for f in &copied_files {
                            let _ = tar_senders[idx].send(TarMessage::AddFile {
                                dest_path: f.clone(),
                                tar_path: tar_path.clone(),
                            });
                        }
                        let _ = tar_senders[idx].send(TarMessage::HourComplete {
                            canal: canal.to_string(),
                            fecha: fecha.clone(),
                            hora: *hora,
                            tar_path,
                        });
                    }

                    log_buffer.push("INFO", "COPY_SUCCESS",
                        &format!("Sincronizado canal {} hora {}. Archivos: {}.", canal, hora, copied_count),
                        Some(canal), None);
                }
                Err(err) => {
                    let _ = ch_db.increment_hora_intentos(canal, fecha, *hora);
                    let is_os_error_32 = err.raw_os_error() == Some(32);

                    let revert_status = if previous_stats > 0 { "Azul" } else { "Rojo" };
                    let _ = ch_db.update_hora_status(canal, fecha, *hora, revert_status, previous_stats);
                    let _ = ui_tx.send(UiMessage::StatusUpdated {
                        canal: canal.to_string(),
                        fecha: fecha.clone(),
                        hora: *hora,
                        status: revert_status.to_string(),
                        count: previous_stats,
                    });

                    if is_os_error_32 {
                        {
                            let mut gs = state.lock().unwrap();
                            gs.cooldowns.insert(canal.to_string(), Instant::now());
                        }
                        log_buffer.push("ERROR", "LOCK_ERROR_32",
                            &format!("OS Error 32 canal {}. Pausando 30 min.", canal),
                            Some(canal), Some(&format!("{:?}", err)));
                    } else {
                        log_buffer.push("ERROR", "COPY_FAIL",
                            &format!("Error copiando canal {} hora {}: {}", canal, hora, err),
                            Some(canal), Some(&format!("{:?}", err)));
                    }
                }
            }
        }

        let _ = log_buffer.flush(&mut ch_db);
    }

    fn parse_task_time(fecha: &str, hora: u8) -> Option<chrono::NaiveDateTime> {
        if fecha.len() != 6 {
            return None;
        }
        let yy = fecha.get(0..2)?.parse::<i32>().ok()?;
        let mm = fecha.get(2..4)?.parse::<u32>().ok()?;
        let dd = fecha.get(4..6)?.parse::<u32>().ok()?;
        let year = 2000 + yy;
        chrono::NaiveDate::from_ymd_opt(year, mm, dd)?
            .and_hms_opt(hora as u32, 0, 0)
    }

    fn discover_tasks(&self, config: &AppConfig, db: &DatabaseManager, source_date: chrono::NaiveDate) -> Vec<(String, String, u8)> {
        if config.canales.is_empty() {
            return Vec::new();
        }

        let dias = config.dias_persistencia.max(1);
        let limit_date = source_date - chrono::Duration::days(dias as i64 - 1);

        let mut tasks = Vec::new();
        let read_dir = match std::fs::read_dir(&config.ruta_origen) {
            Ok(d) => d,
            Err(_) => return tasks,
        };

        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(fecha_str) = path.file_name().and_then(|s| s.to_str()) {
                    if fecha_str.len() == 6 && fecha_str.chars().all(|c| c.is_ascii_digit()) {
                        let in_window = chrono::NaiveDate::parse_from_str(fecha_str, "%y%m%d")
                            .map(|d| d >= limit_date && d <= source_date)
                            .unwrap_or(false);
                        if !in_window {
                            continue;
                        }

                        if let Ok(chan_dir) = std::fs::read_dir(&path) {
                            for chan_entry in chan_dir.flatten() {
                                let chan_path = chan_entry.path();
                                if chan_path.is_dir() {
                                    if let Some(canal_str) = chan_path.file_name().and_then(|s| s.to_str()) {
                                        if config.canales.contains(&canal_str.to_string()) {
                                            if let Ok(hour_dir) = std::fs::read_dir(&chan_path) {
                                                for hour_entry in hour_dir.flatten() {
                                                    let hour_path = hour_entry.path();
                                                    if hour_path.is_dir() {
                                                        if let Some(hour_str) = hour_path.file_name().and_then(|s| s.to_str()) {
                                                            if let Ok(hora) = hour_str.parse::<u8>() {
                                                                if hora < 24 {
                                                                    tasks.push((
                                                                        fecha_str.to_string(),
                                                                        canal_str.to_string(),
                                                                        hora,
                                                                    ));
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut pending_tasks = Vec::new();
        let mut cache_por_fecha: HashMap<String, Vec<(String, u8, String, u32)>> = HashMap::new();

        for (fecha, canal, hora) in tasks {
            if !cache_por_fecha.contains_key(&fecha) {
                if let Ok(records) = db.get_registro_horas_por_fecha(&fecha) {
                    cache_por_fecha.insert(fecha.clone(), records);
                } else {
                    cache_por_fecha.insert(fecha.clone(), Vec::new());
                }
            }

            let is_completed = cache_por_fecha.get(&fecha).map_or(false, |records| {
                records.iter().any(|(c, h, status, _)| c == &canal && *h == hora && status == "Verde")
            });

            if !is_completed {
                pending_tasks.push((fecha, canal, hora));
            }
        }

        pending_tasks.sort_by(|a, b| {
            let res = a.0.cmp(&b.0);
            if res != std::cmp::Ordering::Equal {
                return res;
            }
            let res = a.1.cmp(&b.1);
            if res != std::cmp::Ordering::Equal {
                return res;
            }
            a.2.cmp(&b.2)
        });

        pending_tasks
    }

    fn has_compressed_files(path: &Path) -> bool {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_file() {
                        let name = entry.file_name();
                        if let Some(name_str) = name.to_str() {
                            let name_lower = name_str.to_lowercase();
                            if name_lower.ends_with(".zip") || name_lower.ends_with(".rar") {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    fn count_origin_files_superficial(path: &Path) -> u32 {
        if let Ok(entries) = std::fs::read_dir(path) {
            entries.flatten()
                .filter(|entry| entry.file_type().map(|ft| ft.is_file()).unwrap_or(false))
                .count() as u32
        } else {
            0
        }
    }

    fn get_copied_file_count(base_path: &Path, fecha: &str, canal: &str, hora: u8) -> u32 {
        let path = base_path.join(fecha).join(canal).join(format!("{}", hora));
        if let Ok(entries) = std::fs::read_dir(path) {
            entries.flatten()
                .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
                .count() as u32
        } else {
            0
        }
    }

    #[cfg(windows)]
    fn copy_file_optimized(src: &Path, dst: &Path) -> std::io::Result<u64> {
        use std::os::windows::ffi::OsStrExt;
        let src_wide: Vec<u16> = src.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst_wide: Vec<u16> = dst.as_os_str().encode_wide().chain(Some(0)).collect();
        const COPY_FILE_OPEN_SOURCE: u32 = 0x00000004;
        let result = unsafe {
            windows_sys::Win32::Storage::FileSystem::CopyFileExW(
                src_wide.as_ptr(),
                dst_wide.as_ptr(),
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                COPY_FILE_OPEN_SOURCE,
            )
        };
        if result == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(src.metadata()?.len())
        }
    }

    #[cfg(not(windows))]
    fn copy_file_optimized(src: &Path, dst: &Path) -> std::io::Result<u64> {
        std::fs::copy(src, dst)
    }

    fn execute_atomic_copy(src: &Path, dst: &Path) -> std::io::Result<(u32, Vec<PathBuf>)> {
        std::fs::create_dir_all(dst)?;
        let mut copied = Vec::new();
        let mut count = 0u32;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let name = entry.file_name();
                let dest = dst.join(&name);
                if let Err(e) = Self::copy_file_optimized(&entry.path(), &dest) {
                    if e.raw_os_error() == Some(32) {
                        return Err(e);
                    }
                    continue;
                }
                copied.push(dest);
                count += 1;
            }
        }
        Ok((count, copied))
    }

    fn reactive_purge(&self, config: &AppConfig, source_date: chrono::NaiveDate, db: &DatabaseManager) {
        let dias_a_mantener = config.dias_persistencia;
        if dias_a_mantener == 0 {
            return;
        }
        let limit_date = source_date - chrono::Duration::days(dias_a_mantener as i64 - 1);
        self.purge_path_older_than(&config.ruta_copia_normal, limit_date, db, "COPIA_NORMAL");
        if config.habilitar_tar {
            self.purge_path_older_than(&config.ruta_copia_tar, limit_date, db, "COPIA_TAR");
        }
        let limite_str = limit_date.format("%y%m%d").to_string();
        let _ = db.delete_fechas_before(&limite_str);
    }

    fn purge_path_older_than(&self, path: &Path, limit_date: chrono::NaiveDate, db: &DatabaseManager, tag: &str) {
        let read_dir = match std::fs::read_dir(path) {
            Ok(d) => d,
            Err(_) => return,
        };

        for entry in read_dir.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Some(fecha_str) = p.file_name().and_then(|s| s.to_str()) {
                    if fecha_str.len() == 6 && fecha_str.chars().all(|c| c.is_ascii_digit()) {
                        if let Ok(parsed_date) = chrono::NaiveDate::parse_from_str(fecha_str, "%y%m%d") {
                            if parsed_date < limit_date {
                                if let Err(e) = std::fs::remove_dir_all(&p) {
                                    let _ = db.insert_log(
                                        "ERROR", "PURGE_FAIL",
                                        &format!("No se pudo purgar {} en {:?}: {}", fecha_str, path, e),
                                        None, Some(&format!("{:?}", e)),
                                    );
                                } else {
                                    let _ = db.insert_log(
                                        "INFO", "PURGE_SUCCESS",
                                        &format!("Purga reactiva ({}): eliminado {}.", tag, fecha_str),
                                        None, None,
                                    );
                                }
                                let _ = self.tx.send(UiMessage::LogAdded);
                            }
                        }
                    }
                }
            }
        }
    }
}

const TAR_WORKER_COUNT: usize = 4;

fn select_tar_worker(tar_path: &Path, num_workers: usize) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tar_path.hash(&mut hasher);
    (hasher.finish() as usize) % num_workers
}

struct TarWorker;

impl TarWorker {
    fn run_loop(rx: Receiver<TarMessage>, db_path: PathBuf, ui_tx: Sender<UiMessage>) {
        let db = match DatabaseManager::new(&db_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("TarWorker DB error: {:?}", e);
                return;
            }
        };
        let mut builders: HashMap<PathBuf, tar::Builder<std::fs::File>> = HashMap::new();

        while let Ok(msg) = rx.recv() {
            match msg {
                TarMessage::AddFile { dest_path, tar_path } => {
                    if !builders.contains_key(&tar_path) {
                        if let Some(parent) = tar_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        match std::fs::File::create(&tar_path) {
                            Ok(f) => {
                                builders.insert(tar_path.clone(), tar::Builder::new(f));
                            }
                            Err(e) => {
                                let _ = db.insert_log("ERROR", "TAR_FAIL",
                                    &format!("No se pudo crear TAR {:?}: {}", tar_path, e), None, None);
                                continue;
                            }
                        }
                    }
                    if let Some(builder) = builders.get_mut(&tar_path) {
                        if let Some(name) = dest_path.file_name() {
                            if let Ok(mut f) = std::fs::File::open(&dest_path) {
                                let _ = builder.append_file(name, &mut f);
                            }
                        }
                    }
                }
                TarMessage::HourComplete { canal, fecha, hora, tar_path } => {
                    if let Some(mut builder) = builders.remove(&tar_path) {
                        let _ = builder.finish();
                        let _ = db.insert_log("INFO", "TAR_SUCCESS",
                            &format!("TAR {} {} h{} en {:?}", canal, fecha, hora, tar_path),
                            Some(&canal), None);
                        let _ = ui_tx.send(UiMessage::LogAdded);
                    }
                }
            }
        }

        for (_, mut builder) in builders.drain() {
            let _ = builder.finish();
        }
    }
}
