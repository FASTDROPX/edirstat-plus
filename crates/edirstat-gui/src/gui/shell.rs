//! Experimental application shell. The explorer and treemap renderers are unchanged.
use super::{GuiApp, LayoutMode, Locale, VisMode, theme};
use crate::arena::FileArenaSnapshot;
use eframe::egui::{self, Color32, RichText};
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};
use strum::IntoEnumIterator;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    #[default]
    Home,
    Workspace,
    Settings,
}

#[derive(Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct ShellPreferences {
    recent: Vec<String>,
    compact_navigation: bool,
    hide_hints: bool,
    accent: usize,
    side_by_side: bool,
    left_collapsed: bool,
    right_collapsed: bool,
}

pub struct ShellState {
    pub page: Page,
    preferences: ShellPreferences,
    saved: ShellPreferences,
    path_input: String,
    settings_tab: usize,
}

fn config_path() -> Option<PathBuf> {
    #[cfg(not(target_family = "wasm"))]
    {
        directories::ProjectDirs::from("", "", "eDirStat UX Preview")
            .map(|d| d.config_dir().join("shell.toml"))
    }
    #[cfg(target_family = "wasm")]
    {
        None
    }
}

impl ShellState {
    pub fn load() -> Self {
        let preferences: ShellPreferences = config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            page: Page::Home,
            saved: preferences.clone(),
            preferences,
            path_input: String::new(),
            settings_tab: 0,
        }
    }
    pub(super) fn workspace_preferences(&self) -> (LayoutMode, bool, bool) {
        (
            if self.preferences.side_by_side {
                LayoutMode::Classic
            } else {
                LayoutMode::WinDirStat
            },
            self.preferences.left_collapsed,
            self.preferences.right_collapsed,
        )
    }
    pub(super) fn remember_workspace(&mut self, mode: LayoutMode, left: bool, right: bool) {
        self.preferences.side_by_side = mode == LayoutMode::Classic;
        self.preferences.left_collapsed = left;
        self.preferences.right_collapsed = right;
    }
    pub fn remember(&mut self, path: &Path) {
        let clean = crate::arena::clean_unc_path(&path.to_string_lossy()).into_owned();
        self.preferences.recent.retain(|p| p != &clean);
        self.preferences.recent.insert(0, clean);
        self.preferences.recent.truncate(8);
    }
    pub fn save_if_changed(&mut self) {
        if self.preferences == self.saved {
            return;
        }
        if let Some(path) = config_path() {
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, toml::to_string_pretty(&self.preferences)?)?;
                Ok(())
            })();
            if result.is_ok() {
                self.saved = self.preferences.clone();
            }
        }
    }
    fn accent(&self) -> Color32 {
        match self.preferences.accent {
            1 => Color32::from_rgb(36, 132, 106),
            2 => Color32::from_rgb(131, 93, 188),
            _ => Color32::from_rgb(45, 120, 164),
        }
    }
}

fn shell_style(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing = egui::vec2(10., 10.);
    ui.spacing_mut().button_padding = egui::vec2(12., 8.);
    ui.spacing_mut().interact_size.y = 32.;
    ui.style_mut()
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.));
    ui.style_mut()
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.));
    ui.style_mut()
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(24.));
}

fn section(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    contents: impl FnOnce(&mut egui::Ui),
) {
    ui.add_space(12.);
    ui.label(RichText::new(title).size(18.).strong());
    if !description.is_empty() {
        ui.label(RichText::new(description).weak());
    }
    ui.add_space(3.);
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(8)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
}

fn nav(
    ui: &mut egui::Ui,
    text: &str,
    selected: bool,
    width: f32,
    accent: Color32,
    icon: usize,
) -> bool {
    let (r, response) = ui.allocate_exact_size(egui::vec2(width, 38.), egui::Sense::click());
    let fill = if selected {
        accent.gamma_multiply(0.22)
    } else if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(r, 6, fill);
    let fg = if ui.is_enabled() {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let painter = ui.painter().with_clip_rect(r);
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(r.left_top() + egui::vec2(0., 8.), egui::vec2(3., 22.)),
            2,
            accent,
        );
    }
    if response.has_focus() {
        painter.rect_stroke(
            r.shrink(1.),
            6,
            egui::Stroke::new(1., accent),
            egui::StrokeKind::Inside,
        );
    }
    let origin = egui::pos2(
        if width < 80. {
            r.center().x - 9.
        } else {
            r.left() + 12.
        },
        r.center().y - 9.,
    );
    let pos = |x: f32, y: f32| origin + egui::vec2(x, y);
    let stroke = egui::Stroke::new(1.5, fg);
    let line = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([pos(a.0, a.1), pos(b.0, b.1)], stroke);
    };
    let rect = |a: (f32, f32), b: (f32, f32)| {
        painter.rect_stroke(
            egui::Rect::from_min_max(pos(a.0, a.1), pos(b.0, b.1)),
            2,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    match icon {
        0 => {
            line((1., 8.), (9., 2.));
            line((9., 2.), (17., 8.));
            line((4., 7.), (4., 16.));
            line((4., 16.), (14., 16.));
            line((14., 16.), (14., 7.));
            rect((7., 11.), (11., 16.));
        }
        1 => {
            rect((2., 2.), (16., 16.));
            line((2., 9.), (16., 9.));
            line((10., 2.), (10., 16.));
            line((2., 5.), (10., 5.));
        }
        2 | 6 => {
            line((2., 16.), (17., 16.));
            line((4., 12.), (4., 16.));
            line((9., 7.), (9., 16.));
            line((14., 2.), (14., 16.));
        }
        3 => {
            rect((2., 2.), (12., 12.));
            rect((6., 6.), (16., 16.));
        }
        4 => {
            line((2., 5.), (16., 5.));
            rect((5., 2.), (13., 5.));
            rect((4., 5.), (14., 16.));
            line((8., 8.), (8., 13.));
            line((11., 8.), (11., 13.));
        }
        5 => {
            rect((2., 3.), (7., 8.));
            rect((8., 3.), (13., 8.));
            rect((8., 9.), (13., 14.));
            rect((2., 9.), (7., 14.));
            line((1., 16.), (16., 16.));
        }
        7 => {
            painter.circle_stroke(pos(9., 9.), 5., stroke);
            painter.circle_stroke(pos(9., 9.), 1.5, stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.;
                line(
                    (9. + a.cos() * 6., 9. + a.sin() * 6.),
                    (9. + a.cos() * 8., 9. + a.sin() * 8.),
                );
            }
        }
        _ => {
            painter.circle_stroke(pos(9., 9.), 7., stroke);
            line((9., 8.), (9., 13.));
            painter.circle_filled(pos(9., 5.), 1., fg);
        }
    }
    if width >= 80. {
        painter.text(
            r.left_top() + egui::vec2(40., 19.),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(14.),
            fg,
        );
    }
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text));
    let clicked = response.clicked();
    response.on_hover_text(text).on_disabled_hover_text(format!(
        "{text}: сначала просканируйте диск или откройте снимок."
    ));
    clicked
}

impl GuiApp {
    pub(crate) fn render_shell_top(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &FileArenaSnapshot,
        scanning: bool,
    ) {
        let accent = self.shell.accent();
        egui::Panel::top("ux_top").exact_size(64.).show(ui, |ui| {
            ui.scope(|ui| {
                shell_style(ui);
                ui.horizontal_centered(|ui| {
                    let wide = ui.available_width() > 1000.;
                    if let Some(prefix) = &mut self.top_panel_prefix {
                        prefix(ui);
                    }
                    if ui.button("☰").on_hover_text("Свернуть навигацию").clicked()
                    {
                        self.shell.preferences.compact_navigation ^= true;
                    }
                    ui.add(
                        egui::Image::new(egui::include_image!(
                            "../../assets/img/icon-transparent.png"
                        ))
                        .max_height(27.),
                    );
                    ui.label(RichText::new("eDirStat Plus").size(20.).strong());
                    if wide {
                        ui.label(RichText::new("UX Preview").size(11.).color(accent));
                    }
                    ui.separator();
                    ui.menu_button("Файл", |ui| self.draw_file_menu_contents(ui, snapshot));
                    let title = match self.shell.page {
                        Page::Home => "Главная",
                        Page::Settings => "Настройки",
                        Page::Workspace => match self.vis_mode {
                            VisMode::Treemap => "Обзор диска",
                            VisMode::Plots => "Графики",
                            VisMode::Deduplicator => "Дубликаты",
                            VisMode::Cleanup => "Очистка",
                            VisMode::Docker => "Docker",
                        },
                    };
                    if wide {
                        ui.label(RichText::new(title).strong());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !scanning && self.scanner.is_some(),
                                egui::Button::new(
                                    RichText::new("+ Новый анализ").color(Color32::WHITE),
                                )
                                .fill(accent),
                            )
                            .clicked()
                        {
                            self.open_scan_modal();
                        }
                        if wide && self.shell.page == Page::Workspace {
                            if ui
                                .add_enabled(
                                    !snapshot.nodes.is_empty(),
                                    egui::Button::new("Сохранить снимок"),
                                )
                                .clicked()
                            {
                                self.prompt_save_snapshot(snapshot);
                            }
                            if ui
                                .add_enabled(
                                    !scanning && self.current_scan_path.is_some(),
                                    egui::Button::new("Обновить"),
                                )
                                .on_hover_text("Повторное сканирование · F5")
                                .clicked()
                            {
                                self.rescan_current_root();
                            }
                        }
                    });
                });
            });
        });
        if ui
            .ctx()
            .input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma))
        {
            self.shell.page = Page::Settings;
        }
    }

    pub(crate) fn render_shell_navigation(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &FileArenaSnapshot,
    ) {
        let compact =
            self.shell.preferences.compact_navigation || ui.ctx().content_rect().width() < 1050.;
        let width = if compact { 58. } else { 178. };
        let accent = self.shell.accent();
        egui::Panel::left("ux_navigation")
            .exact_size(width)
            .resizable(false)
            .show(ui, |ui| {
                ui.scope(|ui| {
                    shell_style(ui);
                    ui.add_space(12.);
                    let w = ui.available_width();
                    if nav(ui, "Главная", self.shell.page == Page::Home, w, accent, 0) {
                        self.shell.page = Page::Home;
                    }
                    ui.add_space(14.);
                    if !compact {
                        ui.label(RichText::new("Рабочее пространство").size(11.).weak());
                    }
                    for (mode, label, icon) in [
                        (VisMode::Treemap, "Обзор диска", 1),
                        (VisMode::Plots, "Графики", 2),
                        (VisMode::Deduplicator, "Дубликаты", 3),
                        (VisMode::Cleanup, "Очистка", 4),
                        (VisMode::Docker, "Docker", 5),
                    ] {
                        let available = !snapshot.nodes.is_empty()
                            || matches!(mode, VisMode::Docker | VisMode::Cleanup);
                        let clicked = ui
                            .add_enabled_ui(available, |ui| {
                                nav(
                                    ui,
                                    label,
                                    self.shell.page == Page::Workspace && self.vis_mode == mode,
                                    w,
                                    accent,
                                    icon,
                                )
                            })
                            .inner;
                        if clicked {
                            self.vis_mode = mode;
                            self.shell.page = Page::Workspace;
                        }
                    }
                    if !snapshot.nodes.is_empty() && nav(ui, "Статистика", false, w, accent, 6)
                    {
                        self.disk_stats.open = true;
                    }
                    ui.add_space((ui.available_height() - 132.).max(16.));
                    ui.separator();
                    if nav(
                        ui,
                        "Настройки",
                        self.shell.page == Page::Settings,
                        w,
                        accent,
                        7,
                    ) {
                        self.shell.page = Page::Settings;
                    }
                    if nav(ui, "О программе", false, w, accent, 8) {
                        self.active_modal = Some(super::ActiveModal::About);
                    }
                    if !compact {
                        ui.label(RichText::new("Тестовая оболочка · v3").size(11.).weak());
                    }
                });
            });
    }

    pub(crate) fn render_shell_status(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &FileArenaSnapshot,
        scanning: bool,
    ) {
        egui::Panel::bottom("ux_status")
            .exact_size(if scanning { 58. } else { 32. })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if scanning {
                        ui.spinner();
                    }
                    let cache = self.shared_state.cache_status.load(Ordering::Relaxed);
                    let label = if scanning {
                        match cache {
                            1 => "Открываем кэш…",
                            2 => "Кэш открыт. Проверяем изменения…",
                            _ => "Сканирование…",
                        }
                    } else if cache == 3 {
                        "Кэш показан. Проверка не завершена"
                    } else if self.shared_state.scan_cancel.load(Ordering::Relaxed) {
                        "Сканирование остановлено"
                    } else if snapshot.nodes.is_empty() {
                        "Готов к анализу"
                    } else {
                        "Анализ завершён"
                    };
                    ui.label(label);
                    if scanning && ui.button("Остановить").clicked() {
                        self.shared_state.scan_cancel.store(true, Ordering::SeqCst);
                    }
                    if !snapshot.nodes.is_empty() {
                        ui.separator();
                        let size = prettier_bytes::ByteFormatter::new()
                            .format(snapshot.nodes[0].size)
                            .to_string();
                        ui.strong(size);
                        let files = self
                            .shared_state
                            .scan_stats
                            .files_scanned
                            .load(Ordering::Relaxed);
                        let dirs = self
                            .shared_state
                            .scan_stats
                            .dirs_scanned
                            .load(Ordering::Relaxed);
                        ui.label(format!("{files} файлов  /  {dirs} папок"));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(path) = &self.current_scan_path {
                            let path =
                                crate::arena::clean_unc_path(&path.to_string_lossy()).into_owned();
                            if ui.small_button("Копировать путь").clicked() {
                                ui.ctx().copy_text(path.clone());
                            }
                            ui.add(egui::Label::new(RichText::new(&path).weak()).truncate())
                                .on_hover_text(path);
                        }
                    });
                });
                if scanning {
                    let stats = &self.shared_state.scan_stats;
                    if stats.reading_mft.load(Ordering::Relaxed) {
                        let total = stats.mft_total_records.load(Ordering::Relaxed);
                        let count = stats.mft_records.load(Ordering::Relaxed);
                        ui.add(
                            egui::ProgressBar::new(count as f32 / total.max(1) as f32)
                                .text(format!("Чтение MFT: {count} / {total} записей"))
                                .desired_width(ui.available_width()),
                        );
                    } else {
                        ui.label(
                            RichText::new(
                                "Данные появляются по мере обработки. Можно продолжать работать.",
                            )
                            .size(12.)
                            .weak(),
                        );
                    }
                }
            });
    }

    fn ux_open_snapshot(&mut self) {
        #[cfg(not(target_family = "wasm"))]
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Снимок eDirStat", &["edst", "zst"])
            .pick_file()
            && let Err(e) = self.load_snapshot_file(path)
        {
            super::toast_error(format!("Не удалось открыть снимок: {e}"));
        }
    }

    pub(crate) fn render_shell_loading(&mut self, ui: &mut egui::Ui, scanning: bool) {
        ui.scope(|ui| {
            shell_style(ui);
            egui::Frame::new().inner_margin(32).show(ui,|ui| {
                ui.add_space((ui.available_height()*0.18).min(120.));
                if scanning {
                    ui.horizontal(|ui| {ui.add(egui::Spinner::new().size(28.));ui.heading("Подготавливаем обзор диска");});
                    if let Some(path)=&self.current_scan_path {ui.label(crate::arena::clean_unc_path(&path.to_string_lossy()).into_owned());}
                    ui.add_space(12.);
                    let stats=&self.shared_state.scan_stats;
                    if stats.reading_mft.load(Ordering::Relaxed) {
                        let size=prettier_bytes::ByteFormatter::new().format(stats.metadata_bytes.load(Ordering::Relaxed) as u64).to_string();
                        ui.label(format!("Читаем индекс файловой системы. Обработано {size} метаданных."));
                    } else {ui.label("Собираем сведения о папках и файлах…");}
                    ui.label(RichText::new("Карта появится, как только будут готовы первые данные.").weak());
                    if !self.shared_state.live_updates.load(Ordering::Relaxed) {ui.label("В настройках отключены промежуточные обновления: дерево появится после завершения.");}
                    ui.add_space(16.);
                    if ui.button("На главную").clicked() {self.shell.page=Page::Home;}
                } else {
                    ui.heading("Начните с выбора диска или папки");
                    ui.label("Можно также открыть сохранённый снимок, не сканируя диск заново.");
                    if ui.button("Открыть главную").clicked() {self.shell.page=Page::Home;}
                }
            });
        });
    }

    pub(crate) fn render_shell_home(&mut self, ui: &mut egui::Ui, snapshot: &FileArenaSnapshot) {
        ui.scope(|ui| {
            shell_style(ui);
            egui::ScrollArea::vertical().id_salt("ux_home_scroll").show(ui,|ui| {
                egui::Frame::new().inner_margin(24).show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(1100.));
                    ui.add_space(12.);
                    ui.label(RichText::new("Что занимает место?").size(34.).strong());
                    ui.label(RichText::new("Выберите диск или папку. Найдите крупные файлы и решите, что оставить.").size(16.).weak());
                    ui.add_space(18.);
                    let scanning=self.shared_state.is_scanning.load(Ordering::Relaxed);
                    if !snapshot.nodes.is_empty() {
                        egui::Frame::new().fill(self.shell.accent().gamma_multiply(0.16)).corner_radius(8).inner_margin(16).show(ui,|ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(if scanning {"Анализ уже идёт"} else {"Текущий анализ доступен"}).strong());
                                if let Some(path)=&self.current_scan_path { ui.label(crate::arena::clean_unc_path(&path.to_string_lossy()).into_owned()); }
                                if ui.button("Продолжить просмотр").clicked() { self.shell.page=Page::Workspace; self.vis_mode=VisMode::Treemap; }
                            });
                        });
                    }
                    section(ui,"Начать анализ","Диск целиком, отдельная папка или ранее сохранённый снимок.",|ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(!scanning && self.scanner.is_some(),egui::Button::new("Выбрать папку…")).clicked() {
                                #[cfg(not(target_family="wasm"))]
                                if let Some(path)=rfd::FileDialog::new().pick_folder() { self.start_scan(path); }
                            }
                            if ui.button("Открыть снимок…").clicked() { self.ux_open_snapshot(); }
                            if ui.add_enabled(!scanning && self.scanner.is_some(),egui::Button::new("Все варианты сканирования…")).clicked() { self.open_scan_modal(); }
                        });
                        ui.add_space(4.);
                        ui.horizontal(|ui| {
                            let response=ui.add(egui::TextEdit::singleline(&mut self.shell.path_input).hint_text("Путь к папке, например C:\\Users").desired_width((ui.available_width()-130.).max(120.)));
                            let enter=response.lost_focus() && ui.input(|i|i.key_pressed(egui::Key::Enter));
                            if ui.add_enabled(!scanning && self.scanner.is_some() && !self.shell.path_input.trim().is_empty(),egui::Button::new("Сканировать")).clicked() || (enter && !scanning && self.scanner.is_some()) {
                                let path=PathBuf::from(self.shell.path_input.trim());
                                if path.is_dir() {self.start_scan(path);} else { super::toast_error("Папка не найдена. Проверьте путь."); }
                            }
                        });
                    });
                    section(ui,"Диски","Нажмите на диск, чтобы просканировать его.",|ui| {
                        let drives=super::modals::get_detected_drives(false);
                        if drives.is_empty() { ui.label("Список дисков недоступен. Выберите папку или введите путь выше."); }
                        for drive in drives {
                            ui.push_id(&drive.mount_point,|ui| {
                                ui.horizontal(|ui| {
                                    let title=format!("{}  {}",drive.mount_point.display(),drive.name);
                                    if ui.add_enabled(!scanning && self.scanner.is_some(),egui::Button::new(RichText::new(title).strong()).min_size(egui::vec2(180.,44.))).clicked() { self.start_scan(drive.mount_point.clone()); }
                                    ui.vertical(|ui| {
                                        let total=prettier_bytes::ByteFormatter::new().format(drive.total_bytes).to_string();
                                        let free=prettier_bytes::ByteFormatter::new().format(drive.available_bytes).to_string();
                                        ui.label(format!("{free} свободно из {total}   •   {}",drive.fs_type));
                                        let used=1.-drive.available_bytes as f32/drive.total_bytes.max(1) as f32;
                                        ui.add(egui::ProgressBar::new(used).fill(self.shell.accent()).desired_width(ui.available_width().min(500.)).desired_height(5.));
                                    });
                                });
                            });
                        }
                    });
                    section(ui,"Недавние папки","Повторное открытие использует кэш, если для этого пути есть сохранённые данные.",|ui| {
                        if self.shell.preferences.recent.is_empty() { ui.label(RichText::new("Здесь появятся папки, которые вы просканируете.").weak()); }
                        let recent=self.shell.preferences.recent.clone();
                        for path in recent {
                            ui.horizontal(|ui| {
                                if ui.add_enabled(!scanning && self.scanner.is_some(),egui::Button::new(&path).min_size(egui::vec2((ui.available_width()-48.).max(100.),32.))).clicked() {
                                    if Path::new(&path).is_dir() { self.start_scan(PathBuf::from(&path)); } else { super::toast_error("Путь недоступен. Возможно, диск отключён."); }
                                }
                                if ui.small_button("×").on_hover_text("Убрать из недавних").clicked() {self.shell.preferences.recent.retain(|p|p!=&path);}
                            });
                        }
                    });
                    if !self.shell.preferences.hide_hints {
                        ui.add_space(18.);
                        ui.label(RichText::new("Ctrl + колесо — зум   /   двойной клик — открыть папку   /   перетаскивание — перемещение").size(12.).weak());
                    }
                });
            });
        });
    }

    pub(crate) fn render_shell_settings(&mut self, ui: &mut egui::Ui) {
        ui.scope(|ui| {
            shell_style(ui);
            egui::Frame::new().inner_margin(24).show(ui,|ui| {
                ui.heading("Настройки");
                ui.label(RichText::new("Изменения сохраняются автоматически. У этой тестовой сборки отдельный профиль.").weak());
                ui.horizontal_wrapped(|ui| {
                    for (i,label) in ["Оформление","Рабочее пространство","Сканирование и файлы","Клавиатура"].iter().enumerate() {
                        ui.selectable_value(&mut self.shell.settings_tab,i,*label);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().id_salt("ux_settings_scroll").show(ui,|ui| {
                    ui.set_max_width(ui.available_width().min(1000.));
                    match self.shell.settings_tab {
                        0 => {
                            section(ui,"Тема интерфейса","Оформление оболочки. Палитра и стиль плиток выбираются отдельно на карте.",|ui| {
                                ui.horizontal_wrapped(|ui| {
                                    for (pref,label,bg,fg) in [
                                        (theme::ThemePreference::System,"Системная",Color32::from_rgb(82,95,110),Color32::WHITE),
                                        (theme::ThemePreference::Dark,"Тёмная",Color32::from_rgb(26,29,38),Color32::from_rgb(226,233,240)),
                                        (theme::ThemePreference::Light,"Светлая",Color32::from_rgb(244,246,249),Color32::from_rgb(30,43,59)),
                                        (theme::ThemePreference::HighContrast,"Контрастная",Color32::BLACK,Color32::YELLOW)] {
                                        let selected=self.theme==pref;
                                        if ui.add(egui::Button::new(RichText::new(format!("{}\n{label}","Aa")).color(fg).size(16.)).fill(bg).min_size(egui::vec2(150.,88.)).stroke(egui::Stroke::new(if selected {2.}else{1.},if selected {self.shell.accent()}else{ui.visuals().widgets.noninteractive.bg_stroke.color}))).clicked() {self.theme=pref;theme::clear_color_cache();}
                                    }
                                });
                            });
                            section(ui,"Цвет акцента","Для навигации, кнопок и индикаторов оболочки.",|ui| {
                                ui.horizontal_wrapped(|ui| { for (i,label) in ["Синий","Зелёный","Фиолетовый"].iter().enumerate() {ui.selectable_value(&mut self.shell.preferences.accent,i,*label);} });
                            });
                            section(ui,"Язык интерфейса","Часть экспериментальных экранов пока доступна только на русском.",|ui| {
                                egui::ComboBox::from_id_salt("ux_locale").selected_text(self.locale.as_str()).show_ui(ui,|ui| { for locale in Locale::iter() {if ui.selectable_label(self.locale==locale,locale.as_str()).clicked() {locale.apply();self.locale=locale;self.locale_preference=Some(locale);}} });
                            });
                        }
                        1 => {
                            section(ui,"Навигация","Главная и настройки доступны, не закрывая текущий анализ.",|ui| {
                                ui.checkbox(&mut self.shell.preferences.compact_navigation,"Компактная боковая панель");
                                ui.checkbox(&mut self.shell.preferences.hide_hints,"Скрыть подсказки на главном экране");
                            });
                            section(ui,"Расположение панелей","Меняет расположение панелей вокруг карты.",|ui| {
                                ui.radio_value(&mut self.layout_mode,LayoutMode::WinDirStat,"Таблица сверху, карта снизу");
                                ui.radio_value(&mut self.layout_mode,LayoutMode::Classic,"Дерево слева, карта справа");
                                ui.checkbox(&mut self.left_panel_collapsed,"Скрыть левую панель в боковом расположении");
                                ui.checkbox(&mut self.right_panel_collapsed,"Скрыть список расширений");
                                ui.checkbox(&mut self.monospace_paths,"Моноширинные пути");
                                ui.checkbox(&mut self.highlight_duplicates,"Подсвечивать дубликаты");
                            });
                            section(ui, "Стиль карты", "Существующие параметры карты; оформление оболочки от них не зависит.", |ui| {
                                use crate::stats::treemap::TreemapStyle as S;
                                egui::ComboBox::from_id_salt("ux_map_style").selected_text(match self.treemap_style {S::VerticalGradient=>"Вертикальный градиент",S::OffsetVerticalGradient=>"Смещённый градиент",S::DiagonalGradient=>"Диагональный градиент",S::Cushion=>"Рельеф",S::Flat=>"Плоский",S::Bevel=>"Объёмные плитки",S::Glass=>"Стекло",S::Neon=>"Неон",S::Outline=>"Контуры"}).show_ui(ui, |ui| {
                                    for (style,label) in [(S::VerticalGradient,"Вертикальный градиент"),(S::OffsetVerticalGradient,"Смещённый градиент"),(S::DiagonalGradient,"Диагональный градиент"),(S::Cushion,"Рельеф"),(S::Flat,"Плоский"),(S::Bevel,"Объёмные плитки"),(S::Glass,"Стекло"),(S::Neon,"Неон"),(S::Outline,"Контуры")] { ui.selectable_value(&mut self.treemap_style,style,label); }
                                });
                                ui.checkbox(&mut self.treemap_borders,"Обводки в режиме по папкам");
                            });
                            section(ui,"Дата и время","Формат в таблице файлов и сведениях об объектах.",|ui| {
                                egui::ComboBox::from_id_salt("ux_time").selected_text(&self.time_format.0).show_ui(ui,|ui| { for format in crate::time_utils::CommonTimeFormat::ALL {ui.selectable_value(&mut self.time_format.0,format.as_str().to_string(),format.label());} });
                            });
                            section(ui,"Недавние пути","Удаляет только список в этой оболочке. Снимки кэша остаются на диске.",|ui| {
                                if ui.button("Очистить список недавних").clicked() {self.shell.preferences.recent.clear();}
                            });
                        }
                        2 => {
                            section(ui,"Кэш диска","Снимок сохраняется после успешного сканирования и автоматически открывается при следующем запуске.",|ui| {
                                let mut enabled=self.shared_state.cache_enabled.load(Ordering::Acquire);
                                if ui.checkbox(&mut enabled,"Использовать кэш и открывать последний анализ при запуске").changed() {
                                    self.shared_state.cache_enabled.store(enabled,Ordering::Release);
                                }
                                ui.label(RichText::new("Включено по умолчанию. После открытия снимка изменения проверяются в фоне.").weak());
                                ui.label(RichText::new("При отключении кэш не читается и новые снимки не сохраняются. Уже открытый анализ и старые снимки остаются.").weak());
                            });
                            section(ui,"Во время сканирования","Кэш открывается перед проверкой актуальности данных.",|ui| {
                                let mut live=self.shared_state.live_updates.load(Ordering::Relaxed);
                                if ui.checkbox(&mut live,"Показывать дерево по мере загрузки").changed() {self.shared_state.live_updates.store(live,Ordering::Relaxed);}
                                ui.label(RichText::new("Если выключено, итоговое дерево появится после завершения сканирования.").weak());
                                #[cfg(target_os="windows")]
                                ui.checkbox(&mut self.auto_elevate,"Запрашивать права администратора при запуске");
                            });
                            section(ui,"Подтверждения","Дополнительная проверка перед операциями с файлами.",|ui| {
                                ui.checkbox(&mut self.deletion_confirmation,"Подтверждать безвозвратное удаление");
                                ui.checkbox(&mut self.trash_confirmation,"Подтверждать перемещение в корзину");
                            });
                        }
                        _ => {
                            section(ui,"Быстрые действия","Работают в рабочем пространстве и через меню «Файл».",|ui| {
                                egui::Grid::new("ux_shortcuts").spacing([36.,14.]).show(ui,|ui| {
                                    for (key,label) in [("Ctrl + O","Новый анализ"),("F5 / Ctrl + R","Повторить сканирование"),("Ctrl + S","Сохранить снимок"),("Ctrl + F","Поиск в таблице"),("Ctrl + ,","Открыть настройки"),("Ctrl + колесо","Приблизить или отдалить карту"),("Двойной клик","Открыть папку на карте"),("Перетаскивание","Переместить карту")] { ui.label(RichText::new(key).strong()); ui.label(label); ui.end_row(); }
                                });
                            });
                        }
                    }
                });
            });
        });
    }
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
