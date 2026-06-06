use crate::config::AppConfig;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub id: i64,
    pub timestamp: String,
    pub nivel: String,
    pub tipo_accion: String,
    pub mensaje: String,
    pub canal_contexto: Option<String>,
    pub traza_error: Option<String>,
}

pub struct DatabaseManager {
    conn: Connection,
}

impl DatabaseManager {
    /// Abre la base de datos y configura el entorno para concurrencia segura con WAL
    pub fn new(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        
        // Habilitar modo WAL y configurar timeout para evitar bloqueos
        conn.pragma_update(None, "journal_mode", &"WAL")?;
        conn.pragma_update(None, "foreign_keys", &"ON")?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;

        let db = DatabaseManager { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Crea las tablas según el modelo DER si no existen
    pub fn init_schema(&self) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS CONFIGURACION (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                ruta_origen TEXT NOT NULL,
                ruta_copia_normal TEXT NOT NULL,
                ruta_copia_tar TEXT NOT NULL,
                horas_desface INTEGER NOT NULL,
                dias_persistencia INTEGER NOT NULL,
                habilitar_normal INTEGER NOT NULL,
                habilitar_tar INTEGER NOT NULL,
                margen_pre_minutos INTEGER NOT NULL,
                margen_post_minutos INTEGER NOT NULL
            );",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS CANAL (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                codigo_canal TEXT NOT NULL UNIQUE
            );",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS FECHA_SINCRONIZACION (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                codigo_fecha TEXT NOT NULL UNIQUE
            );",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS REGISTRO_HORA (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                canal_id INTEGER NOT NULL,
                fecha_id INTEGER NOT NULL,
                hora INTEGER NOT NULL,
                cantidad_frames INTEGER NOT NULL,
                estado_color TEXT NOT NULL,
                ultimo_intento TEXT NOT NULL,
                FOREIGN KEY(canal_id) REFERENCES CANAL(id) ON DELETE CASCADE,
                FOREIGN KEY(fecha_id) REFERENCES FECHA_SINCRONIZACION(id) ON DELETE CASCADE,
                UNIQUE(canal_id, fecha_id, hora)
            );",
            [],
        )?;

        // Migraciones para bases de datos existentes
        let _ = self.conn.execute("ALTER TABLE REGISTRO_HORA ADD COLUMN intentos INTEGER NOT NULL DEFAULT 0;", []);
        let _ = self.conn.execute("ALTER TABLE CONFIGURACION ADD COLUMN max_hilos_copia INTEGER NOT NULL DEFAULT 4;", []);

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS LOG_SISTEMA (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                nivel TEXT NOT NULL,
                tipo_accion TEXT NOT NULL,
                mensaje TEXT NOT NULL,
                canal_contexto TEXT,
                traza_error TEXT
            );",
            [],
        )?;

        // Inicializar configuración por defecto si la tabla está vacía
        self.conn.execute(
            "INSERT OR IGNORE INTO CONFIGURACION (
                id, ruta_origen, ruta_copia_normal, ruta_copia_tar, horas_desface, 
                dias_persistencia, habilitar_normal, habilitar_tar, margen_pre_minutos, margen_post_minutos,
                max_hilos_copia
            ) VALUES (1, '', '', '', 0, 2, 1, 0, 0, 0, 4);",
            [],
        )?;

        Ok(())
    }

    /// Obtiene la configuración desde la base de datos sincronizada con los canales registrados
    pub fn get_config(&self) -> Result<AppConfig, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT ruta_origen, ruta_copia_normal, ruta_copia_tar, horas_desface, 
                    dias_persistencia, habilitar_normal, habilitar_tar, margen_pre_minutos, margen_post_minutos,
                    max_hilos_copia
             FROM CONFIGURACION WHERE id = 1",
        )?;

        let mut config = stmt.query_row([], |row| {
            let ruta_origen: String = row.get(0)?;
            let ruta_copia_normal: String = row.get(1)?;
            let ruta_copia_tar: String = row.get(2)?;
            let habilitar_normal_int: i32 = row.get(5)?;
            let habilitar_tar_int: i32 = row.get(6)?;

            Ok(AppConfig {
                ruta_origen: PathBuf::from(ruta_origen),
                ruta_copia_normal: PathBuf::from(ruta_copia_normal),
                ruta_copia_tar: PathBuf::from(ruta_copia_tar),
                horas_desface: row.get(3)?,
                dias_persistencia: row.get(4)?,
                habilitar_normal: habilitar_normal_int != 0,
                habilitar_tar: habilitar_tar_int != 0,
                margen_pre_minutos: row.get(7)?,
                margen_post_minutos: row.get(8)?,
                max_hilos_copia: row.get(9)?,
                canales: Vec::new(),
            })
        })?;

        // Cargar canales
        let mut stmt_canales = self.conn.prepare("SELECT codigo_canal FROM CANAL ORDER BY codigo_canal ASC")?;
        let canales_iter = stmt_canales.query_map([], |row| row.get::<_, String>(0))?;
        for c in canales_iter {
            config.canales.push(c?);
        }

        Ok(config)
    }

    /// Guarda la configuración y sincroniza los canales (agrega nuevos, purga removidos)
    pub fn save_config(&self, config: &AppConfig) -> Result<(), rusqlite::Error> {
        // Actualizar parámetros principales
        self.conn.execute(
            "UPDATE CONFIGURACION SET 
                ruta_origen = ?, 
                ruta_copia_normal = ?, 
                ruta_copia_tar = ?, 
                horas_desface = ?, 
                dias_persistencia = ?, 
                habilitar_normal = ?, 
                habilitar_tar = ?, 
                margen_pre_minutos = ?, 
                margen_post_minutos = ?,
                max_hilos_copia = ?
             WHERE id = 1",
            params![
                config.ruta_origen.to_string_lossy().to_string(),
                config.ruta_copia_normal.to_string_lossy().to_string(),
                config.ruta_copia_tar.to_string_lossy().to_string(),
                config.horas_desface,
                config.dias_persistencia,
                if config.habilitar_normal { 1 } else { 0 },
                if config.habilitar_tar { 1 } else { 0 },
                config.margen_pre_minutos,
                config.margen_post_minutos,
                config.max_hilos_copia,
            ],
        )?;

        // Sincronizar canales en base de datos
        // 1. Insertar canales faltantes
        for canal in &config.canales {
            self.conn.execute(
                "INSERT OR IGNORE INTO CANAL (codigo_canal) VALUES (?)",
                params![canal],
            )?;
        }

        // 2. Eliminar canales que ya no están en config
        if config.canales.is_empty() {
            self.conn.execute("DELETE FROM CANAL", [])?;
        } else {
            let vars = vec!["?"; config.canales.len()].join(",");
            let sql = format!("DELETE FROM CANAL WHERE codigo_canal NOT IN ({})", vars);
            
            // rusqlite exige pasar los parámetros en una estructura compatible
            let params_ref: Vec<&dyn rusqlite::ToSql> = config.canales.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            self.conn.execute(&sql, rusqlite::params_from_iter(params_ref))?;
        }

        Ok(())
    }

    /// Obtiene o crea una fecha y retorna su ID
    pub fn get_or_create_fecha(&self, codigo_fecha: &str) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT OR IGNORE INTO FECHA_SINCRONIZACION (codigo_fecha) VALUES (?)",
            params![codigo_fecha],
        )?;
        self.conn.query_row(
            "SELECT id FROM FECHA_SINCRONIZACION WHERE codigo_fecha = ?",
            params![codigo_fecha],
            |row| row.get(0),
        )
    }

    /// Busca un canal registrado y retorna su ID. No crea el canal si no existe.
    pub fn get_canal(&self, codigo_canal: &str) -> Result<Option<i64>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM CANAL WHERE codigo_canal = ?",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![codigo_canal], |row| row.get::<_, i64>(0))?;
        if let Some(res) = rows.next() {
            Ok(Some(res?))
        } else {
            Ok(None)
        }
    }

    pub fn update_hora_status(
        &self,
        canal: &str,
        fecha: &str,
        hora: u8,
        status: &str,
        count: u32,
    ) -> Result<(), rusqlite::Error> {
        // Solo operar sobre canales previamente configurados (evita insertar canales fantasma)
        let canal_id = match self.get_canal(canal)? {
            Some(id) => id,
            None => return Ok(()), // Canal no existe en config, ignorar silenciosamente
        };
        let fecha_id = self.get_or_create_fecha(fecha)?;
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let sql = if status == "Verde" {
            "INSERT INTO REGISTRO_HORA (canal_id, fecha_id, hora, cantidad_frames, estado_color, ultimo_intento, intentos)
             VALUES (?, ?, ?, ?, ?, ?, 0)
             ON CONFLICT(canal_id, fecha_id, hora) DO UPDATE SET
                cantidad_frames = excluded.cantidad_frames,
                estado_color = excluded.estado_color,
                ultimo_intento = excluded.ultimo_intento,
                intentos = 0"
        } else {
            "INSERT INTO REGISTRO_HORA (canal_id, fecha_id, hora, cantidad_frames, estado_color, ultimo_intento)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(canal_id, fecha_id, hora) DO UPDATE SET
                cantidad_frames = excluded.cantidad_frames,
                estado_color = excluded.estado_color,
                ultimo_intento = excluded.ultimo_intento"
        };

        self.conn.execute(sql, params![canal_id, fecha_id, hora, count, status, now])?;

        Ok(())
    }

    /// Obtiene los intentos y el último intento para una hora específica
    pub fn get_hora_intentos(
        &self,
        canal: &str,
        fecha: &str,
        hora: u8,
    ) -> Result<Option<(u32, String)>, rusqlite::Error> {
        let canal_id = match self.get_canal(canal)? {
            Some(id) => id,
            None => return Ok(None),
        };
        let fecha_id = self.get_or_create_fecha(fecha)?;

        let mut stmt = self.conn.prepare(
            "SELECT intentos, ultimo_intento FROM REGISTRO_HORA WHERE canal_id = ? AND fecha_id = ? AND hora = ?",
        )?;

        let mut rows = stmt.query_map(params![canal_id, fecha_id, hora], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })?;

        if let Some(res) = rows.next() {
            Ok(Some(res?))
        } else {
            Ok(None)
        }
    }

    /// Incrementa los intentos para una hora específica y establece su marca de tiempo
    pub fn increment_hora_intentos(
        &self,
        canal: &str,
        fecha: &str,
        hora: u8,
    ) -> Result<(), rusqlite::Error> {
        let canal_id = match self.get_canal(canal)? {
            Some(id) => id,
            None => return Ok(()), // Canal no registrado, ignorar
        };
        let fecha_id = self.get_or_create_fecha(fecha)?;
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        self.conn.execute(
            "INSERT INTO REGISTRO_HORA (canal_id, fecha_id, hora, cantidad_frames, estado_color, ultimo_intento, intentos)
             VALUES (?, ?, ?, 0, 'Rojo', ?, 1)
             ON CONFLICT(canal_id, fecha_id, hora) DO UPDATE SET
                intentos = REGISTRO_HORA.intentos + 1,
                ultimo_intento = excluded.ultimo_intento",
            params![canal_id, fecha_id, hora, now],
        )?;

        Ok(())
    }

    /// Obtiene los registros de hora de una fecha específica
    pub fn get_registro_horas_por_fecha(
        &self,
        codigo_fecha: &str,
    ) -> Result<Vec<(String, u8, String, u32)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT c.codigo_canal, r.hora, r.estado_color, r.cantidad_frames 
             FROM REGISTRO_HORA r
             JOIN CANAL c ON r.canal_id = c.id
             JOIN FECHA_SINCRONIZACION f ON r.fecha_id = f.id
             WHERE f.codigo_fecha = ?",
        )?;

        let rows = stmt.query_map(params![codigo_fecha], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u8>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u32>(3)?,
            ))
        })?;

        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    /// Registra un evento en la base de datos
    pub fn insert_log(
        &self,
        nivel: &str,
        tipo_accion: &str,
        mensaje: &str,
        canal: Option<&str>,
        traza: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        self.conn.execute(
            "INSERT INTO LOG_SISTEMA (timestamp, nivel, tipo_accion, mensaje, canal_contexto, traza_error) 
             VALUES (?, ?, ?, ?, ?, ?)",
            params![now, nivel, tipo_accion, mensaje, canal, traza],
        )?;
        Ok(())
    }

    /// Obtiene las últimas N entradas de logs
    pub fn get_recent_logs(&self, limit: u32) -> Result<Vec<LogEntry>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, timestamp, nivel, tipo_accion, mensaje, canal_contexto, traza_error 
             FROM LOG_SISTEMA 
             ORDER BY id DESC LIMIT ?",
        )?;

        let rows = stmt.query_map(params![limit], |row| {
            Ok(LogEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                nivel: row.get(2)?,
                tipo_accion: row.get(3)?,
                mensaje: row.get(4)?,
                canal_contexto: row.get(5)?,
                traza_error: row.get(6)?,
            })
        })?;

        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    /// Obtiene todas las fechas sincronizadas registradas
    pub fn get_all_dates(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT codigo_fecha FROM FECHA_SINCRONIZACION ORDER BY codigo_fecha DESC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    /// Elimina registros de FECHA_SINCRONIZACION (y REGISTRO_HORA en cascada) cuyo
    /// codigo_fecha YYMMDD sea estrictamente anterior al límite dado.
    ///
    /// El formato YYMMDD es lexicográficamente comparable en SQL (260514 < 260522).
    /// Esto limpia la base de datos cuando las carpetas de copia son purgadas.
    /// La carpeta de ORIGEN nunca se toca — solo se limpian registros de la copia.
    pub fn delete_fechas_before(&self, limite_yymmdd: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM FECHA_SINCRONIZACION WHERE codigo_fecha < ?",
            params![limite_yymmdd],
        )?;
        Ok(())
    }
}

pub struct LogBuffer {
    entries: Vec<(String, String, String, Option<String>, Option<String>)>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub fn push(&mut self, nivel: &str, tipo_accion: &str, mensaje: &str, canal: Option<&str>, traza: Option<&str>) {
        self.entries.push((
            nivel.to_string(),
            tipo_accion.to_string(),
            mensaje.to_string(),
            canal.map(|s| s.to_string()),
            traza.map(|s| s.to_string()),
        ));
    }

    pub fn push_log_entry(&mut self, entry: &LogEntry) {
        self.entries.push((
            entry.nivel.clone(),
            entry.tipo_accion.clone(),
            entry.mensaje.clone(),
            entry.canal_contexto.clone(),
            entry.traza_error.clone(),
        ));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn flush(&mut self, db: &mut DatabaseManager) -> Result<(), rusqlite::Error> {
        if self.entries.is_empty() {
            return Ok(());
        }
        db.insert_logs_batch(&self.entries)?;
        self.entries.clear();
        Ok(())
    }
}

impl DatabaseManager {
    pub fn insert_logs_batch(
        &mut self,
        entries: &[(String, String, String, Option<String>, Option<String>)],
    ) -> Result<(), rusqlite::Error> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let tx = self.conn.transaction()?;
        for (nivel, tipo_accion, mensaje, canal, traza) in entries {
            tx.execute(
                "INSERT INTO LOG_SISTEMA (timestamp, nivel, tipo_accion, mensaje, canal_contexto, traza_error) 
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![now, nivel, tipo_accion, mensaje, canal, traza],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_config_json_roundtrip() {
        let mut config = AppConfig::default();
        config.horas_desface = -5;
        config.canales = vec!["1001".to_string(), "1002".to_string()];
        
        let json = config.to_json().unwrap();
        let loaded = AppConfig::from_json(&json).unwrap();
        assert_eq!(loaded.horas_desface, -5);
        assert_eq!(loaded.canales, vec!["1001", "1002"]);
    }

    #[test]
    fn test_database_operations() {
        let db_path = PathBuf::from("test_copiador.db");
        // Asegurarse de que no exista
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file("test_copiador.db-shm");
        let _ = std::fs::remove_file("test_copiador.db-wal");

        let db = DatabaseManager::new(&db_path).unwrap();
        
        // Test insert log
        db.insert_log("INFO", "TEST_ACTION", "Testing log insertion", Some("1001"), None).unwrap();
        let logs = db.get_recent_logs(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].nivel, "INFO");
        assert_eq!(logs[0].mensaje, "Testing log insertion");

        // Test save config
        let mut config = db.get_config().unwrap();
        config.horas_desface = 3;
        config.canales = vec!["1234".to_string()];
        db.save_config(&config).unwrap();

        let loaded_config = db.get_config().unwrap();
        assert_eq!(loaded_config.horas_desface, 3);
        assert_eq!(loaded_config.canales, vec!["1234"]);

        // Test update status
        db.update_hora_status("1234", "260519", 2, "Verde", 3600).unwrap();
        let status = db.get_registro_horas_por_fecha("260519").unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].0, "1234");
        assert_eq!(status[0].1, 2);
        assert_eq!(status[0].2, "Verde");
        assert_eq!(status[0].3, 3600);

        // Test attempts tracking
        let attempts_none = db.get_hora_intentos("1234", "260519", 2).unwrap();
        assert_eq!(attempts_none.unwrap().0, 0);

        db.increment_hora_intentos("1234", "260519", 3).unwrap();
        let attempts_one = db.get_hora_intentos("1234", "260519", 3).unwrap();
        assert_eq!(attempts_one.unwrap().0, 1);

        db.increment_hora_intentos("1234", "260519", 3).unwrap();
        let attempts_two = db.get_hora_intentos("1234", "260519", 3).unwrap();
        assert_eq!(attempts_two.unwrap().0, 2);

        // Limpiar
        drop(db); // Cerrar conexión
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file("test_copiador.db-shm");
        let _ = std::fs::remove_file("test_copiador.db-wal");
    }

    #[test]
    fn test_log_buffer_batch_insert() {
        let db_path = PathBuf::from("test_copiador_log.db");
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file("test_copiador_log.db-shm");
        let _ = std::fs::remove_file("test_copiador_log.db-wal");

        let mut db = DatabaseManager::new(&db_path).unwrap();

        let mut buf = LogBuffer::new();
        assert!(buf.is_empty());

        buf.push("INFO", "TEST", "msg1", Some("C1"), None);
        buf.push("WARN", "TEST", "msg2", None, Some("trace"));
        assert_eq!(buf.len(), 2);

        buf.flush(&mut db).unwrap();
        assert!(buf.is_empty());

        let logs = db.get_recent_logs(10).unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].nivel, "WARN");
        assert_eq!(logs[1].mensaje, "msg1");

        drop(db);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file("test_copiador_log.db-shm");
        let _ = std::fs::remove_file("test_copiador_log.db-wal");
    }

    #[test]
    fn test_config_max_hilos_copia() {
        let mut config = AppConfig::default();
        assert_eq!(config.max_hilos_copia, 4);
        config.max_hilos_copia = 8;
        let json = config.to_json().unwrap();
        let loaded = AppConfig::from_json(&json).unwrap();
        assert_eq!(loaded.max_hilos_copia, 8);
    }
}
