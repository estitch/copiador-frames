use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    pub max_hilos_tar: u32,
    pub copia_intra_hora: bool,
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
            max_hilos_tar: 2,
            copia_intra_hora: false,
            canales: Vec::new(),
        }
    }
}

impl AppConfig {
    #[cfg(test)]
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    #[cfg(test)]
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}
