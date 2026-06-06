use std::path::PathBuf;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub ruta_origen: PathBuf,
    pub ruta_copia_normal: PathBuf,
    pub ruta_copia_tar: PathBuf,
    pub horas_desface: i32,
    pub dias_persistencia: u32,
    pub habilitar_normal: bool,
    pub habilitar_tar: bool,
    pub margen_pre_minutos: u32,
    pub margen_post_minutos: u32,
    pub max_hilos_copia: u32,
    pub canales: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ruta_origen: PathBuf::new(),
            ruta_copia_normal: PathBuf::new(),
            ruta_copia_tar: PathBuf::new(),
            horas_desface: 0,
            dias_persistencia: 2,
            habilitar_normal: true,
            habilitar_tar: false,
            margen_pre_minutos: 0,
            margen_post_minutos: 0,
            max_hilos_copia: 4,
            canales: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}
