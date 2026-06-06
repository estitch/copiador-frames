mod config;
mod db;
mod worker;

use config::AppConfig;
use db::{DatabaseManager, LogEntry};
use worker::{SyncWorker, WorkerCommand, UiMessage};

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Instant, Duration};

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Dashboard,
    Matrix,
    Settings,
    Channels,
}

struct CopiadorApp {
    db_path: PathBuf,
    tx: Sender<WorkerCommand>,
    rx: Receiver<UiMessage>,
    worker_thread: Option<std::thread::JoinHandle<()>>,
    
    // Configuración activa y buffer de edición
    current_config: AppConfig,
    config_edit: AppConfig,

    // Estado del Frontend
    active_tab: Tab,
    cached_logs: Vec<LogEntry>,
    selected_error_log: Option<LogEntry>,

    // Estado de la Matriz de Canales
    selected_date: String,
    available_dates: Vec<String>,
    matrix_data: Vec<(String, u8, String, u32)>, // (canal, hora, estado, cantidad)

    // Entradas auxiliares
    new_canal_input: String,

    // Monitoreo
    is_syncing: bool,
    last_refresh: Instant,

    // Sidebar colapsable
    sidebar_collapsed: bool,
}

impl CopiadorApp {
    pub fn new(db_path: PathBuf) -> Self {
        // Inicializar base de datos
        let db = DatabaseManager::new(&db_path).expect("No se pudo iniciar la base de datos");
        let config = db.get_config().unwrap_or_default();

        // Configurar canales de comunicación
        let (worker_tx, worker_rx) = channel();
        let (ui_tx, ui_rx) = channel();

        // Spawnear el background worker en un hilo independiente
        let worker_db_path = db_path.clone();
        let handle = std::thread::spawn(move || {
            let mut worker = SyncWorker::new(worker_db_path, worker_rx, ui_tx);
            worker.run_loop();
        });

        let mut app = Self {
            db_path,
            tx: worker_tx,
            rx: ui_rx,
            worker_thread: Some(handle),
            current_config: config.clone(),
            config_edit: config,
            active_tab: Tab::Dashboard,
            cached_logs: Vec::new(),
            selected_error_log: None,
            selected_date: String::new(),
            available_dates: Vec::new(),
            matrix_data: Vec::new(),
            new_canal_input: String::new(),
            is_syncing: false,
            last_refresh: Instant::now() - Duration::from_secs(10),
            sidebar_collapsed: false,
        };

        app.refresh_logs();
        app.refresh_dates_and_matrix();
        app
    }

    /// Carga las últimas entradas del log
    fn refresh_logs(&mut self) {
        if let Ok(db) = DatabaseManager::new(&self.db_path) {
            if let Ok(logs) = db.get_recent_logs(60) {
                self.cached_logs = logs;
            }
        }
    }

    /// Carga las fechas y la matriz de estados de la fecha seleccionada.
    /// Solo muestra fechas dentro de la ventana de persistencia actual.
    fn refresh_dates_and_matrix(&mut self) {
        if let Ok(db) = DatabaseManager::new(&self.db_path) {
            if let Ok(dates) = db.get_all_dates() {
                // Calcular ventana de persistencia usando la fecha del origen (con desfase)
                let source_date = (chrono::Local::now()
                    + chrono::Duration::hours(self.current_config.horas_desface as i64))
                    .date_naive();
                let dias = self.current_config.dias_persistencia.max(1);
                let limit_date = source_date - chrono::Duration::days(dias as i64 - 1);

                // Filtrar: solo fechas en [limit_date, source_date]
                self.available_dates = dates
                    .into_iter()
                    .filter(|d| {
                        chrono::NaiveDate::parse_from_str(d, "%y%m%d")
                            .map(|parsed| parsed >= limit_date && parsed <= source_date)
                            .unwrap_or(false)
                    })
                    .collect();

                // Seleccionar la fecha más reciente si no hay ninguna o la actual quedó fuera
                if self.selected_date.is_empty() || !self.available_dates.contains(&self.selected_date) {
                    self.selected_date = self.available_dates.first().cloned().unwrap_or_default();
                }
            }

            if !self.selected_date.is_empty() {
                if let Ok(matrix) = db.get_registro_horas_por_fecha(&self.selected_date) {
                    self.matrix_data = matrix;
                }
            }
        }
    }

    /// Aplica estilos visuales premium (Tokyo Night / Cyberpunk)
    fn apply_theme(&self, ctx: &egui::Context) {
        let mut style = (*ctx.global_style()).clone();
        
        style.visuals.dark_mode = false;
        style.visuals.override_text_color = None; // Usar color de texto predeterminado del sistema (alto contraste)
        
        // Colores de fondo de paneles y ventanas principales para asegurar el modo claro limpio
        style.visuals.panel_fill = egui::Color32::from_rgb(243, 244, 246);
        style.visuals.window_fill = egui::Color32::from_rgb(243, 244, 246);
        
        // Colores de fondo de widgets (tarjetas contenedoras en blanco limpio) con texto oscuro
        style.visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(255, 255, 255);
        style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(17, 24, 39));
        style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(8);
        
        // Widget inactivo (botones, dropdowns) con texto en gris oscuro
        style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(255, 255, 255);
        style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(31, 41, 55));
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        
        // Widget hovered
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(249, 250, 251);
        style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(17, 24, 39));
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        
        // Widget activo / clickeado
        style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(239, 246, 255);
        style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(30, 58, 138));
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
        
        ctx.set_global_style(style);
    }
}

/// Convierte un código de fecha YYMMDD (ej: "260522") en una cadena legible "22/05/2026".
/// Retorna el código original si el formato no es válido.
fn format_fecha_display(codigo: &str) -> String {
    if codigo.len() == 6 {
        if let (Ok(yy), Ok(mm), Ok(dd)) = (
            codigo[0..2].parse::<u32>(),
            codigo[2..4].parse::<u32>(),
            codigo[4..6].parse::<u32>(),
        ) {
            return format!("{:02}/{:02}/{}", dd, mm, 2000 + yy);
        }
    }
    codigo.to_string()
}

impl eframe::App for CopiadorApp {
    fn on_exit(&mut self) {
        let _ = self.tx.send(WorkerCommand::Shutdown);
        if let Some(handle) = self.worker_thread.take() {
            let _ = handle.join();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx();
        self.apply_theme(ctx);

        // Procesar mensajes del background worker en tiempo real
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                UiMessage::SyncStarted => {
                    self.is_syncing = true;
                }
                UiMessage::SyncFinished => {
                    self.is_syncing = false;
                    self.refresh_dates_and_matrix();
                    self.refresh_logs();
                }
                UiMessage::StatusUpdated { canal, fecha, hora, status, count } => {
                    // Si coincide con la fecha mostrada, actualizar matriz en memoria
                    if fecha == self.selected_date {
                        if let Some(pos) = self.matrix_data.iter().position(|(c, h, _, _)| c == &canal && *h == hora) {
                            self.matrix_data[pos] = (canal, hora, status, count);
                        } else {
                            self.matrix_data.push((canal, hora, status, count));
                        }
                    }
                }
                UiMessage::LogAdded => {
                    self.refresh_logs();
                }
            }
        }

        // Refresco periódico automático (cada 5 segundos)
        if self.last_refresh.elapsed() >= Duration::from_secs(5) {
            self.refresh_dates_and_matrix();
            self.refresh_logs();
            self.last_refresh = Instant::now();
        }

        // Solicitar repintado continuo si está sincronizando (para actualizar relojes y estado animado)
        ctx.request_repaint();

        // Diseño general de la ventana
        let sidebar_width = if self.sidebar_collapsed { 60.0 } else { 200.0 };

        egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(sidebar_width)
            .show_inside(ui, |ui| {
                if self.sidebar_collapsed {
                    ui.vertical_centered(|ui| {
                        ui.add_space(15.0);
                        if ui.button(">").on_hover_text("Expandir Sidebar").clicked() {
                            self.sidebar_collapsed = false;
                        }
                        ui.add_space(25.0);

                        let mut render_icon_btn = |ui: &mut egui::Ui, tab: Tab, icon: &str, tooltip: &str| {
                            let is_selected = self.active_tab == tab;
                            let btn = egui::Button::new(
                                egui::RichText::new(icon)
                                    .size(14.0)
                                    .color(if is_selected { egui::Color32::from_rgb(37, 99, 235) } else { egui::Color32::from_rgb(75, 85, 99) })
                            ).frame(true);
                            
                            let response = ui.add(btn);
                            if response.clicked() {
                                self.active_tab = tab;
                                self.selected_error_log = None;
                            }
                            response.on_hover_text(tooltip);
                            ui.add_space(15.0);
                        };

                        render_icon_btn(ui, Tab::Dashboard, "[D]", "Dashboard");
                        render_icon_btn(ui, Tab::Matrix, "[M]", "Matriz de Estado");
                        render_icon_btn(ui, Tab::Settings, "[S]", "Configuracion");
                        render_icon_btn(ui, Tab::Channels, "[C]", "Canales Activos");
                    });
                } else {
                    ui.vertical(|ui| {
                        ui.add_space(15.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.heading("HEIMDALL");
                                ui.label(egui::RichText::new("Copiador de Canales").weak().size(11.0));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("<").on_hover_text("Colapsar Sidebar").clicked() {
                                    self.sidebar_collapsed = true;
                                }
                            });
                        });
                        ui.add_space(25.0);

                        let mut render_tab_btn = |ui: &mut egui::Ui, tab: Tab, label: &str| {
                            let is_selected = self.active_tab == tab;
                            let btn = egui::Button::new(
                                egui::RichText::new(label)
                                    .size(14.0)
                                    .color(if is_selected { egui::Color32::from_rgb(37, 99, 235) } else { egui::Color32::from_rgb(75, 85, 99) })
                            ).frame(true);
                            
                            if ui.add(btn).clicked() {
                                self.active_tab = tab;
                                self.selected_error_log = None;
                            }
                            ui.add_space(10.0);
                        };

                        render_tab_btn(ui, Tab::Dashboard, "Dashboard");
                        render_tab_btn(ui, Tab::Matrix, "Matriz de Estado");
                        render_tab_btn(ui, Tab::Settings, "Configuracion");
                        render_tab_btn(ui, Tab::Channels, "Canales Activos");
                    });
                }

                // Indicador de sincronización en la parte inferior (colores más oscuros para modo claro)
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(15.0);
                    if self.is_syncing {
                        if self.sidebar_collapsed {
                            ui.label(egui::RichText::new("[Sync]").color(egui::Color32::from_rgb(194, 65, 12)).strong())
                                .on_hover_text("Sincronizando...");
                        } else {
                            ui.label(egui::RichText::new("Sincronizando...").color(egui::Color32::from_rgb(194, 65, 12)).strong());
                        }
                    } else {
                        if self.sidebar_collapsed {
                            ui.label(egui::RichText::new("[Ok]").color(egui::Color32::from_rgb(22, 101, 52)).size(12.0))
                                .on_hover_text("Listo / En Espera");
                        } else {
                            ui.label(egui::RichText::new("Esperando ciclo").color(egui::Color32::from_rgb(22, 101, 52)).size(12.0));
                        }
                    }
                    ui.add_space(10.0);
                });
            });

        // Contenedor principal de pestañas
        egui::CentralPanel::default().show_inside(ui, |ui| {
            match self.active_tab {
                Tab::Dashboard => self.render_dashboard(ui),
                Tab::Matrix => self.render_matrix(ui),
                Tab::Settings => self.render_settings(ui),
                Tab::Channels => self.render_channels(ui),
            }
        });
    }
}

impl CopiadorApp {
    /// Renderiza el Dashboard
    fn render_dashboard(&mut self, ui: &mut egui::Ui) {
        ui.heading("Dashboard de Control");
        ui.add_space(8.0);

        let local_time = chrono::Local::now();
        let offset_hours = self.current_config.horas_desface;
        let remote_time = local_time + chrono::Duration::hours(offset_hours as i64);

        // Relojes y estado del worker: 3 columnas compactas
        ui.columns(3, |cols| {
            // Columna 1: Reloj Local
            cols[0].vertical_centered(|ui| {
                ui.set_min_width(ui.available_width());
                egui::Frame::new()
                    .fill(egui::Color32::WHITE)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(229, 231, 235)))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(egui::RichText::new("Reloj Local").size(10.0).color(egui::Color32::from_rgb(107, 114, 128)));
                        ui.label(egui::RichText::new(local_time.format("%H:%M:%S").to_string()).size(20.0).strong());
                        ui.label(egui::RichText::new(local_time.format("%d/%m/%Y").to_string()).size(11.0).color(egui::Color32::from_rgb(107, 114, 128)));
                    });
            });

            // Columna 2: Reloj Origen
            cols[1].vertical_centered(|ui| {
                ui.set_min_width(ui.available_width());
                egui::Frame::new()
                    .fill(egui::Color32::WHITE)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(219, 234, 254)))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(egui::RichText::new("Origen (Desfase)").size(10.0).color(egui::Color32::from_rgb(107, 114, 128)));
                        ui.label(
                            egui::RichText::new(remote_time.format("%H:%M:%S").to_string())
                                .size(20.0)
                                .strong()
                                .color(egui::Color32::from_rgb(29, 78, 216))
                        );
                        ui.label(egui::RichText::new(format!("Desfase: {} hrs", offset_hours)).size(11.0).color(egui::Color32::from_rgb(107, 114, 128)));
                    });
            });

            // Columna 3: Acciones
            cols[2].vertical_centered(|ui| {
                ui.set_min_width(ui.available_width());
                egui::Frame::new()
                    .fill(egui::Color32::WHITE)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(229, 231, 235)))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(egui::RichText::new("Acciones").size(10.0).color(egui::Color32::from_rgb(107, 114, 128)));
                        ui.add_space(4.0);
                        if ui.button("Sincronizar Ahora").clicked() {
                            let _ = self.tx.send(WorkerCommand::TriggerSync);
                        }
                    });
            });
        });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        // Registro de logs
        ui.label(egui::RichText::new("Registro de logs").strong());
        ui.add_space(4.0);

        // Si hay una traza de error seleccionada, mostrar panel detallado
        if let Some(log) = self.selected_error_log.clone() {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(254, 242, 242))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(252, 165, 165)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Traza de Error").strong().size(11.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Cerrar").clicked() {
                                self.selected_error_log = None;
                            }
                        });
                    });
                    ui.label(egui::RichText::new(format!("{} — {}", log.timestamp, log.mensaje)).size(10.0));
                    let traza = log.traza_error.as_deref().unwrap_or("Sin traza.");
                    egui::ScrollArea::vertical().max_height(60.0).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut traza.to_string())
                                .font(egui::TextStyle::Monospace)
                                .desired_rows(3)
                                .lock_focus(true)
                        );
                    });
                });
            ui.add_space(6.0);
        }

        // Tabla de logs compacta responsive a todo el ancho
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                let total_w = ui.available_width() - 15.0;

                // Cabecera
                ui.horizontal(|ui| {
                    ui.add_sized([total_w * 0.18, 18.0], egui::Label::new(egui::RichText::new("Hora").strong().size(11.0)));
                    ui.add_sized([total_w * 0.09, 18.0], egui::Label::new(egui::RichText::new("Nivel").strong().size(11.0)));
                    ui.add_sized([total_w * 0.17, 18.0], egui::Label::new(egui::RichText::new("Accion").strong().size(11.0)));
                    ui.add_sized([total_w * 0.46, 18.0], egui::Label::new(egui::RichText::new("Mensaje").strong().size(11.0)));
                    ui.add_sized([total_w * 0.10, 18.0], egui::Label::new(egui::RichText::new("Canal").strong().size(11.0)));
                });
                ui.separator();

                // Filas compactas alternadas
                for (idx, log) in self.cached_logs.iter().take(50).enumerate() {
                    let row_fill = if idx % 2 == 0 {
                        egui::Color32::from_rgb(255, 255, 255)
                    } else {
                        egui::Color32::from_rgb(249, 250, 251)
                    };

                    egui::Frame::new()
                        .fill(row_fill)
                        .inner_margin(egui::Margin::symmetric(4, 2))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_sized([total_w * 0.18, 16.0], egui::Label::new(egui::RichText::new(&log.timestamp).size(10.0)));

                                let nivel_text = match log.nivel.as_str() {
                                    "ERROR" => egui::RichText::new("ERROR").size(10.0).color(egui::Color32::from_rgb(220, 38, 38)).strong(),
                                    "WARN"  => egui::RichText::new("WARN").size(10.0).color(egui::Color32::from_rgb(217, 119, 6)).strong(),
                                    _       => egui::RichText::new("INFO").size(10.0).color(egui::Color32::from_rgb(107, 114, 128)),
                                };

                                ui.allocate_ui(egui::vec2(total_w * 0.09, 16.0), |ui| {
                                    if log.nivel == "ERROR" {
                                        if ui.link(nivel_text).clicked() {
                                            self.selected_error_log = Some(log.clone());
                                        }
                                    } else {
                                        ui.label(nivel_text);
                                    }
                                });

                                ui.add_sized([total_w * 0.17, 16.0], egui::Label::new(egui::RichText::new(&log.tipo_accion).size(10.0)).truncate());
                                ui.add_sized([total_w * 0.46, 16.0], egui::Label::new(egui::RichText::new(&log.mensaje).size(10.0)).truncate());
                                ui.add_sized([total_w * 0.10, 16.0], egui::Label::new(egui::RichText::new(log.canal_contexto.as_deref().unwrap_or("-")).size(10.0)).truncate());
                            });
                        });
                }
            });
    }

    /// Renderiza la Matriz de Canales (compacta)
    fn render_matrix(&mut self, ui: &mut egui::Ui) {
        ui.heading("Matriz de Estado de Sincronizacion");
        ui.add_space(6.0);

        // Filtro de fecha
        ui.horizontal(|ui| {
            ui.label("Fecha:");
            egui::ComboBox::from_id_salt("date_combobox")
                .selected_text(format_fecha_display(&self.selected_date))
                .show_ui(ui, |ui| {
                    let available_dates = self.available_dates.clone();
                    for date in &available_dates {
                        if ui.selectable_value(&mut self.selected_date, date.clone(), format_fecha_display(date)).clicked() {
                            self.refresh_dates_and_matrix();
                        }
                    }
                });
            if ui.small_button("Actualizar").clicked() {
                self.refresh_dates_and_matrix();
            }
        });

        ui.add_space(8.0);

        if self.current_config.canales.is_empty() {
            ui.label("No hay canales configurados. Configure canales en la pestana 'Canales Activos'.");
            return;
        }

        // Dimensiones compactas de celda y canal
        const CELL_W: f32 = 16.0;
        const CELL_H: f32 = 16.0;
        const CELL_GAP: f32 = 2.0;
        const CANAL_W: f32 = 80.0;
        const ROW_GAP: f32 = 3.0;

        egui::ScrollArea::both().show(ui, |ui| {
            // --- Cabecera de horas ---
            ui.horizontal(|ui| {
                ui.add_space(CANAL_W + CELL_GAP);
                for h in 0u8..24 {
                    let label = egui::RichText::new(format!("{}", h))
                        .size(9.0)
                        .color(egui::Color32::from_rgb(80, 80, 80));
                    ui.add_sized(
                        [CELL_W, 14.0],
                        egui::Label::new(label),
                    );
                    if h < 23 {
                        ui.add_space(CELL_GAP);
                    }
                }
            });

            ui.add_space(2.0);

            // --- Filas de canales ---
            let canales = self.current_config.canales.clone();
            let matrix_data = self.matrix_data.clone();

            for canal in &canales {
                ui.horizontal(|ui| {
                    // Etiqueta canal truncada
                    let canal_label = egui::RichText::new(canal)
                        .size(10.0)
                        .color(egui::Color32::from_rgb(30, 30, 30));
                    ui.add_sized(
                        [CANAL_W, CELL_H],
                        egui::Label::new(canal_label).truncate(),
                    );
                    ui.add_space(CELL_GAP);

                    for hora in 0u8..24 {
                        let (color, count, estado_str) = match matrix_data.iter().find(|(c, h, _, _)| c == canal && *h == hora) {
                            Some((_, _, estado, cant)) => {
                                let c = match estado.as_str() {
                                    "Verde"   => egui::Color32::from_rgb(34, 197, 94),
                                    "Azul"    => egui::Color32::from_rgb(59, 130, 246),
                                    "Naranja" => egui::Color32::from_rgb(251, 146, 60),
                                    _         => egui::Color32::from_rgb(239, 68, 68),
                                };
                                (c, *cant, estado.clone())
                            }
                            None => (egui::Color32::from_rgb(209, 213, 219), 0u32, "Sin datos".to_string()),
                        };

                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(CELL_W, CELL_H),
                            egui::Sense::hover(),
                        );

                        if ui.is_rect_visible(rect) {
                            ui.painter().rect_filled(rect, 2.0, color);
                        }

                        response.on_hover_ui(|ui| {
                            ui.label(egui::RichText::new(format!("Canal: {}", canal)).strong());
                            ui.label(format!("Hora: {:02}:00", hora));
                            ui.label(format!("Estado: {}", estado_str));
                            ui.label(format!("Archivos: {} / 3600", count));
                        });

                        if hora < 23 {
                            ui.add_space(CELL_GAP);
                        }
                    }
                });
                ui.add_space(ROW_GAP);
            }

            // Leyenda compacta
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (color, label) in [
                    (egui::Color32::from_rgb(34, 197, 94),  "Completo"),
                    (egui::Color32::from_rgb(59, 130, 246), "Incompleto"),
                    (egui::Color32::from_rgb(251, 146, 60), "Copiando"),
                    (egui::Color32::from_rgb(239, 68, 68),  "Error"),
                    (egui::Color32::from_rgb(209, 213, 219), "Sin datos"),
                ] {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, color);
                    ui.label(egui::RichText::new(label).size(10.0).color(egui::Color32::from_rgb(60, 60, 60)));
                    ui.add_space(8.0);
                }
            });
        });
    }

    /// Renderiza la Configuracion
    fn render_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Configuracion del Sistema").strong().size(14.0));
        ui.add_space(8.0);

        egui::ScrollArea::vertical().show(ui, |ui| {

            // --- Rutas (fuera del Grid para ocupar todo el ancho) ---
            let btn_w = 60.0;
            let gap   = 6.0;

            // Origen
            ui.label(egui::RichText::new("Origen (Red):").size(11.0));
            ui.horizontal(|ui| {
                let avail = ui.available_width() - btn_w - gap;
                let mut s = self.config_edit.ruta_origen.to_string_lossy().to_string();
                ui.add(egui::TextEdit::singleline(&mut s)
                    .desired_width(avail)
                    .font(egui::TextStyle::Monospace));
                self.config_edit.ruta_origen = PathBuf::from(s);
                if ui.small_button("Carpeta").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.config_edit.ruta_origen = p;
                    }
                }
            });
            ui.add_space(4.0);

            // Destino Normal
            ui.label(egui::RichText::new("Destino Normal:").size(11.0));
            ui.horizontal(|ui| {
                let avail = ui.available_width() - btn_w - gap;
                let mut s = self.config_edit.ruta_copia_normal.to_string_lossy().to_string();
                ui.add(egui::TextEdit::singleline(&mut s)
                    .desired_width(avail)
                    .font(egui::TextStyle::Monospace));
                self.config_edit.ruta_copia_normal = PathBuf::from(s);
                if ui.small_button("Carpeta").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.config_edit.ruta_copia_normal = p;
                    }
                }
            });
            ui.add_space(4.0);

            // Destino TAR
            ui.label(egui::RichText::new("Destino TAR:").size(11.0));
            ui.horizontal(|ui| {
                let avail = ui.available_width() - btn_w - gap;
                let mut s = self.config_edit.ruta_copia_tar.to_string_lossy().to_string();
                ui.add(egui::TextEdit::singleline(&mut s)
                    .desired_width(avail)
                    .font(egui::TextStyle::Monospace));
                self.config_edit.ruta_copia_tar = PathBuf::from(s);
                if ui.small_button("Carpeta").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.config_edit.ruta_copia_tar = p;
                    }
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            // --- Parametros numericos y checkboxes en Grid de 2 columnas ---
            egui::Grid::new("settings_params")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Desfase Horario:").size(11.0));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.config_edit.horas_desface).range(-23..=23));
                        ui.label(egui::RichText::new("horas").size(10.0).color(egui::Color32::from_rgb(107, 114, 128)));
                    });
                    ui.end_row();

                    ui.label(egui::RichText::new("Dias de Persistencia:").size(11.0));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.config_edit.dias_persistencia).range(1..=365));
                        ui.label(egui::RichText::new("dias").size(10.0).color(egui::Color32::from_rgb(107, 114, 128)));
                    });
                    ui.end_row();

                    ui.label(egui::RichText::new("Margen Pre-Hora:").size(11.0));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.config_edit.margen_pre_minutos).range(0..=59));
                        ui.label(egui::RichText::new("min antes del cambio").size(10.0).color(egui::Color32::from_rgb(107, 114, 128)));
                    });
                    ui.end_row();

                    ui.label(egui::RichText::new("Margen Post-Hora:").size(11.0));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.config_edit.margen_post_minutos).range(0..=59));
                        ui.label(egui::RichText::new("min tras el cambio").size(10.0).color(egui::Color32::from_rgb(107, 114, 128)));
                    });
                    ui.end_row();

                    ui.label(egui::RichText::new("Copia Normal:").size(11.0));
                    ui.checkbox(&mut self.config_edit.habilitar_normal, "");
                    ui.end_row();

                    ui.label(egui::RichText::new("Copia TAR:").size(11.0));
                    ui.checkbox(&mut self.config_edit.habilitar_tar, "");
                    ui.end_row();

                    ui.label(egui::RichText::new("Hilos de Copia:").size(11.0));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.config_edit.max_hilos_copia).range(1..=16));
                        ui.label(egui::RichText::new("hilos (1-16)").size(10.0).color(egui::Color32::from_rgb(107, 114, 128)));
                    });
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                if ui.button("Guardar").clicked() {
                    if let Ok(db) = DatabaseManager::new(&self.db_path) {
                        if db.save_config(&self.config_edit).is_ok() {
                            self.current_config = self.config_edit.clone();
                            let _ = self.tx.send(WorkerCommand::ReloadConfig);
                            let _ = db.insert_log("INFO", "CONFIG_SAVE", "Configuracion guardada.", None, None);
                            self.refresh_logs();
                        }
                    }
                }

                if ui.button("Exportar JSON").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).save_file() {
                        if let Ok(json) = self.config_edit.to_json() {
                            let _ = std::fs::write(path, json);
                        }
                    }
                }

                if ui.button("Importar JSON").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                        if let Ok(json) = std::fs::read_to_string(path) {
                            if let Ok(imported_config) = AppConfig::from_json(&json) {
                                self.config_edit = imported_config;
                            }
                        }
                    }
                }
            });
        });
    }


    /// Renderiza la Gestion de Canales Activos
    fn render_channels(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Gestion de Canales Activos").strong().size(14.0));
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Nuevo canal:").size(11.0));
            let input_width = (ui.available_width() - 150.0).max(80.0);
            ui.add(egui::TextEdit::singleline(&mut self.new_canal_input).desired_width(input_width).hint_text("codigo"));
            if ui.button("Agregar").clicked() {
                let code = self.new_canal_input.trim();
                if !code.is_empty() && !self.config_edit.canales.contains(&code.to_string()) {
                    self.config_edit.canales.push(code.to_string());
                    self.new_canal_input.clear();
                    if let Ok(db) = DatabaseManager::new(&self.db_path) {
                        if db.save_config(&self.config_edit).is_ok() {
                            self.current_config = self.config_edit.clone();
                            let _ = self.tx.send(WorkerCommand::ReloadConfig);
                            self.refresh_dates_and_matrix();
                        }
                    }
                }
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.label(egui::RichText::new(format!("Canales activos: {}", self.config_edit.canales.len())).size(10.0).color(egui::Color32::from_rgb(107, 114, 128)));
        ui.add_space(4.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut removed_index = None;
            for (idx, canal) in self.config_edit.canales.iter().enumerate() {
                let row_fill = if idx % 2 == 0 { egui::Color32::from_rgb(255, 255, 255) } else { egui::Color32::from_rgb(249, 250, 251) };
                egui::Frame::new()
                    .fill(row_fill)
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(canal).size(11.0).monospace());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("x").on_hover_text("Eliminar canal").clicked() {
                                    removed_index = Some(idx);
                                }
                            });
                        });
                    });
            }

            if let Some(idx) = removed_index {
                self.config_edit.canales.remove(idx);
                if let Ok(db) = DatabaseManager::new(&self.db_path) {
                    if db.save_config(&self.config_edit).is_ok() {
                        self.current_config = self.config_edit.clone();
                        let _ = self.tx.send(WorkerCommand::ReloadConfig);
                        self.refresh_dates_and_matrix();
                    }
                }
            }
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions::default();
    
    // Instanciar la base de datos local en el directorio de ejecución
    let db_path = PathBuf::from("copiador.db");

    eframe::run_native(
        "Heimdall Copiador",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(CopiadorApp::new(db_path)))
        }),
    )
}