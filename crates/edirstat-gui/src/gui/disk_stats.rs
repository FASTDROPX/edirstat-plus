//! On-demand snapshot analytics. Expensive traversal runs away from the UI thread.
use crate::arena::{FileArenaSnapshot, NO_INDEX, get_ext_slice};
use eframe::egui;
use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
pub struct DiskStatsWindow {
    pub open: bool,
    query: String,
    section: String,
    report: Option<Report>,
    pending: Option<mpsc::Receiver<Report>>,
    error: Option<String>,
}

struct Metric {
    section: &'static str,
    name: String,
    value: String,
}
struct Report {
    source: Arc<FileArenaSnapshot>,
    rows: Vec<Metric>,
}

impl DiskStatsWindow {
    fn start(&mut self, snapshot: &Arc<FileArenaSnapshot>, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        let source = Arc::clone(snapshot);
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = None;
        let ctx = ctx.clone();
        #[cfg(not(target_family = "wasm"))]
        std::thread::spawn(move || {
            let report = analyze(source);
            let _ = tx.send(report);
            ctx.request_repaint();
        });
        #[cfg(target_family = "wasm")]
        {
            let _ = tx.send(analyze(source));
            ctx.request_repaint();
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, snapshot: &Arc<FileArenaSnapshot>) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(report) => {
                    self.report = Some(report);
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.error = Some("Расчёт прерван. Нажмите «Пересчитать».".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.report.is_none()
            && self.pending.is_none()
            && self.error.is_none()
            && !snapshot.nodes.is_empty()
        {
            self.start(snapshot, ctx);
        }
        let mut open = self.open;
        egui::Window::new("Статистика диска и файлов")
            .open(&mut open).default_size([940.0, 700.0]).min_size([520.0, 340.0])
            .show(ctx, |ui| {
                ui.label("Данные относятся ко всему снимку сканирования, а не к выбранной папке. Размеры — сохранённые сканером байты; это не полная занятость тома.");
                ui.label("SMART, температура, фрагментация и точный объём дубликатов не определяются по снимку. Неизвестные даты исключены из возрастных интервалов.");
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(self.pending.is_none() && !snapshot.nodes.is_empty(), egui::Button::new("Пересчитать")).clicked() {
                        self.start(snapshot, ctx);
                    }
                    if let Some(report) = &self.report {
                        ui.label(format!("{} показателей", report.rows.len()));
                        if ui.button("Копировать отчёт (TSV)").clicked() {
                            let mut text = String::from("Раздел\tПоказатель\tЗначение\n");
                            for row in &report.rows {
                                text.push_str(&format!("{}\t{}\t{}\n", row.section, row.name.replace(['\t','\n','\r'], " "), row.value.replace(['\t','\n','\r'], " ")));
                            }
                            ctx.copy_text(text);
                        }
                    }
                });
                if self.pending.is_some() { ui.horizontal(|ui| { ui.spinner(); ui.label("Рассчитываем статистику… интерфейс доступен"); }); }
                if let Some(error) = &self.error { ui.colored_label(ui.visuals().error_fg_color, error); }
                let Some(report) = &self.report else {
                    if snapshot.nodes.is_empty() { ui.label("Сначала отсканируйте диск или откройте снимок."); }
                    return;
                };
                if !Arc::ptr_eq(&report.source.nodes, &snapshot.nodes) {
                    ui.colored_label(ui.visuals().warn_fg_color, "Снимок изменился. Нажмите «Пересчитать», чтобы обновить показатели.");
                }
                ui.label(crate::arena::clean_unc_path(&report.source.get_full_path(0)).as_ref());
                ui.horizontal_wrapped(|ui| {
                    ui.label("Поиск:");
                    ui.text_edit_singleline(&mut self.query);
                    egui::ComboBox::from_id_salt("disk_stats_section")
                        .selected_text(if self.section.is_empty() { "Все разделы" } else { &self.section })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.section, String::new(), "Все разделы");
                            for section in ["Обзор", "Размеры", "Возраст", "Глубина", "Типы файлов", "Имена", "Крупнейшие файлы", "Папки", "Том (сейчас)"] {
                                ui.selectable_value(&mut self.section, section.to_owned(), section);
                            }
                        });
                });
                ui.separator();
                let query = self.query.to_lowercase();
                let filtered: Vec<_> = report.rows.iter().filter(|row| {
                    (self.section.is_empty() || row.section == self.section) &&
                    (query.is_empty() || format!("{} {} {}", row.section, row.name, row.value).to_lowercase().contains(&query))
                }).collect();
                ui.label(format!("Показано: {}", filtered.len()));
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    egui::Grid::new("disk_stats_grid").striped(true).spacing([22.0, 7.0]).show(ui, |ui| {
                        ui.strong("Раздел"); ui.strong("Показатель"); ui.strong("Значение"); ui.end_row();
                        for row in filtered {
                            ui.weak(row.section); ui.label(&row.name);
                            ui.add(egui::Label::new(&row.value).selectable(true)); ui.end_row();
                        }
                    });
                });
            });
        self.open = open;
    }
}

fn bytes(value: u64) -> String {
    format!(
        "{} ({} Б)",
        prettier_bytes::ByteFormatter::new().format(value),
        value
    )
}
fn ratio(n: u64, d: u64) -> String {
    if d == 0 {
        "Нет данных".into()
    } else {
        format!("{:.2}%", n as f64 * 100.0 / d as f64)
    }
}
fn average(sum: u64, n: u64) -> String {
    if n == 0 {
        "Нет данных".into()
    } else {
        format!("{:.2}", sum as f64 / n as f64)
    }
}
fn add(
    rows: &mut Vec<Metric>,
    section: &'static str,
    name: impl Into<String>,
    value: impl ToString,
) {
    rows.push(Metric {
        section,
        name: name.into(),
        value: value.to_string(),
    });
}
fn distribution(
    rows: &mut Vec<Metric>,
    section: &'static str,
    name: &str,
    count: u64,
    size: u64,
    files: u64,
    total: u64,
) {
    add(rows, section, format!("{name} · файлов"), count);
    add(rows, section, format!("{name} · объём"), bytes(size));
    add(
        rows,
        section,
        format!("{name} · доля файлов"),
        ratio(count, files),
    );
    add(
        rows,
        section,
        format!("{name} · доля объёма"),
        ratio(size, total),
    );
}

fn analyze(source: Arc<FileArenaSnapshot>) -> Report {
    let mut rows = Vec::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut files: Vec<(u64, u32)> = Vec::new();
    let mut dirs: Vec<(u64, u32)> = Vec::new();
    let mut extensions: HashMap<String, (u64, u64)> = HashMap::new();
    let size_limits = [
        0,
        1024,
        10 * 1024,
        100 * 1024,
        1024 * 1024,
        10 * 1024 * 1024,
        100 * 1024 * 1024,
        1024 * 1024 * 1024,
        10 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    ];
    let size_labels = [
        "0 Б",
        "1 Б – <1 КиБ",
        "1 – <10 КиБ",
        "10 – <100 КиБ",
        "100 КиБ – <1 МиБ",
        "1 – <10 МиБ",
        "10 – <100 МиБ",
        "100 МиБ – <1 ГиБ",
        "1 – <10 ГиБ",
        "10 – <100 ГиБ",
        "≥100 ГиБ",
    ];
    let mut sizes = [(0u64, 0u64); 11];
    let age_limits = [1, 7, 30, 90, 180, 365, 730, 1825, 3650];
    let age_labels = [
        "<1 дня",
        "1 – <7 дней",
        "7 – <30 дней",
        "30 – <90 дней",
        "90 – <180 дней",
        "180 – <365 дней",
        "1 – <2 лет",
        "2 – <5 лет",
        "5 – <10 лет",
        "≥10 лет",
    ];
    let mut ages = [(0u64, 0u64); 10];
    let mut depths = [(0u64, 0u64); 12];
    let mut visited = vec![false; source.nodes.len()];
    let mut stack = Vec::new();
    if !source.nodes.is_empty() {
        stack.push((0u32, 0usize, 0usize));
        visited[0] = true;
    }
    let mut total = 0u64;
    let mut depth_sum = 0u64;
    let mut max_depth = 0usize;
    let mut deepest = 0u32;
    let mut flags = [0u64; 5];
    let mut empty_dirs = 0u64;
    let mut unknown_modified = 0u64;
    let mut unknown_created = 0u64;
    let mut future_modified = 0u64;
    let mut oldest_modified: Option<u32> = None;
    let mut newest_modified = 0u32;
    let mut name_counts = [0u64; 6];
    let mut name_chars = 0u64;
    let mut longest_name = (0usize, 0u32);
    let mut longest_path = (0usize, 0u32);
    let mut max_children = (0u64, 0u32);
    let mut reachable = 0u64;
    while let Some((idx, depth, parent_chars)) = stack.pop() {
        let node = &source.nodes[idx as usize];
        reachable += 1;
        let name = source.string_pool.get(node.name_id).unwrap_or("");
        let chars = name.chars().count();
        let path_chars = parent_chars + chars + usize::from(depth > 0);
        if depth > max_depth {
            max_depth = depth;
            deepest = idx;
        }
        if path_chars > longest_path.0 {
            longest_path = (path_chars, idx);
        }
        for (i, yes) in [
            node.is_symlink(),
            node.has_no_permission(),
            node.is_dataless(),
            node.is_special(),
            node.size == 0,
        ]
        .into_iter()
        .enumerate()
        {
            flags[i] += u64::from(yes);
        }
        if node.is_directory() {
            dirs.push((node.size, idx));
            if node.first_child == NO_INDEX {
                empty_dirs += 1;
            }
            let mut child = node.first_child;
            let mut children = 0;
            for _ in 0..source.nodes.len() {
                let Some(entry) = source.nodes.get(child as usize) else {
                    break;
                };
                if visited[child as usize] {
                    break;
                }
                visited[child as usize] = true;
                stack.push((child, depth + 1, path_chars));
                children += 1;
                child = entry.next_sibling;
            }
            if children > max_children.0 {
                max_children = (children, idx);
            }
            continue;
        }
        files.push((node.size, idx));
        total = total.saturating_add(node.size);
        depth_sum += depth as u64;
        let bucket = size_limits
            .iter()
            .filter(|&&limit| node.size >= limit && (limit > 0 || node.size > 0))
            .count();
        sizes[bucket].0 += 1;
        sizes[bucket].1 = sizes[bucket].1.saturating_add(node.size);
        let d = depth.min(12).saturating_sub(1);
        depths[d].0 += 1;
        depths[d].1 = depths[d].1.saturating_add(node.size);
        if node.modified_timestamp == 0 {
            unknown_modified += 1;
        } else if u64::from(node.modified_timestamp) > now {
            future_modified += 1;
        } else {
            let age = (now - u64::from(node.modified_timestamp)) / 86400;
            let bucket = age_limits.iter().filter(|&&limit| age >= limit).count();
            ages[bucket].0 += 1;
            ages[bucket].1 = ages[bucket].1.saturating_add(node.size);
        }
        if node.modified_timestamp > 0 {
            oldest_modified = Some(
                oldest_modified.map_or(node.modified_timestamp, |v| v.min(node.modified_timestamp)),
            );
            newest_modified = newest_modified.max(node.modified_timestamp);
        }
        unknown_created += u64::from(node.created_timestamp == 0);
        name_chars += chars as u64;
        if chars > longest_name.0 {
            longest_name = (chars, idx);
        }
        let ext = get_ext_slice(name).to_lowercase();
        for (i, yes) in [
            !name.is_ascii(),
            name.contains(' '),
            name.starts_with('.'),
            ext.is_empty() || ext == crate::arena::NO_EXTENSION,
            chars > 100,
            path_chars > 260,
        ]
        .into_iter()
        .enumerate()
        {
            name_counts[i] += u64::from(yes);
        }
        let entry = extensions.entry(ext).or_default();
        entry.0 += 1;
        entry.1 = entry.1.saturating_add(node.size);
    }
    files.sort_unstable();
    dirs.sort_unstable();
    let count = files.len() as u64;
    let path = |idx: u32| crate::arena::clean_unc_path(&source.get_full_path(idx)).into_owned();
    add(&mut rows, "Обзор", "Файлов", count);
    if let Some(&(size, idx)) = files.last() {
        add(&mut rows, "Обзор", "Самый большой файл", path(idx));
        add(
            &mut rows,
            "Обзор",
            "Размер самого большого файла",
            bytes(size),
        );
    }
    add(&mut rows, "Обзор", "Папок (включая корень)", dirs.len());
    add(&mut rows, "Обзор", "Доступных записей дерева", reachable);
    add(&mut rows, "Обзор", "Суммарный размер файлов", bytes(total));
    add(
        &mut rows,
        "Обзор",
        "Размер корня по снимку",
        bytes(source.nodes.first().map_or(0, |n| n.size)),
    );
    add(
        &mut rows,
        "Обзор",
        "Средний размер файла, Б",
        average(total, count),
    );
    add(
        &mut rows,
        "Обзор",
        "Максимальная вложенность (корень = 0)",
        max_depth,
    );
    add(
        &mut rows,
        "Обзор",
        "Самый глубокий объект",
        if reachable == 0 {
            "Нет данных".into()
        } else {
            path(deepest)
        },
    );
    add(
        &mut rows,
        "Обзор",
        "Средняя глубина файлов",
        average(depth_sum, count),
    );
    add(&mut rows, "Обзор", "Пустых папок по дереву", empty_dirs);
    add(
        &mut rows,
        "Обзор",
        "Уникальных расширений (включая отсутствие)",
        extensions.len(),
    );
    for (i, label) in [
        "Символических ссылок",
        "Записей без разрешения",
        "Облачных заглушек",
        "Специальных записей",
        "Записей нулевого размера",
    ]
    .iter()
    .enumerate()
    {
        add(&mut rows, "Обзор", *label, flags[i]);
    }
    add(
        &mut rows,
        "Обзор",
        "Файлов с неизвестной датой изменения",
        unknown_modified,
    );
    add(
        &mut rows,
        "Обзор",
        "Файлов с неизвестной датой создания",
        unknown_created,
    );
    add(
        &mut rows,
        "Обзор",
        "Файлов с будущей датой изменения",
        future_modified,
    );
    add(
        &mut rows,
        "Обзор",
        "Самая ранняя дата изменения, Unix s",
        oldest_modified.map_or("Нет данных".into(), |v| v.to_string()),
    );
    add(
        &mut rows,
        "Обзор",
        "Самая поздняя дата изменения, Unix s",
        if newest_modified == 0 {
            "Нет данных".into()
        } else {
            newest_modified.to_string()
        },
    );
    for p in [0, 10, 25, 50, 75, 90, 95, 99, 100] {
        let value = if files.is_empty() {
            "Нет данных".into()
        } else {
            bytes(files[(files.len() - 1) * p / 100].0)
        };
        add(
            &mut rows,
            "Размеры",
            format!("Перцентиль P{p} (нижний ранг)"),
            value,
        );
    }
    for (i, label) in size_labels.iter().enumerate() {
        distribution(
            &mut rows,
            "Размеры",
            label,
            sizes[i].0,
            sizes[i].1,
            count,
            total,
        );
    }
    for (i, label) in age_labels.iter().enumerate() {
        distribution(
            &mut rows,
            "Возраст",
            label,
            ages[i].0,
            ages[i].1,
            count,
            total,
        );
    }
    for (i, &(n, size)) in depths.iter().enumerate() {
        distribution(
            &mut rows,
            "Глубина",
            &format!("Уровень {}{}", i + 1, if i == 11 { "+" } else { "" }),
            n,
            size,
            count,
            total,
        );
    }
    let categories = [
        (
            "Изображения",
            "png jpg jpeg gif webp bmp tiff tif svg avif heic ico raw",
        ),
        ("Видео", "mp4 mkv avi mov webm mpeg mpg m4v wmv flv"),
        ("Аудио", "mp3 flac wav ogg m4a aac opus wma aiff"),
        ("Архивы", "zip 7z rar tar gz bz2 xz zst tgz cab"),
        ("Документы", "pdf doc docx odt rtf txt md tex epub"),
        ("Таблицы", "xls xlsx ods csv tsv"),
        ("Презентации", "ppt pptx odp key"),
        (
            "Исходный код",
            "rs py js ts tsx jsx c cpp h hpp cs java go rb php swift kt lua",
        ),
        ("Программы", "exe msi msp com bat cmd ps1 sh"),
        ("Библиотеки", "dll so dylib lib a pdb"),
        ("Базы данных", "db sqlite sqlite3 mdb accdb sql"),
        ("Образы дисков", "iso img vhd vhdx vmdk qcow2"),
        ("Шрифты", "ttf otf woff woff2"),
        ("Журналы и временные", "log tmp temp bak dmp"),
        ("Конфигурация", "json yaml yml toml xml ini cfg conf"),
    ];
    let mut classified = (0u64, 0u64);
    for (name, list) in categories {
        let mut n = 0;
        let mut size = 0u64;
        for ext in list.split_whitespace() {
            if let Some(&(c, b)) = extensions.get(ext) {
                n += c;
                size = size.saturating_add(b);
            }
        }
        classified.0 += n;
        classified.1 = classified.1.saturating_add(size);
        distribution(&mut rows, "Типы файлов", name, n, size, count, total);
    }
    distribution(
        &mut rows,
        "Типы файлов",
        "Прочие (включая без расширения)",
        count.saturating_sub(classified.0),
        total.saturating_sub(classified.1),
        count,
        total,
    );
    for (i, name) in [
        "Не-ASCII имена",
        "Имена с пробелами",
        "Имена с начальной точкой",
        "Без расширения",
        "Имена длиннее 100 символов",
        "Пути длиннее 260 символов",
    ]
    .iter()
    .enumerate()
    {
        add(&mut rows, "Имена", *name, name_counts[i]);
    }
    add(
        &mut rows,
        "Имена",
        "Средняя длина имени файла, символов",
        average(name_chars, count),
    );
    add(
        &mut rows,
        "Имена",
        "Максимальная длина имени файла, символов",
        longest_name.0,
    );
    add(
        &mut rows,
        "Имена",
        "Самое длинное имя файла",
        if count == 0 {
            "Нет данных".into()
        } else {
            path(longest_name.1)
        },
    );
    add(
        &mut rows,
        "Имена",
        "Максимальная длина пути, символов",
        longest_path.0,
    );
    add(
        &mut rows,
        "Имена",
        "Самый длинный путь",
        if reachable == 0 {
            "Нет данных".into()
        } else {
            path(longest_path.1)
        },
    );
    add(
        &mut rows,
        "Папки",
        "Максимум непосредственных детей",
        max_children.0,
    );
    add(
        &mut rows,
        "Папки",
        "Папка с наибольшим числом детей",
        if reachable == 0 {
            "Нет данных".into()
        } else {
            path(max_children.1)
        },
    );
    for (rank, &(size, idx)) in files.iter().rev().take(10).enumerate() {
        add(
            &mut rows,
            "Крупнейшие файлы",
            format!("№{} · путь", rank + 1),
            path(idx),
        );
        add(
            &mut rows,
            "Крупнейшие файлы",
            format!("№{} · размер", rank + 1),
            bytes(size),
        );
        add(
            &mut rows,
            "Крупнейшие файлы",
            format!("№{} · доля объёма", rank + 1),
            ratio(size, total),
        );
    }
    for (rank, &(size, idx)) in dirs
        .iter()
        .rev()
        .filter(|(_, idx)| *idx != 0)
        .take(10)
        .enumerate()
    {
        add(
            &mut rows,
            "Папки",
            format!("№{} · путь (по объёму)", rank + 1),
            path(idx),
        );
        add(
            &mut rows,
            "Папки",
            format!("№{} · размер поддерева", rank + 1),
            bytes(size),
        );
    }
    #[cfg(not(target_family = "wasm"))]
    if !source.nodes.is_empty() {
        let root = path(0).to_lowercase();
        let disks = sysinfo::Disks::new_with_refreshed_list();
        if let Some(disk) = disks
            .iter()
            .filter(|d| root.starts_with(&d.mount_point().to_string_lossy().to_lowercase()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
        {
            add(
                &mut rows,
                "Том (сейчас)",
                "Источник",
                "Текущая система; может отличаться от сохранённого снимка",
            );
            add(
                &mut rows,
                "Том (сейчас)",
                "Точка монтирования",
                disk.mount_point().display(),
            );
            add(
                &mut rows,
                "Том (сейчас)",
                "Файловая система",
                disk.file_system().to_string_lossy(),
            );
            add(
                &mut rows,
                "Том (сейчас)",
                "Ёмкость",
                bytes(disk.total_space()),
            );
            add(
                &mut rows,
                "Том (сейчас)",
                "Доступно",
                bytes(disk.available_space()),
            );
            add(
                &mut rows,
                "Том (сейчас)",
                "Занято",
                bytes(disk.total_space().saturating_sub(disk.available_space())),
            );
            add(
                &mut rows,
                "Том (сейчас)",
                "Процент заполнения",
                ratio(
                    disk.total_space().saturating_sub(disk.available_space()),
                    disk.total_space(),
                ),
            );
        }
    }
    Report { source, rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{FileNode, NodeStorage, StringPool, precompute_dir_counts};
    fn fixture() -> Arc<FileArenaSnapshot> {
        let mut pool = StringPool::new();
        let mut root = FileNode::new(pool.get_or_insert(b"root"), None, true, false, 0, 0);
        root.first_child = 1;
        root.size = 1024;
        let mut dir = FileNode::new(pool.get_or_insert(b"folder"), Some(0), true, false, 0, 0);
        dir.first_child = 2;
        dir.size = 1024;
        let mut file = FileNode::new(
            pool.get_or_insert(b"photo.JPG"),
            Some(1),
            false,
            false,
            1,
            0,
        );
        file.size = 1024;
        file.next_sibling = 3;
        let empty = FileNode::new(pool.get_or_insert(b"empty"), Some(1), false, false, 0, 0);
        // This orphan is intentionally NOT in the reachable child chain.
        let mut orphan = FileNode::new(
            pool.get_or_insert(b"deleted.exe"),
            Some(0),
            false,
            false,
            1,
            1,
        );
        orphan.size = 999999;
        let nodes = vec![root, dir, file, empty, orphan];
        Arc::new(FileArenaSnapshot {
            dir_counts: Arc::new(precompute_dir_counts(&nodes)),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            extensions: Default::default(),
        })
    }
    #[test]
    fn analytics_excludes_orphans_and_has_over_100_metrics() {
        let report = analyze(fixture());
        assert!(report.rows.len() > 100);
        let value = |label: &str| {
            report
                .rows
                .iter()
                .find(|r| r.name == label)
                .map(|r| r.value.as_str())
        };
        assert_eq!(value("Файлов"), Some("2"));
        assert_eq!(value("Максимальная вложенность (корень = 0)"), Some("2"));
        assert_eq!(value("0 Б · файлов"), Some("1"));
        assert_eq!(value("1 – <10 КиБ · файлов"), Some("1"));
        assert_eq!(value("Изображения · файлов"), Some("1"));
        assert!(!report.rows.iter().any(|r| r.value.contains("deleted.exe")));
    }
    #[test]
    fn empty_snapshot_and_zero_denominators_are_supported() {
        let mut snapshot = (*fixture()).clone();
        snapshot.nodes = Arc::new(NodeStorage::Owned(Vec::new()));
        let report = analyze(Arc::new(snapshot));
        assert!(report.rows.len() > 100);
        assert!(report.rows.iter().any(|r| r.value == "Нет данных"));
        assert!(!report.rows.iter().any(|r| r.value.contains("NaN")));
    }

    #[test]
    fn statistics_window_renders_and_filters_without_native_window() {
        let source = fixture();
        let mut window = DiskStatsWindow {
            open: true,
            report: Some(analyze(Arc::clone(&source))),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        for query in ["", "Самый большой файл", "несуществующий показатель"]
        {
            window.query = query.into();
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                window.show(ui.ctx(), &source)
            });
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                window.show(ui.ctx(), &source)
            });
            assert!(!output.shapes.is_empty());
        }
    }
}
