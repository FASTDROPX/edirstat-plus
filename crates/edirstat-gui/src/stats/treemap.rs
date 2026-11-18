use eframe::egui::{Color32, Rect, pos2};
use fluent_zero::t;

use super::{StatComponent, StatContext, StatsChart};
use crate::{
    arena::{FileNode, NO_INDEX, StringPool},
    colors,
};

const TRUNCATE_DEPTH: usize = 80000;
const PIXEL_PRECISION_LIMIT: f32 = 0.2;
const MIN_FILE_PIXELS: f32 = 3.0; // 1 pixel of content plus a 1-pixel border on each side.

#[derive(Clone)]
pub struct TreemapBlock {
    pub rect: Rect,
    pub node_idx: u32,
    pub color: Color32,
    pub is_frame: bool,
    pub transition: f32,
    pub header_transition: f32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TreemapStyle {
    VerticalGradient,
    #[default]
    OffsetVerticalGradient,
    DiagonalGradient,
    Cushion,
    Flat,
    Bevel,
    Glass,
    Neon,
    Outline,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MapLayout {
    #[default]
    Folders,
    Classic,
    Stripes,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MapColor {
    #[default]
    Extension,
    Folder,
    Size,
    Age,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MapDetail {
    #[default]
    Folders,
    Files,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MapAnimation {
    Off,
    Colors,
    #[default]
    Soft,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MapPalette {
    #[default]
    Pastel,
    Vivid,
    Ocean,
    Earth,
    Mono,
}

/// Camera zoom is independent of folder navigation. Keep the point under the cursor fixed.
fn zoom_about(
    offset: eframe::egui::Vec2,
    pivot: eframe::egui::Vec2,
    old: f32,
    new: f32,
) -> eframe::egui::Vec2 {
    pivot - (pivot - offset) * (new / old)
}

// Keep the original long camera ease-out; camera motion never changes layout geometry.
fn inertial_camera_step(scale: f32, target: f32, dt: f32) -> f32 {
    let next = scale + (target - scale) * (1.0 - (-16.0 * dt.min(0.05)).exp());
    if (next - target).abs() < 0.001 {
        target
    } else {
        next
    }
}

fn palette_color(color: Color32, palette: MapPalette) -> Color32 {
    if palette == MapPalette::Pastel {
        return color;
    }
    let mut hsv = eframe::egui::ecolor::Hsva::from(color);
    match palette {
        MapPalette::Pastel => {}
        MapPalette::Vivid => {
            hsv.s = 0.92;
            hsv.v = 0.95;
        }
        MapPalette::Ocean => {
            hsv.h = 0.45 + hsv.h * 0.22;
            hsv.s = 0.7;
            hsv.v = 0.85;
        }
        MapPalette::Earth => {
            hsv.h *= 0.16;
            hsv.s = 0.55;
            hsv.v = 0.85;
        }
        MapPalette::Mono => {
            hsv.s = 0.0;
            hsv.v = 0.3 + hsv.h * 0.65;
        }
    }
    Color32::from(hsv)
}

fn raw_block_color(
    block: &TreemapBlock,
    snapshot: &crate::arena::FileArenaSnapshot,
    coloring: MapColor,
    palette: MapPalette,
) -> Color32 {
    let node = &snapshot.nodes[block.node_idx as usize];
    let fill_color = match coloring {
        MapColor::Extension => block.color,
        MapColor::Folder => {
            let parent = snapshot.nodes.get(node.parent as usize).unwrap_or(node);
            colors::get_treemap_dir_color(snapshot.string_pool.get(parent.name_id).unwrap_or(""))
        }
        MapColor::Size => heat_color((node.size.max(1) as f32).log2() / 40.0),
        MapColor::Age => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            if node.modified_timestamp == 0 {
                Color32::GRAY
            } else {
                heat_color(
                    now.saturating_sub(u64::from(node.modified_timestamp)) as f32
                        / (86400.0 * 3650.0),
                )
            }
        }
    };
    let palette = if matches!(coloring, MapColor::Age | MapColor::Size) {
        MapPalette::Pastel
    } else {
        palette
    };
    palette_color(fill_color, palette)
}

fn mix_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

fn block_color(
    block: &TreemapBlock,
    snapshot: &crate::arena::FileArenaSnapshot,
    coloring: MapColor,
    palette: MapPalette,
    directory_colors: &[Color32],
) -> Color32 {
    let own = raw_block_color(block, snapshot, coloring, palette);
    if block.transition >= 1.0 {
        return own;
    }
    let parent_idx = snapshot.nodes[block.node_idx as usize].parent;
    let Some(parent) = snapshot.nodes.get(parent_idx as usize) else {
        return own;
    };
    let parent_block = TreemapBlock {
        rect: block.rect,
        node_idx: parent_idx,
        color: directory_colors
            .get(parent_idx as usize)
            .copied()
            .unwrap_or_else(|| {
                colors::get_treemap_dir_color(
                    snapshot.string_pool.get(parent.name_id).unwrap_or(""),
                )
            }),
        is_frame: true,
        transition: 1.0,
        header_transition: 1.0,
    };
    mix_color(
        raw_block_color(&parent_block, snapshot, coloring, palette),
        own,
        block.transition,
    )
}

pub struct TileMotion {
    progress: f32,
    header_progress: f32,
    had_header: bool,
    rect: Rect,
    seen: bool,
}

// Animate only small local corrections. Large reflows snap directly rather
// than sending tiles across the map. Tiny files never enter geometric tweens.
fn animate_layout(
    states: &mut std::collections::HashMap<u32, TileMotion>,
    targets: &[TreemapBlock],
    dt: f32,
    initialize: bool,
    mode: MapAnimation,
    map: Rect,
    dpi: f32,
) -> (Vec<TreemapBlock>, bool) {
    animate_layout_inner(states, targets, dt, initialize, mode, map, dpi, true)
}

fn animate_layout_inner(
    states: &mut std::collections::HashMap<u32, TileMotion>,
    targets: &[TreemapBlock],
    dt: f32,
    initialize: bool,
    mode: MapAnimation,
    map: Rect,
    dpi: f32,
    animate_geometry: bool,
) -> (Vec<TreemapBlock>, bool) {
    if initialize || mode == MapAnimation::Off {
        states.clear();
    }
    if mode == MapAnimation::Off {
        return (targets.to_vec(), false);
    }
    for state in states.values_mut() {
        state.seen = false;
    }
    let step = dt.clamp(0.0, 0.05) / 0.16;
    let alpha = 1.0 - (-28.0 * dt.clamp(0.0, 0.05)).exp();
    let relative = |r: Rect| {
        Rect::from_min_max(
            pos2(
                (r.left() - map.left()) / map.width(),
                (r.top() - map.top()) / map.height(),
            ),
            pos2(
                (r.right() - map.left()) / map.width(),
                (r.bottom() - map.top()) / map.height(),
            ),
        )
    };
    let screen = |r: Rect| {
        Rect::from_min_max(
            map.min + r.min.to_vec2() * map.size(),
            map.min + r.max.to_vec2() * map.size(),
        )
    };
    let mut running = false;
    let mut closing_headers = Vec::new();
    let mut output: Vec<_> = targets
        .iter()
        .map(|block| {
            let has_header =
                block.is_frame && block.rect.width() >= 70.0 && block.rect.height() >= 42.0;
            let state = states.entry(block.node_idx).or_insert(TileMotion {
                progress: if initialize { 1.0 } else { 0.0 },
                header_progress: if initialize && has_header { 1.0 } else { 0.0 },
                had_header: has_header,
                rect: relative(block.rect),
                seen: true,
            });
            if has_header && !state.had_header && !initialize {
                state.header_progress = 0.0;
            }
            state.had_header = has_header;
            state.seen = true;
            state.progress = (state.progress + step).min(1.0);
            state.header_progress = if has_header {
                (state.header_progress + step).min(1.0)
            } else {
                (state.header_progress - step).max(0.0)
            };
            let eligible =
                has_header || (!block.is_frame && block.rect.area() * dpi * dpi >= 256.0);
            let mut result = block.clone();
            if animate_geometry && mode == MapAnimation::Soft && eligible {
                let old = screen(state.rect);
                let distance_sq = (old.min - block.rect.min)
                    .length_sq()
                    .max((old.max - block.rect.max).length_sq());
                let limit = (12.0 / dpi).min(block.rect.width().min(block.rect.height()) * 0.12);
                let epsilon = (0.15 / dpi).max(map.width().max(map.height()) * f32::EPSILON * 2.0);
                if distance_sq > epsilon * epsilon && distance_sq <= limit * limit {
                    result.rect = Rect::from_min_max(
                        old.min + (block.rect.min - old.min) * alpha,
                        old.max + (block.rect.max - old.max) * alpha,
                    );
                    running = true;
                }
            }
            state.rect = relative(result.rect);
            running |= state.progress < 1.0;
            result.transition = state.progress * state.progress * (3.0 - 2.0 * state.progress);
            if mode == MapAnimation::Soft {
                let q = state.header_progress;
                result.header_transition = q * q * (3.0 - 2.0 * q);
                running |= if has_header { q < 1.0 } else { q > 0.0 };
                if !block.is_frame && q > 0.0 {
                    let mut header = result.clone();
                    header.is_frame = true;
                    closing_headers.push(header);
                }
            }
            result
        })
        .collect();
    output.extend(closing_headers);
    states.retain(|_, state| state.seen);
    (output, running)
}

// Compute a representative file for each directory once per snapshot. Parent
// links can be out of arena order, so process leaves before their parents.
fn directory_fill_colors(nodes: &[FileNode], pool: &StringPool) -> Vec<Color32> {
    let mut representatives = vec![NO_INDEX; nodes.len()];
    let mut pending = vec![0u32; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        if !node.is_directory() {
            representatives[i] = i as u32;
        }
        if let Some(count) = pending.get_mut(node.parent as usize) {
            *count += 1;
        }
    }
    let mut ready: Vec<u32> = pending
        .iter()
        .enumerate()
        .filter_map(|(i, &count)| (count == 0).then_some(i as u32))
        .collect();
    while let Some(i) = ready.pop() {
        let parent = nodes[i as usize].parent as usize;
        if parent >= nodes.len() {
            continue;
        }
        let candidate = representatives[i as usize];
        let current = representatives[parent];
        if candidate != NO_INDEX
            && (current == NO_INDEX
                || nodes[candidate as usize].size > nodes[current as usize].size)
        {
            representatives[parent] = candidate;
        }
        pending[parent] -= 1;
        if pending[parent] == 0 {
            ready.push(parent as u32);
        }
    }
    representatives
        .iter()
        .enumerate()
        .map(|(i, &file)| {
            if !nodes[i].is_directory() {
                return Color32::TRANSPARENT;
            }
            if let Some(file) = nodes.get(file as usize) {
                colors::get_color_for_extension(crate::arena::get_ext_slice(
                    pool.get(file.name_id).unwrap_or(""),
                ))
            } else {
                colors::get_treemap_dir_color(pool.get(nodes[i].name_id).unwrap_or(""))
            }
        })
        .collect()
}

fn file_border_color(fill: Color32) -> Color32 {
    scale_rgb(fill, 0.72)
}

// Four disjoint strips keep every edge one physical pixel wide and share one
// GPU mesh, including folder plates whose children are below the size limit.
fn append_file_border(mesh: &mut eframe::egui::Mesh, rect: Rect, dpi: f32, color: Color32) {
    let edge = (1.0 / dpi).min(rect.width() * 0.5).min(rect.height() * 0.5);
    mesh.add_colored_rect(
        Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + edge)),
        color,
    );
    mesh.add_colored_rect(
        Rect::from_min_max(pos2(rect.left(), rect.bottom() - edge), rect.max),
        color,
    );
    mesh.add_colored_rect(
        Rect::from_min_max(
            pos2(rect.left(), rect.top() + edge),
            pos2(rect.left() + edge, rect.bottom() - edge),
        ),
        color,
    );
    mesh.add_colored_rect(
        Rect::from_min_max(
            pos2(rect.right() - edge, rect.top() + edge),
            pos2(rect.right(), rect.bottom() - edge),
        ),
        color,
    );
}

fn raster_tile(block: &TreemapBlock) -> bool {
    !block.is_frame && block.rect.area() < 36.0
}

fn gpu_tiles(
    blocks: &[TreemapBlock],
    snapshot: &crate::arena::FileArenaSnapshot,
    viewport: Rect,
    dpi: f32,
    coloring: MapColor,
    palette: MapPalette,
    style: TreemapStyle,
    directory_colors: &[Color32],
) -> Vec<super::treemap_gpu::Tile> {
    let mut tiles = Vec::with_capacity(blocks.len());
    for block in blocks.iter().filter(|b| raster_tile(b)) {
        let r = block.rect.intersect(viewport);
        if !r.is_positive() {
            continue;
        }
        let min = ((r.min - viewport.min) * dpi).round();
        let max = ((r.max - viewport.min) * dpi).round();
        if min.x >= max.x || min.y >= max.y {
            continue;
        }
        let original_min = (block.rect.min - viewport.min) * dpi;
        let original_max = (block.rect.max - viewport.min) * dpi;
        let base = block_color(block, snapshot, coloring, palette, directory_colors);
        tiles.push(super::treemap_gpu::Tile {
            original: [
                original_min.x,
                original_min.y,
                original_max.x,
                original_max.y,
            ],
            color: u32::from_le_bytes(base.to_array()),
            border: u32::from_le_bytes(file_border_color(base).to_array()),
            transition: block.transition,
            style: style as u32,
        });
    }
    tiles
}

// Tiny files retain their real rectangles and node IDs for picking. A screen-size
// texture bounds GPU memory independently of the number of files in a directory.
fn rasterize_files(
    blocks: &[TreemapBlock],
    snapshot: &crate::arena::FileArenaSnapshot,
    viewport: Rect,
    pixels_per_point: f32,
    coloring: MapColor,
    palette: MapPalette,
    style: TreemapStyle,
    directory_colors: &[Color32],
) -> eframe::egui::ColorImage {
    let width = ((viewport.width() * pixels_per_point).ceil() as usize).clamp(1, 4096);
    let height = ((viewport.height() * pixels_per_point).ceil() as usize).clamp(1, 4096);
    let mut image =
        eframe::egui::ColorImage::new([width, height], vec![Color32::TRANSPARENT; width * height]);
    let sx = width as f32 / viewport.width().max(1.0);
    let sy = height as f32 / viewport.height().max(1.0);
    for block in blocks.iter().filter(|b| raster_tile(b)) {
        let r = block.rect.intersect(viewport);
        let x0 = ((r.left() - viewport.left()) * sx)
            .round()
            .clamp(0.0, width as f32) as usize;
        let x1 = ((r.right() - viewport.left()) * sx)
            .round()
            .clamp(0.0, width as f32) as usize;
        let y0 = ((r.top() - viewport.top()) * sy)
            .round()
            .clamp(0.0, height as f32) as usize;
        let y1 = ((r.bottom() - viewport.top()) * sy)
            .round()
            .clamp(0.0, height as f32) as usize;
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let base = block_color(block, snapshot, coloring, palette, directory_colors);
        let border = file_border_color(base);
        for y in y0..y1 {
            let py = viewport.top() + (y as f32 + 0.5) / sy;
            let v = ((py - block.rect.top()) / block.rect.height()).clamp(0.0, 1.0);
            for x in x0..x1 {
                let px = viewport.left() + (x as f32 + 0.5) / sx;
                let u = ((px - block.rect.left()) / block.rect.width()).clamp(0.0, 1.0);
                let shade = match style {
                    TreemapStyle::Flat => 1.0,
                    TreemapStyle::DiagonalGradient => 1.15 - 0.2 * (u + v),
                    TreemapStyle::Cushion => {
                        1.25 - 0.6 * ((u - 0.4).powi(2) + (v - 0.4).powi(2)).sqrt().min(1.0)
                    }
                    TreemapStyle::OffsetVerticalGradient => 1.15 - 0.35 * v - 0.06 * u,
                    TreemapStyle::Outline | TreemapStyle::Neon => 0.5,
                    _ => 1.15 - 0.4 * v,
                };
                let edge = (px - block.rect.left())
                    .min(block.rect.right() - px)
                    .min(py - block.rect.top())
                    .min(block.rect.bottom() - py);
                image.pixels[y * width + x] = if edge * pixels_per_point < 1.0 {
                    border
                } else {
                    scale_rgb(base, 1.0 + (shade - 1.0) * block.transition)
                };
            }
        }
    }
    image
}

// Focus is a camera excursion through the same scene. The previous root and
// topology stay available while the wheel continuously reverses the excursion.
pub(crate) struct FocusLevel {
    root: u32,
    child: u32,
    root_depth: usize,
    child_depth: usize,
    scale: f32,
    target_scale: f32,
    offset: eframe::egui::Vec2,
    pivot: eframe::egui::Vec2,
    source: Rect,
    parent_cache: std::cell::RefCell<DirectoryCache>,
    progress: f32,
    target_progress: f32,
    entry_drive: f32,
    auto_entering: bool,
}

fn focus_fit(source: Rect) -> f32 {
    (1.0 / source.width().max(0.0001))
        .min(1.0 / source.height().max(0.0001))
        .max(1.0)
}

fn focus_motion_step(progress: f32, target: f32, source: Rect, dt: f32) -> f32 {
    let range = (focus_fit(source) - 1.0).max(1.0);
    let destination = 1.0 + range * target;
    let next = inertial_camera_step(1.0 + range * progress, destination, dt);
    if next == destination {
        target
    } else {
        ((next - 1.0) / range).clamp(0.0, 1.0)
    }
}

fn local_camera_rect(viewport: Rect, scale: f32, offset: eframe::egui::Vec2, item: Rect) -> Rect {
    Rect::from_min_size(
        viewport.min + offset + (item.min - viewport.min) * scale,
        item.size() * scale,
    )
}

fn focus_camera_rect(viewport: Rect, source: Rect, progress: f32, item: Rect) -> Rect {
    let t = progress.clamp(0.0, 1.0);
    let pivot = viewport.min + source.center().to_vec2() * viewport.size();
    let zoom = 1.0 + (focus_fit(source) - 1.0) * t;
    let pan = (viewport.center() - pivot) * t;
    Rect::from_min_max(
        pivot + (item.min - pivot) * zoom + pan,
        pivot + (item.max - pivot) * zoom + pan,
    )
}

fn focus_rect(viewport: Rect, source: Rect, progress: f32) -> Rect {
    let start = Rect::from_min_max(
        viewport.min + source.min.to_vec2() * viewport.size(),
        viewport.min + source.max.to_vec2() * viewport.size(),
    );
    let camera = focus_camera_rect(viewport, source, progress, start);
    // Only the focused region changes aspect. The surrounding scene follows
    // a uniform zoom and pan, so siblings come back at the correct large scale.
    Rect::from_min_max(
        camera.min.lerp(viewport.min, progress),
        camera.max.lerp(viewport.max, progress),
    )
}

pub struct TreemapChart {
    pub(crate) focus_history: Vec<FocusLevel>,
    pub directory_cache: std::cell::RefCell<DirectoryCache>,
    pub reference_size: eframe::egui::Vec2,
    pub detail: MapDetail,
    pub animation: MapAnimation,
    pub last_animation: MapAnimation,
    pub directory_colors: Vec<Color32>,
    pub palette: MapPalette,
    pub scale: f32,
    pub target_scale: f32,
    pub offset: eframe::egui::Vec2,
    pub pivot: eframe::egui::Vec2,
    pub last_detail: MapDetail,
    pub last_pixels_per_point: f32,
    pub detail_texture: Option<eframe::egui::TextureHandle>,
    pub detail_texture_style: Option<(MapPalette, MapColor, TreemapStyle)>,
    pub(crate) gpu_tiles: [Option<std::sync::Arc<Vec<super::treemap_gpu::Tile>>>; 2],
    pub delete_request: Option<u32>,
    pub allow_fs_actions: bool,

    pub layout: MapLayout,
    pub coloring: MapColor,
    pub last_layout: MapLayout,
    pub cached_blocks: Vec<TreemapBlock>,
    pub display_blocks: Vec<TreemapBlock>,
    pub background_display_blocks: Vec<TreemapBlock>,
    pub background_nodes: std::collections::HashSet<u32>,
    pub background_texture: Option<eframe::egui::TextureHandle>,
    pub background_texture_style: Option<(MapPalette, MapColor, TreemapStyle)>,
    pub tile_motion: std::collections::HashMap<u32, TileMotion>,
    pub animation_running: bool,
    pub last_snapshot_ptr: usize,
    pub last_rect: Rect,
    pub draw_borders: bool,
    pub style: TreemapStyle,
    pub zoom_root: u32,
    pub last_zoom_root: u32,
    pub context_menu_target: Option<u32>,
}

impl TreemapChart {
    fn enter_focus(
        &mut self,
        child: u32,
        source: Rect,
        viewport: Rect,
        snapshot: &crate::arena::FileArenaSnapshot,
    ) {
        let nodes = &snapshot.nodes;
        let normalize = |r: Rect| {
            Rect::from_min_max(
                ((r.min - viewport.min) / viewport.size()).to_pos2(),
                ((r.max - viewport.min) / viewport.size()).to_pos2(),
            )
        };
        let root_depth = self.focus_history.last().map_or(0, |f| f.child_depth);
        let mut child_depth = root_depth;
        let mut current = child;
        while current != self.zoom_root && current != NO_INDEX {
            child_depth += 1;
            current = nodes
                .get(current as usize)
                .map_or(NO_INDEX, |node| node.parent);
        }
        self.focus_history.push(FocusLevel {
            root: self.zoom_root,
            child,
            root_depth,
            child_depth,
            scale: self.scale,
            target_scale: self.target_scale,
            offset: self.offset / viewport.size(),
            pivot: self.pivot / viewport.size(),
            source: normalize(source),
            parent_cache: std::cell::RefCell::new(self.directory_cache.borrow().clone()),
            progress: if self.animation == MapAnimation::Off {
                1.0
            } else {
                0.0
            },
            target_progress: 1.0,
            entry_drive: 0.0,
            auto_entering: self.animation != MapAnimation::Off,
        });
        self.zoom_root = child;
        self.reset_camera();
        self.prepare_focus_layout(snapshot, viewport, child_depth);
    }

    fn prepare_focus_layout(
        &self,
        snapshot: &crate::arena::FileArenaSnapshot,
        viewport: Rect,
        depth: usize,
    ) {
        // Navigation changes the reference aspect ratio. Keep the old rows in
        // the history for the parent camera, but derive a separate topology for
        // the final focus viewport before the animated rectangle starts small.
        self.directory_cache.borrow_mut().entries.clear();
        let config = TreemapConfig {
            skip_subtree: None,
            layout_cache: Some(&self.directory_cache),
            geometry_scale: 1.0,
            nodes: &snapshot.nodes,
            string_pool: &snapshot.string_pool,
            max_depth: 128,
            pixels_per_point: self.last_pixels_per_point,
            directory_colors: &self.directory_colors,
            layout: self.layout,
            detail: self.detail,
            clip: viewport,
        };
        build_treemap(&config, self.zoom_root, viewport, depth, &mut Vec::new());
    }

    pub fn reset_camera(&mut self) {
        self.scale = 1.0;
        self.target_scale = 1.0;
        self.offset = eframe::egui::Vec2::ZERO;
    }

    #[must_use]
    pub fn new() -> Self {
        Self {
            focus_history: Vec::new(),
            directory_cache: Default::default(),
            reference_size: eframe::egui::Vec2::ZERO,
            detail: MapDetail::Folders,
            animation: MapAnimation::Soft,
            last_animation: MapAnimation::Off,
            directory_colors: Vec::new(),
            palette: MapPalette::Pastel,
            scale: 1.0,
            target_scale: 1.0,
            offset: eframe::egui::Vec2::ZERO,
            pivot: eframe::egui::Vec2::ZERO,
            last_detail: MapDetail::Folders,
            last_pixels_per_point: 1.0,
            detail_texture: None,
            detail_texture_style: None,
            gpu_tiles: [None, None],
            delete_request: None,
            allow_fs_actions: true,
            layout: MapLayout::Folders,
            coloring: MapColor::Extension,
            last_layout: MapLayout::Folders,
            cached_blocks: Vec::new(),
            display_blocks: Vec::new(),
            background_display_blocks: Vec::new(),
            background_nodes: Default::default(),
            background_texture: None,
            background_texture_style: None,
            tile_motion: Default::default(),
            animation_running: false,
            last_snapshot_ptr: 0,
            last_rect: Rect::NOTHING,
            draw_borders: false,
            style: TreemapStyle::VerticalGradient,
            zoom_root: 0,
            last_zoom_root: 0,
            context_menu_target: None,
        }
    }
}

impl Default for TreemapChart {
    fn default() -> Self {
        Self::new()
    }
}

impl StatsChart for TreemapChart {
    type Output = Vec<TreemapBlock>;

    fn compute(&mut self, snapshot: &crate::arena::FileArenaSnapshot) -> Self::Output {
        let mut blocks = Vec::new();
        if snapshot.nodes.is_empty() {
            return blocks;
        }

        if (self.zoom_root as usize) >= snapshot.nodes.len()
            || snapshot.nodes[self.zoom_root as usize].size == 0
            || (self.zoom_root != 0 && !snapshot.nodes[self.zoom_root as usize].is_directory())
        {
            self.zoom_root = 0;
        }

        self.directory_colors = directory_fill_colors(&snapshot.nodes, &snapshot.string_pool);
        let config = TreemapConfig {
            skip_subtree: None,
            layout_cache: None,
            geometry_scale: 1.0,
            nodes: &snapshot.nodes,
            string_pool: &snapshot.string_pool,
            max_depth: 128,
            pixels_per_point: self.last_pixels_per_point,
            directory_colors: &self.directory_colors,
            layout: self.layout,
            detail: self.detail,
            clip: self.last_rect,
        };

        build_treemap(&config, self.zoom_root, self.last_rect, 0, &mut blocks);

        blocks
    }
}

#[must_use]
pub fn get_selection_roots<S: ::std::hash::BuildHasher + Default>(
    nodes: &[FileNode],
    selected_nodes: &std::collections::HashSet<u32, S>,
) -> std::collections::HashSet<u32> {
    let mut roots = std::collections::HashSet::new();
    for &idx in selected_nodes {
        if idx as usize >= nodes.len() {
            continue;
        }
        let mut curr = nodes[idx as usize].parent;
        let mut ancestor_selected = false;
        while curr != crate::arena::NO_INDEX {
            if selected_nodes.contains(&curr) {
                ancestor_selected = true;
                break;
            }
            if curr as usize >= nodes.len() {
                break;
            }
            curr = nodes[curr as usize].parent;
        }
        if !ancestor_selected {
            roots.insert(idx);
        }
    }
    roots
}

fn heat_color(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        (40.0 + 210.0 * t) as u8,
        (170.0 - 100.0 * t) as u8,
        (230.0 - 185.0 * t) as u8,
    )
}

fn scale_rgb(color: Color32, factor: f32) -> Color32 {
    Color32::from_rgba_premultiplied(
        (color.r() as f32 * factor).clamp(0.0, 255.0) as u8,
        (color.g() as f32 * factor).clamp(0.0, 255.0) as u8,
        (color.b() as f32 * factor).clamp(0.0, 255.0) as u8,
        color.a(),
    )
}

impl StatComponent for TreemapChart {
    fn render(
        &mut self,
        ui: &mut eframe::egui::Ui,
        snapshot: &crate::arena::FileArenaSnapshot,
        context: &mut StatContext,
    ) {
        let available_rect = ui.available_rect_before_wrap();
        let (rect, response) = ui.allocate_exact_size(
            eframe::egui::vec2(available_rect.width() - 4.0, available_rect.height()),
            eframe::egui::Sense::click_and_drag(),
        );

        let snapshot_ptr = std::sync::Arc::as_ptr(&snapshot.nodes) as usize;
        if snapshot_ptr != self.last_snapshot_ptr
            || self
                .focus_history
                .last()
                .is_some_and(|f| f.child != self.zoom_root)
        {
            self.focus_history.clear();
        }
        if self.zoom_root != self.last_zoom_root {
            self.reset_camera();
        }
        // Camera wheel gestures, including reversing folder focus, require Ctrl.
        let zoom = if response.hovered() && ui.input(|i| i.modifiers.ctrl) {
            ui.input(|i| i.zoom_delta())
        } else {
            1.0
        };
        let mut consumed_zoom = false;
        if (zoom - 1.0).abs() > 0.00001 {
            if let Some(focus) = self.focus_history.last_mut() {
                if self.target_scale <= 1.001 && (zoom < 1.0 || focus.target_progress < 1.0) {
                    if focus.auto_entering {
                        focus.target_progress = focus.progress;
                        focus.auto_entering = false;
                    }
                    let range = (focus_fit(focus.source) - 1.0).max(1.0);
                    focus.target_progress = (((1.0 + range * focus.target_progress) * zoom - 1.0)
                        / range)
                        .clamp(0.0, 1.0);
                    consumed_zoom = true;
                }
            }
        }
        let navigating = self
            .focus_history
            .last()
            .is_some_and(|f| f.progress < 1.0 || f.target_progress < 1.0);
        if !consumed_zoom
            && !navigating
            && (zoom - 1.0).abs() > 0.00001
            && (ui.input(|i| i.modifiers.ctrl) || !self.focus_history.is_empty() || zoom < 1.0)
        {
            self.pivot = response.hover_pos().unwrap_or(rect.center()) - rect.min;
            self.target_scale = (self.target_scale * zoom).clamp(1.0, 4096.0);
        }
        if response.dragged() && self.scale > 1.001 {
            self.offset += response.drag_delta();
            self.pivot += response.drag_delta();
        }
        if self.animation == MapAnimation::Off && self.scale != self.target_scale {
            self.offset = zoom_about(self.offset, self.pivot, self.scale, self.target_scale);
            self.scale = self.target_scale;
        }
        if self.scale != self.target_scale {
            let next =
                inertial_camera_step(self.scale, self.target_scale, ui.input(|i| i.stable_dt));
            self.offset = zoom_about(self.offset, self.pivot, self.scale, next);
            self.scale = next;
            ui.ctx().request_repaint();
        }
        if self.target_scale == 1.0 && self.scale == 1.0 {
            self.offset = eframe::egui::Vec2::ZERO;
        }
        // Do not pan the whole map out of reach.
        self.offset.x = self.offset.x.clamp(rect.width() * (1.0 - self.scale), 0.0);
        self.offset.y = self.offset.y.clamp(rect.height() * (1.0 - self.scale), 0.0);
        let mut restored = false;
        let mut focus_changed = false;
        if let Some(focus) = self.focus_history.last_mut() {
            // Exactly the same normalized exponential response as wheel camera
            // motion. No second smoothstep or fixed-duration navigation timer.
            let previous = focus.progress;
            let dt = ui.input(|i| i.stable_dt);
            if self.animation == MapAnimation::Off {
                focus.auto_entering = false;
                focus.entry_drive = focus.target_progress;
            }
            // Feed the normal camera response through a gently moving target
            // only for automatic entry: zero initial velocity, no abrupt jump.
            let destination = if focus.auto_entering && self.animation != MapAnimation::Off {
                focus.entry_drive =
                    focus_motion_step(focus.entry_drive, focus.target_progress, focus.source, dt);
                focus.entry_drive
            } else {
                focus.target_progress
            };
            focus.progress = if self.animation == MapAnimation::Off {
                focus.target_progress
            } else {
                focus_motion_step(
                    focus.progress,
                    destination,
                    focus.source,
                    ui.input(|i| i.stable_dt),
                )
            };
            if focus.progress == focus.target_progress && focus.entry_drive == focus.target_progress
            {
                focus.auto_entering = false;
            }
            focus_changed = previous != focus.progress;
            if focus.progress != focus.target_progress {
                ui.ctx().request_repaint();
            }
            if focus.progress == 0.0 && focus.target_progress == 0.0 {
                let parent = self.focus_history.pop().unwrap();
                self.zoom_root = parent.root;
                self.scale = parent.scale;
                self.target_scale = parent.target_scale;
                self.offset = parent.offset * rect.size();
                self.pivot = parent.pivot * rect.size();
                *self.directory_cache.borrow_mut() = parent.parent_cache.into_inner();
                restored = true;
            }
        }
        let layout_rect = Rect::from_min_size(rect.min + self.offset, rect.size() * self.scale);
        let map_rect = if !restored {
            self.focus_history
                .last()
                .filter(|f| f.progress < 1.0)
                .map_or(layout_rect, |f| {
                    local_camera_rect(
                        rect,
                        self.scale,
                        self.offset,
                        focus_rect(rect, f.source, f.progress),
                    )
                })
        } else {
            layout_rect
        };

        // --- Layout Cache Check ---
        // Race-free (the pointer and the snapshot it identifies are the same value).
        // The rescan path always publishes a fresh `Arc`, so every content change
        // invalidates this cache.
        if (self.zoom_root as usize) >= snapshot.nodes.len()
            || snapshot.nodes[self.zoom_root as usize].size == 0
            || (self.zoom_root != 0 && !snapshot.nodes[self.zoom_root as usize].is_directory())
        {
            self.zoom_root = 0;
        }

        let pixels_per_point = ui.ctx().pixels_per_point();
        let initialize_motion = rect.size() != self.reference_size
            || snapshot_ptr != self.last_snapshot_ptr
            || self.zoom_root != self.last_zoom_root
            || self.layout != self.last_layout
            || self.detail != self.last_detail
            || self.animation != self.last_animation;
        let needs_rebuild = initialize_motion
            || focus_changed
            || snapshot_ptr != self.last_snapshot_ptr
            || map_rect != self.last_rect
            || self.zoom_root != self.last_zoom_root
            || self.layout != self.last_layout
            || self.detail != self.last_detail
            || pixels_per_point != self.last_pixels_per_point
            || self.animation != self.last_animation;

        if needs_rebuild {
            if initialize_motion
                && (snapshot_ptr != self.last_snapshot_ptr
                    || self.layout != self.last_layout
                    || self.detail != self.last_detail
                    || (self.focus_history.is_empty() && !restored))
            {
                self.directory_cache.borrow_mut().entries.clear();
            }
            if !self.focus_history.is_empty()
                && (rect.size() != self.reference_size
                    || self.layout != self.last_layout
                    || self.detail != self.last_detail)
            {
                let depth = self.focus_history.last().unwrap().child_depth;
                self.prepare_focus_layout(snapshot, rect, depth);
            }
            if snapshot_ptr != self.last_snapshot_ptr {
                self.context_menu_target = None;
            }
            if snapshot_ptr != self.last_snapshot_ptr {
                self.directory_colors =
                    directory_fill_colors(&snapshot.nodes, &snapshot.string_pool);
            }
            let mut blocks = Vec::new();
            self.background_nodes.clear();
            if !snapshot.nodes.is_empty() {
                let focus = self.focus_history.last();
                let mut geometry_scale = self.scale;
                if let Some(focus) = focus.filter(|f| f.progress < 1.0) {
                    let parent_map = Rect::from_min_size(
                        rect.min + focus.offset * rect.size(),
                        rect.size() * focus.scale,
                    );
                    let camera_map = local_camera_rect(
                        rect,
                        self.scale,
                        self.offset,
                        focus_camera_rect(rect, focus.source, focus.progress, parent_map),
                    );
                    let camera_scale =
                        focus.scale * (1.0 + (focus_fit(focus.source) - 1.0) * focus.progress);
                    let parent_config = TreemapConfig {
                        skip_subtree: Some(focus.child),
                        layout_cache: Some(&focus.parent_cache),
                        geometry_scale: camera_scale * self.scale,
                        nodes: &snapshot.nodes,
                        string_pool: &snapshot.string_pool,
                        max_depth: 128,
                        pixels_per_point,
                        directory_colors: &self.directory_colors,
                        layout: self.layout,
                        detail: self.detail,
                        clip: rect.expand(64.0),
                    };
                    build_treemap(
                        &parent_config,
                        focus.root,
                        camera_map,
                        focus.root_depth,
                        &mut blocks,
                    );
                    self.background_nodes
                        .extend(blocks.iter().map(|block| block.node_idx));
                    geometry_scale =
                        (camera_scale + (1.0 - camera_scale) * focus.progress) * self.scale;
                }
                let config = TreemapConfig {
                    skip_subtree: None,
                    layout_cache: Some(&self.directory_cache),
                    geometry_scale,
                    nodes: &snapshot.nodes,
                    string_pool: &snapshot.string_pool,
                    max_depth: 128,
                    pixels_per_point,
                    directory_colors: &self.directory_colors,
                    layout: self.layout,
                    detail: self.detail,
                    clip: rect.expand(64.0),
                };
                build_treemap(
                    &config,
                    self.zoom_root,
                    map_rect,
                    focus.map_or(0, |f| f.child_depth),
                    &mut blocks,
                );
            }

            self.detail_texture_style = None;
            self.cached_blocks = blocks;
            self.last_snapshot_ptr = snapshot_ptr;
            self.last_rect = map_rect;
            self.reference_size = rect.size();
            self.last_detail = self.detail;
            self.last_animation = self.animation;
            self.last_pixels_per_point = pixels_per_point;
            self.last_zoom_root = self.zoom_root;
            self.last_layout = self.layout;
        }

        if needs_rebuild || self.animation_running {
            let (blocks, running) = animate_layout_inner(
                &mut self.tile_motion,
                &self.cached_blocks,
                ui.input(|i| i.stable_dt),
                initialize_motion,
                self.animation,
                map_rect,
                pixels_per_point,
                !focus_changed && self.detail != MapDetail::Files,
            );
            (self.background_display_blocks, self.display_blocks) = blocks
                .into_iter()
                .partition(|block| self.background_nodes.contains(&block.node_idx));
            self.animation_running = running;
            self.detail_texture_style = None;
            self.background_texture_style = None;
            if running {
                ui.ctx().request_repaint();
            }
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, eframe::egui::Color32::BLACK);
        // Each scene completes every paint phase before the next scene begins.
        // In particular, parent headers can never be drawn over focus files.
        let focus_clip = map_rect.intersect(rect);
        for (slot, (layer_blocks, texture, texture_style, clip, texture_name)) in [
            (
                &self.background_display_blocks,
                &mut self.background_texture,
                &mut self.background_texture_style,
                rect,
                "treemap-background-detail",
            ),
            (
                &self.display_blocks,
                &mut self.detail_texture,
                &mut self.detail_texture_style,
                focus_clip,
                "treemap-file-detail",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if layer_blocks.is_empty() || !clip.is_positive() {
                continue;
            }
            let painter = ui.painter_at(clip);
            let mut backing = eframe::egui::Mesh::default();
            for block in layer_blocks.iter().filter(|b| b.is_frame) {
                backing.add_colored_rect(
                    block.rect,
                    block_color(
                        block,
                        snapshot,
                        self.coloring,
                        self.palette,
                        &self.directory_colors,
                    ),
                );
            }
            painter.add(backing);
            if self.detail == MapDetail::Files
                && let Some(format) = super::treemap_gpu::target(ui.ctx())
            {
                let key = (self.palette, self.coloring, self.style);
                if *texture_style != Some(key) || self.gpu_tiles[slot].is_none() {
                    self.gpu_tiles[slot] = Some(std::sync::Arc::new(gpu_tiles(
                        layer_blocks,
                        snapshot,
                        rect,
                        pixels_per_point,
                        self.coloring,
                        self.palette,
                        self.style,
                        &self.directory_colors,
                    )));
                    *texture_style = Some(key);
                }
                super::treemap_gpu::paint(
                    &painter,
                    rect,
                    format,
                    slot,
                    self.gpu_tiles[slot].as_ref().unwrap().clone(),
                );
            } else if self.detail == MapDetail::Files {
                let key = (self.palette, self.coloring, self.style);
                if *texture_style != Some(key) {
                    let image = rasterize_files(
                        layer_blocks,
                        snapshot,
                        rect,
                        pixels_per_point,
                        self.coloring,
                        self.palette,
                        self.style,
                        &self.directory_colors,
                    );
                    if let Some(texture) = texture.as_mut() {
                        texture.set(image, eframe::egui::TextureOptions::NEAREST);
                    } else {
                        *texture = Some(ui.ctx().load_texture(
                            texture_name,
                            image,
                            eframe::egui::TextureOptions::NEAREST,
                        ));
                    }
                    *texture_style = Some(key);
                }
                if let Some(texture) = texture.as_ref() {
                    painter.image(
                        texture.id(),
                        rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
            // GPU Batching: Consolidate static blocks into exactly ONE single mesh submission to the GPU
            let mut combined_mesh = eframe::egui::Mesh::default();
            for block in layer_blocks {
                if block.is_frame || (self.detail == MapDetail::Files && raster_tile(block)) {
                    continue;
                }
                let mut render_rect = block.rect;
                let mut draw_border = false;

                if (self.draw_borders || self.detail == MapDetail::Files)
                    && block.rect.width() > 2.0 / pixels_per_point
                    && block.rect.height() > 2.0 / pixels_per_point
                {
                    draw_border = true;
                    render_rect = block.rect.shrink(if self.detail == MapDetail::Files {
                        1.0 / pixels_per_point
                    } else {
                        1.0
                    });
                }

                let fill_color = block_color(
                    block,
                    snapshot,
                    self.coloring,
                    self.palette,
                    &self.directory_colors,
                );
                if draw_border {
                    let border_color = if self.detail == MapDetail::Files {
                        file_border_color(fill_color)
                    } else {
                        colors::get_treemap_border_color()
                    };
                    let base_vertex_idx = combined_mesh.vertices.len() as u32;

                    combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                        pos: block.rect.left_top(),
                        uv: eframe::egui::epaint::WHITE_UV,
                        color: border_color,
                    });
                    combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                        pos: block.rect.right_top(),
                        uv: eframe::egui::epaint::WHITE_UV,
                        color: border_color,
                    });
                    combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                        pos: block.rect.right_bottom(),
                        uv: eframe::egui::epaint::WHITE_UV,
                        color: border_color,
                    });
                    combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                        pos: block.rect.left_bottom(),
                        uv: eframe::egui::epaint::WHITE_UV,
                        color: border_color,
                    });

                    combined_mesh.add_triangle(
                        base_vertex_idx,
                        base_vertex_idx + 1,
                        base_vertex_idx + 2,
                    );
                    combined_mesh.add_triangle(
                        base_vertex_idx,
                        base_vertex_idx + 2,
                        base_vertex_idx + 3,
                    );
                }

                let color_light = scale_rgb(fill_color, 1.0 + 0.15 * block.transition);
                let color_dark = scale_rgb(fill_color, 1.0 - 0.25 * block.transition);

                let base_vertex_idx = combined_mesh.vertices.len() as u32;

                match self.style {
                    TreemapStyle::Bevel
                    | TreemapStyle::Glass
                    | TreemapStyle::Neon
                    | TreemapStyle::Outline => {
                        let edge = render_rect.width().min(render_rect.height()).min(2.0) * 0.5;
                        let inner = render_rect.shrink(edge);
                        let border = if self.style == TreemapStyle::Bevel {
                            color_light
                        } else {
                            fill_color
                        };
                        combined_mesh.add_colored_rect(render_rect, border);
                        let center = match self.style {
                            TreemapStyle::Neon => {
                                scale_rgb(fill_color, 1.0 - 0.78 * block.transition)
                            }
                            TreemapStyle::Outline => {
                                mix_color(fill_color, Color32::from_gray(18), block.transition)
                            }
                            _ => color_dark,
                        };
                        combined_mesh.add_colored_rect(inner, center);
                        if self.style == TreemapStyle::Glass {
                            let shine =
                                Rect::from_min_max(inner.min, pos2(inner.max.x, inner.center().y));
                            combined_mesh.add_colored_rect(shine, color_light);
                        }
                    }
                    TreemapStyle::Flat => combined_mesh.add_colored_rect(render_rect, fill_color),
                    TreemapStyle::VerticalGradient => {
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.left_top(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_light,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.right_top(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_light,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.right_bottom(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_dark,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.left_bottom(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_dark,
                        });

                        combined_mesh.add_triangle(
                            base_vertex_idx,
                            base_vertex_idx + 1,
                            base_vertex_idx + 2,
                        );
                        combined_mesh.add_triangle(
                            base_vertex_idx,
                            base_vertex_idx + 2,
                            base_vertex_idx + 3,
                        );
                    }
                    TreemapStyle::OffsetVerticalGradient => {
                        let left_light = color_light;
                        let right_light = scale_rgb(color_light, 1.0 - 0.08 * block.transition);
                        let left_dark = scale_rgb(color_dark, 1.0 + 0.08 * block.transition);
                        let right_dark = color_dark;

                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.left_top(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: left_light,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.right_top(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: right_light,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.right_bottom(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: right_dark,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.left_bottom(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: left_dark,
                        });

                        combined_mesh.add_triangle(
                            base_vertex_idx,
                            base_vertex_idx + 1,
                            base_vertex_idx + 2,
                        );
                        combined_mesh.add_triangle(
                            base_vertex_idx,
                            base_vertex_idx + 2,
                            base_vertex_idx + 3,
                        );
                    }
                    TreemapStyle::DiagonalGradient => {
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.left_top(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_light,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.right_top(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: fill_color,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.right_bottom(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_dark,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.left_bottom(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: fill_color,
                        });

                        combined_mesh.add_triangle(
                            base_vertex_idx,
                            base_vertex_idx + 1,
                            base_vertex_idx + 2,
                        );
                        combined_mesh.add_triangle(
                            base_vertex_idx,
                            base_vertex_idx + 2,
                            base_vertex_idx + 3,
                        );
                    }
                    TreemapStyle::Cushion => {
                        let center = pos2(
                            render_rect.width().mul_add(0.4, render_rect.left()),
                            render_rect.height().mul_add(0.4, render_rect.top()),
                        );

                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.left_top(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_dark,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.right_top(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_dark,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.right_bottom(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_dark,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: render_rect.left_bottom(),
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_dark,
                        });
                        combined_mesh.vertices.push(eframe::egui::epaint::Vertex {
                            pos: center,
                            uv: eframe::egui::epaint::WHITE_UV,
                            color: color_light,
                        });

                        combined_mesh.add_triangle(
                            base_vertex_idx,
                            base_vertex_idx + 1,
                            base_vertex_idx + 4,
                        );
                        combined_mesh.add_triangle(
                            base_vertex_idx + 1,
                            base_vertex_idx + 2,
                            base_vertex_idx + 4,
                        );
                        combined_mesh.add_triangle(
                            base_vertex_idx + 2,
                            base_vertex_idx + 3,
                            base_vertex_idx + 4,
                        );
                        combined_mesh.add_triangle(
                            base_vertex_idx + 3,
                            base_vertex_idx,
                            base_vertex_idx + 4,
                        );
                    }
                }
            }

            painter.add(combined_mesh);
            for block in layer_blocks {
                if !block.is_frame
                    || self.layout != MapLayout::Folders
                    || block.header_transition <= 0.0
                    || (self.animation != MapAnimation::Soft
                        && (block.rect.width() < 70.0 || block.rect.height() < 42.0))
                {
                    continue;
                }
                let header = Rect::from_min_size(
                    block.rect.min,
                    eframe::egui::vec2(block.rect.width(), 18.0 * block.header_transition),
                );
                painter.rect_filled(
                    header,
                    0.0,
                    mix_color(
                        block_color(
                            block,
                            snapshot,
                            self.coloring,
                            self.palette,
                            &self.directory_colors,
                        ),
                        Color32::from_rgb(45, 49, 55),
                        block.header_transition,
                    ),
                );
                if self.detail != MapDetail::Files {
                    painter.rect_stroke(
                        block.rect,
                        0.0,
                        eframe::egui::Stroke::new(
                            block.header_transition / pixels_per_point,
                            Color32::from_gray(100).gamma_multiply(block.header_transition),
                        ),
                        eframe::egui::StrokeKind::Inside,
                    );
                }
                let node = &snapshot.nodes[block.node_idx as usize];
                let name = snapshot.string_pool.get(node.name_id).unwrap_or("");
                let label = format!(
                    "{} ({})",
                    name,
                    prettier_bytes::ByteFormatter::new().format(node.size)
                );
                painter.with_clip_rect(header.intersect(clip)).text(
                    header.left_top() + eframe::egui::vec2(4.0, 2.0),
                    eframe::egui::Align2::LEFT_TOP,
                    label,
                    eframe::egui::FontId::proportional(11.0),
                    Color32::from_gray(230).gamma_multiply(block.header_transition),
                );
            }

            if self.detail == MapDetail::Files {
                let mut outlines = eframe::egui::Mesh::default();
                for block in layer_blocks.iter().filter(|b| b.is_frame) {
                    let fill = block_color(
                        block,
                        snapshot,
                        self.coloring,
                        self.palette,
                        &self.directory_colors,
                    );
                    append_file_border(
                        &mut outlines,
                        block.rect,
                        pixels_per_point,
                        file_border_color(fill),
                    );
                }
                painter.add(outlines);
            }
        }

        let mut hovered_block = None;
        let hover_pos = response.hover_pos();

        // Look up hovered block (O(M) linear search is fast on layout blocks in Rust)
        if let Some(pos) = hover_pos {
            for block in self.display_blocks.iter().rev() {
                if block.rect.contains(pos) {
                    hovered_block = Some(block);
                    break;
                }
            }
        }

        // Empty directory indicator when zoomed into an empty folder
        if self.display_blocks.is_empty()
            && self.zoom_root != 0
            && (self.zoom_root as usize) < snapshot.nodes.len()
        {
            let name = snapshot
                .string_pool
                .get(snapshot.nodes[self.zoom_root as usize].name_id)
                .unwrap_or("");
            painter.text(
                rect.center(),
                eframe::egui::Align2::CENTER_CENTER,
                format!("📁 {name} ({})", t!("zoom-empty-dir")),
                eframe::egui::FontId::proportional(15.0),
                eframe::egui::Color32::GRAY,
            );
        }

        let painter = ui.painter_at(map_rect.intersect(rect));
        // Dynamic overlays for highlights
        if let Some(block) = hovered_block {
            let hover_stroke_color = if colors::get_current_theme() == colors::AppTheme::Light {
                eframe::egui::Color32::BLACK
            } else {
                colors::COLOR_WHITE
            };
            let stroke = eframe::egui::Stroke::new(1.5f32, hover_stroke_color);
            painter.rect(
                block.rect,
                0.0,
                colors::COLOR_TRANSPARENT,
                stroke,
                eframe::egui::StrokeKind::Inside,
            );
        }

        if !context.selected_nodes.is_empty() {
            if context.selected_nodes.len() == 1 {
                // Fast-path: Skip get_selection_roots entirely.
                // A single selected node is guaranteed to be its own selection root.
                if let Some(&root_idx) = context.selected_nodes.iter().next() {
                    let mut target_rect: Option<eframe::egui::Rect> = None;

                    for block in &self.display_blocks {
                        if is_descendant(&snapshot.nodes, block.node_idx, root_idx) {
                            match target_rect {
                                None => target_rect = Some(block.rect),
                                Some(ref mut r) => *r = r.union(block.rect),
                            }
                        }
                    }

                    if let Some(rect) = target_rect {
                        draw_glow(ui, &painter, rect);
                    }
                }
            } else {
                // Slow-path: Multiple selections.
                let roots = get_selection_roots(&snapshot.nodes, context.selected_nodes);
                let mut root_rects: std::collections::HashMap<
                    u32,
                    eframe::egui::Rect,
                    ahash::RandomState,
                > = std::collections::HashMap::with_hasher(ahash::RandomState::new());

                for block in &self.display_blocks {
                    let mut curr = Some(block.node_idx);
                    while let Some(idx) = curr {
                        if roots.contains(&idx) {
                            root_rects
                                .entry(idx)
                                .and_modify(|r| *r = r.union(block.rect))
                                .or_insert(block.rect);
                            break;
                        }
                        curr = snapshot
                            .nodes
                            .get(idx as usize)
                            .and_then(crate::arena::FileNode::parent_opt);
                    }
                }

                for (&_root_idx, &rect) in &root_rects {
                    draw_glow(ui, &painter, rect);
                }
            }
        }

        // Click event to select node
        if response.clicked()
            && let Some(block) = hovered_block
        {
            let modifiers = ui.input(|i| i.modifiers);
            if modifiers.command || modifiers.ctrl {
                if context.selected_nodes.contains(&block.node_idx) {
                    context.selected_nodes.remove(&block.node_idx);
                } else {
                    context.selected_nodes.insert(block.node_idx);
                }
            } else {
                // Normal click: toggle selection if already selected singly
                if context.selected_nodes.len() == 1
                    && context.selected_nodes.contains(&block.node_idx)
                {
                    context.selected_nodes.clear();
                } else {
                    context.selected_nodes.clear();
                    context.selected_nodes.insert(block.node_idx);
                }
            }
            *context.scroll_to_selected = true; // Raise scroll trigger

            // Auto expand parents so it shows up in tree view
            let mut curr = Some(block.node_idx);
            while let Some(idx) = curr {
                if let Some(node) = snapshot.nodes.get(idx as usize) {
                    if node.is_directory() {
                        context.expanded_nodes.insert(idx);
                    }
                    curr = node.parent_opt();
                } else {
                    break;
                }
            }
        }

        let mut pending_focus = None;
        // Double-click event to zoom into directory
        if response.double_clicked()
            && !navigating
            && let Some(block) = hovered_block
            && (block.node_idx as usize) < snapshot.nodes.len()
        {
            let clicked_node = &snapshot.nodes[block.node_idx as usize];
            let target = if clicked_node.is_directory() {
                block.node_idx
            } else {
                clicked_node.parent
            };
            if target != NO_INDEX && target != self.zoom_root {
                let source = self
                    .display_blocks
                    .iter()
                    .find(|b| b.node_idx == target && b.is_frame)
                    .map_or(block.rect, |b| b.rect);
                pending_focus = Some((target, source));
                context.selected_nodes.clear();
                context.selected_nodes.insert(target);
                *context.scroll_to_selected = true;

                let mut curr = Some(target);
                while let Some(idx) = curr {
                    if let Some(node) = snapshot.nodes.get(idx as usize) {
                        if node.is_directory() {
                            context.expanded_nodes.insert(idx);
                        }
                        curr = node.parent_opt();
                    } else {
                        break;
                    }
                }
                ui.ctx().request_repaint();
            }
        }

        // Capture clicked block and sync selection on right-click
        if response.secondary_clicked() {
            if let Some(block) = hovered_block {
                self.context_menu_target = Some(block.node_idx);
                context.selected_nodes.clear();
                context.selected_nodes.insert(block.node_idx);
                *context.scroll_to_selected = true;
            } else {
                self.context_menu_target = None;
            }
        }

        // Context menu on right-click (only open if an item is targeted or we are zoomed in)
        let has_context_menu = self.context_menu_target.is_some() || self.zoom_root != 0;
        if has_context_menu {
            let menu_response = response.context_menu(|ui| {
                ui.style_mut().wrap_mode = Some(eframe::egui::TextWrapMode::Extend);
                ui.set_min_width(180.0);

                if let Some(node_idx) = self.context_menu_target
                    && (node_idx as usize) < snapshot.nodes.len()
                {
                    #[cfg(not(target_family = "wasm"))]
                    {
                        if ui.button("Показать в проводнике").clicked() {
                            let path = snapshot.get_full_path(node_idx);
                            if let Err(error) = crate::gui::reveal::reveal_in_file_manager(
                                std::path::Path::new(&path),
                            ) {
                                crate::gui::toast_error(format!(
                                    "Не удалось открыть проводник: {error}"
                                ));
                            }
                            ui.close_kind(eframe::egui::UiKind::Menu);
                        }
                        if ui
                            .add_enabled(
                                self.allow_fs_actions,
                                eframe::egui::Button::new("Удалить…"),
                            )
                            .on_disabled_hover_text("Дождитесь окончания сканирования")
                            .clicked()
                        {
                            self.delete_request = Some(node_idx);
                            ui.close_kind(eframe::egui::UiKind::Menu);
                        }
                        ui.separator();
                    }
                    let clicked_node = &snapshot.nodes[node_idx as usize];
                    let target = if clicked_node.is_directory() {
                        node_idx
                    } else {
                        clicked_node.parent
                    };

                    if target != NO_INDEX {
                        let is_current = target == self.zoom_root;
                        let btn = eframe::egui::Button::new(t!("zoom-to-dir"));
                        if is_current {
                            ui.add_enabled(false, btn)
                                .on_disabled_hover_text(t!("toast-already-root"));
                        } else if ui.add(btn).clicked() {
                            self.zoom_root = target;
                            context.selected_nodes.clear();
                            context.selected_nodes.insert(target);
                            *context.scroll_to_selected = true;
                            ui.ctx().request_repaint();
                            ui.close_kind(eframe::egui::UiKind::Menu);
                        }
                    }
                }

                if ui.button("Сбросить масштаб").clicked() {
                    self.scale = 1.0;
                    self.target_scale = 1.0;
                    self.offset = eframe::egui::Vec2::ZERO;
                    ui.close_kind(eframe::egui::UiKind::Menu);
                }
                if self.zoom_root != 0 {
                    if ui.button(t!("zoom-up-level")).clicked() {
                        let parent = snapshot
                            .nodes
                            .get(self.zoom_root as usize)
                            .map_or(NO_INDEX, |n| n.parent);
                        self.zoom_root = if parent == NO_INDEX { 0 } else { parent };
                        context.selected_nodes.clear();
                        if self.zoom_root != 0 {
                            context.selected_nodes.insert(self.zoom_root);
                        }
                        *context.scroll_to_selected = true;
                        ui.ctx().request_repaint();
                        ui.close_kind(eframe::egui::UiKind::Menu);
                    }

                    if ui.button(t!("zoom-reset")).clicked() {
                        self.zoom_root = 0;
                        ui.ctx().request_repaint();
                        ui.close_kind(eframe::egui::UiKind::Menu);
                    }
                }
            });

            if menu_response.is_none() && !response.secondary_clicked() {
                self.context_menu_target = None;
            }
        }

        // Keyboard navigation shortcuts
        if (response.hovered() || response.has_focus()) && self.zoom_root != 0 {
            if ui.input(|i| i.key_pressed(eframe::egui::Key::Escape)) {
                self.zoom_root = 0;
                ui.ctx().request_repaint();
            } else if ui.input(|i| {
                i.key_pressed(eframe::egui::Key::Backspace)
                    || (i.modifiers.alt && i.key_pressed(eframe::egui::Key::ArrowUp))
            }) {
                let parent = snapshot
                    .nodes
                    .get(self.zoom_root as usize)
                    .map_or(NO_INDEX, |n| n.parent);
                self.zoom_root = if parent == NO_INDEX { 0 } else { parent };
                ui.ctx().request_repaint();
            }
        }

        // Draw tooltip
        if let Some(block) = hovered_block {
            let path_str = snapshot.get_full_path(block.node_idx);
            let size_str = prettier_bytes::ByteFormatter::new()
                .format(snapshot.nodes[block.node_idx as usize].size)
                .to_string();
            eframe::egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                eframe::egui::Id::new("treemap_tooltip"),
                eframe::egui::PopupAnchor::Pointer,
            )
            .show(|ui| {
                ui.style_mut().wrap_mode = Some(eframe::egui::TextWrapMode::Extend);
                let cleaned_path = crate::arena::clean_unc_path(&path_str);
                ui.label(format!("📁 {cleaned_path}"));
                ui.label(format!("💾 Size: {size_str}"));
            });
        }
        if let Some((target, source)) = pending_focus {
            self.enter_focus(target, source, rect, snapshot);
            ui.ctx().request_repaint();
        }
    }
}

fn draw_glow(ui: &eframe::egui::Ui, painter: &eframe::egui::Painter, rect: eframe::egui::Rect) {
    let time = ui.input(|i| i.time);
    let pulse = 0.5f64.mul_add((time * 6.0).sin(), 0.5);

    // 1. Draw Outer Expanding Glow (grows and fades)
    let glow_alpha = 0.20f64.mul_add(pulse, 0.1);
    let glow_color = colors::get_glow_outer_base().gamma_multiply(glow_alpha as f32);
    let glow_thickness = 6.0f32.mul_add(pulse as f32, 4.0); // Oscillates thickness
    painter.rect(
        rect,
        0.0,
        colors::COLOR_TRANSPARENT,
        eframe::egui::Stroke::new(glow_thickness, glow_color),
        eframe::egui::StrokeKind::Outside,
    );

    // 2. Draw Inner Sharp Contrast Core (stays crisp)
    let core_color = colors::get_glow_inner_core();
    let core_thickness = 1.0f32.mul_add(pulse as f32, 1.5);
    painter.rect(
        rect,
        0.0,
        colors::COLOR_TRANSPARENT,
        eframe::egui::Stroke::new(core_thickness, core_color),
        eframe::egui::StrokeKind::Inside,
    );
}

#[derive(Clone, Default)]
pub struct DirectoryCache {
    entries: std::collections::HashMap<u32, std::sync::Arc<SiblingLayout>>,
}
struct SiblingLayout {
    children: Vec<u32>,
    total: f64,
    rows: Vec<(usize, bool)>,
    prefix: Vec<f64>,
    last_directory: Option<usize>,
    directory_positions: Vec<usize>,
}

// Sorting and squarification topology depend on the snapshot, not on camera frames.
// Keep row membership and orientation stable when fixed-height headers change aspect.
fn sibling_layout(
    config: &TreemapConfig,
    node_idx: u32,
    rect: Rect,
) -> std::sync::Arc<SiblingLayout> {
    if let Some(cache) = config.layout_cache {
        if let Some(layout) = cache.borrow().entries.get(&node_idx) {
            return layout.clone();
        }
    }
    let mut children = Vec::new();
    let mut curr = config.nodes[node_idx as usize].first_child;
    let mut total = 0.0;
    while curr != NO_INDEX && (curr as usize) < config.nodes.len() {
        let node = &config.nodes[curr as usize];
        if node.size > 0 {
            children.push(curr);
            total += node.size as f64;
        }
        curr = node.next_sibling;
    }
    children.sort_unstable_by(|&a, &b| {
        config.nodes[b as usize]
            .size
            .cmp(&config.nodes[a as usize].size)
            .then(a.cmp(&b))
    });
    let directory_positions: Vec<_> = children
        .iter()
        .enumerate()
        .filter_map(|(position, &idx)| {
            config.nodes[idx as usize]
                .is_directory()
                .then_some(position)
        })
        .collect();
    let last_directory = directory_positions.last().copied();
    let mut prefix = Vec::with_capacity(children.len() + 1);
    prefix.push(0.0);
    for &idx in &children {
        prefix.push(prefix.last().unwrap() + config.nodes[idx as usize].size as f64);
    }
    let mut rows = Vec::new();
    let aspect = (rect.width() / rect.height().max(0.01)).max(0.0001) as f64;
    let mut width = (total * aspect).sqrt();
    let mut height = (total / aspect).sqrt();
    let mut i = 0;
    while i < children.len() && width > 0.0 && height > 0.0 {
        let size = |i: usize| config.nodes[children[i] as usize].size as f64;
        let side = width.min(height);
        let mut j = i + 1;
        let mut sum = size(i);
        let mut worst = sorted_aspect_ratio(sum, size(i), size(i), side);
        while j < children.len() {
            let next = sorted_aspect_ratio(sum + size(j), size(i), size(j), side);
            if next > worst {
                break;
            }
            worst = next;
            sum += size(j);
            j += 1;
        }
        let vertical = width >= height;
        rows.push((j, vertical));
        if vertical {
            width = (width - sum / height).max(0.0);
        } else {
            height = (height - sum / width).max(0.0);
        }
        i = j;
    }
    // Roundoff in a very long tail must never exclude the final siblings.
    if rows.last().is_some_and(|&(end, _)| end < children.len()) {
        rows.push((children.len(), width >= height));
    }
    let result = std::sync::Arc::new(SiblingLayout {
        children,
        total,
        rows,
        prefix,
        last_directory,
        directory_positions,
    });
    if let Some(cache) = config.layout_cache {
        cache.borrow_mut().entries.insert(node_idx, result.clone());
    }
    result
}

struct TreemapConfig<'a> {
    skip_subtree: Option<u32>,
    geometry_scale: f32,
    layout_cache: Option<&'a std::cell::RefCell<DirectoryCache>>,
    nodes: &'a [FileNode],
    string_pool: &'a StringPool,
    max_depth: usize,
    pixels_per_point: f32,
    directory_colors: &'a [Color32],
    layout: MapLayout,
    detail: MapDetail,
    clip: Rect,
}

#[cfg(test)]
fn worst_aspect_ratio(row: &[f64], w: f64) -> f64 {
    if row.is_empty() || w <= 0.0 {
        return f64::INFINITY;
    }
    let sum: f64 = row.iter().sum();
    if sum <= 0.0 {
        return f64::INFINITY;
    }
    let sum_sq = sum * sum;
    let w_sq = w * w;

    let mut max_ratio = 0.0;
    for &area in row {
        if area <= 0.0 {
            continue;
        }
        let ratio1 = (w_sq * area) / sum_sq;
        let ratio2 = sum_sq / (w_sq * area);
        let ratio = ratio1.max(ratio2);
        if ratio > max_ratio {
            max_ratio = ratio;
        }
    }
    max_ratio
}

// For descending positive areas, the worst item is one of the two extremes.
fn sorted_aspect_ratio(sum: f64, max: f64, min: f64, w: f64) -> f64 {
    let sum_sq = sum * sum;
    let w_sq = w * w;
    (w_sq * max / sum_sq).max(sum_sq / (w_sq * min))
}

fn folder_color(config: &TreemapConfig, idx: u32) -> Color32 {
    config
        .directory_colors
        .get(idx as usize)
        .copied()
        .unwrap_or_else(|| {
            colors::get_treemap_dir_color(
                config
                    .string_pool
                    .get(config.nodes[idx as usize].name_id)
                    .unwrap_or(""),
            )
        })
}

fn recurse_child(
    config: &TreemapConfig,
    child_idx: u32,
    child_rect: Rect,
    depth: usize,
    blocks: &mut Vec<TreemapBlock>,
) {
    if config.skip_subtree == Some(child_idx) {
        return;
    }
    let min_dim = if config.detail == MapDetail::Files {
        MIN_FILE_PIXELS / config.pixels_per_point.max(0.01)
    } else {
        PIXEL_PRECISION_LIMIT
    };
    if !child_rect.intersects(config.clip)
        || child_rect.width() + 0.0001 < min_dim
        || child_rect.height() + 0.0001 < min_dim
    {
        return; // Discard sub-pixel visual artifacts early
    }

    let child = &config.nodes[child_idx as usize];

    let is_leaf_or_too_small = !child.is_directory()
        || depth >= config.max_depth
        || (config.detail == MapDetail::Folders
            && (child_rect.width() < 12.0 || child_rect.height() < 12.0));

    if is_leaf_or_too_small {
        let name = config.string_pool.get(child.name_id).unwrap_or("");
        let color = if child.is_directory() {
            folder_color(config, child_idx)
        } else {
            let ext = crate::arena::get_ext_slice(name);
            colors::get_color_for_extension(ext)
        };
        blocks.push(TreemapBlock {
            rect: child_rect,
            node_idx: child_idx,
            color,
            is_frame: false,
            transition: 1.0,
            header_transition: 1.0,
        });
        return;
    }

    build_treemap(config, child_idx, child_rect, depth + 1, blocks);
}

/// Walks up the parent chain of a node to determine if it is a descendant of a target ancestor.
#[must_use]
pub fn is_descendant(nodes: &[FileNode], child_idx: u32, ancestor_idx: u32) -> bool {
    let mut curr = Some(child_idx);
    while let Some(idx) = curr {
        if idx == ancestor_idx {
            return true;
        }
        if let Some(node) = nodes.get(idx as usize) {
            curr = node.parent_opt();
        } else {
            break;
        }
    }
    false
}

fn build_treemap(
    config: &TreemapConfig,
    node_idx: u32,
    rect: Rect,
    depth: usize,
    blocks: &mut Vec<TreemapBlock>,
) {
    if !rect.intersects(config.clip) || node_idx as usize >= config.nodes.len() {
        return;
    }

    if config.skip_subtree == Some(node_idx) {
        return;
    }

    // Give every visible region its share of the GPU budget. Collapse a complete
    // subtree rather than dropping subsequent siblings and leaving black holes.
    let start = blocks.len();
    build_treemap_inner(config, node_idx, rect, depth, blocks);
    let visible = rect.intersect(config.clip).area().max(0.0);
    let viewport = if config.clip.area().is_finite() {
        config.clip.area().max(1.0)
    } else {
        rect.area().max(1.0)
    };
    let budget = ((TRUNCATE_DEPTH as f32 * visible / viewport) as usize).clamp(1, TRUNCATE_DEPTH);
    if config.detail == MapDetail::Folders && blocks.len() - start > budget {
        blocks.truncate(start);
        blocks.push(TreemapBlock {
            rect,
            node_idx,
            color: folder_color(config, node_idx),
            is_frame: false,
            transition: 1.0,
            header_transition: 1.0,
        });
    }
}

// Fill a dense remainder without allocating or visiting objects which cannot fit.
// Every cell has at least one content pixel plus its eight border neighbours.
fn fill_dense_tail(
    config: &TreemapConfig,
    children: &[u32],
    rect: Rect,
    depth: usize,
    blocks: &mut Vec<TreemapBlock>,
) {
    let min = MIN_FILE_PIXELS / config.pixels_per_point.max(0.01);
    let max_cols = (rect.width() / min).floor() as usize;
    let max_rows = (rect.height() / min).floor() as usize;
    if max_cols == 0 || max_rows == 0 {
        return;
    }
    let count = children.len().min(max_cols.saturating_mul(max_rows));
    if count == 0 {
        return;
    }
    let cols = ((count as f32 * rect.width() / rect.height()).sqrt().ceil() as usize)
        .max(count.div_ceil(max_rows))
        .min(max_cols)
        .min(count);
    let rows = count.div_ceil(cols);
    let visible = rect.intersect(config.clip);
    if !visible.is_positive() {
        return;
    }
    let first_row = (((visible.top() - rect.top()) / rect.height()) * rows as f32)
        .floor()
        .max(0.0) as usize;
    let last_row = ((((visible.bottom() - rect.top()) / rect.height()) * rows as f32).ceil()
        as usize)
        .min(rows);
    for row in first_row..last_row {
        let start = row * cols;
        let row_count = cols.min(count - start);
        let first_col = (((visible.left() - rect.left()) / rect.width()) * row_count as f32)
            .floor()
            .max(0.0) as usize;
        let last_col =
            ((((visible.right() - rect.left()) / rect.width()) * row_count as f32).ceil() as usize)
                .min(row_count);
        for col in first_col..last_col {
            let cell = Rect::from_min_max(
                pos2(
                    rect.left() + rect.width() * col as f32 / row_count as f32,
                    rect.top() + rect.height() * row as f32 / rows as f32,
                ),
                pos2(
                    rect.left() + rect.width() * (col + 1) as f32 / row_count as f32,
                    rect.top() + rect.height() * (row + 1) as f32 / rows as f32,
                ),
            );
            recurse_child(config, children[start + col], cell, depth, blocks);
        }
    }
}

fn build_treemap_inner(
    config: &TreemapConfig,
    node_idx: u32,
    rect: Rect,
    depth: usize,
    blocks: &mut Vec<TreemapBlock>,
) {
    // A stale or corrupt index (e.g. a cached layout referencing indices
    // from a snapshot that shrank after a full re-scan) must never panic
    // the render loop — bail out of this branch instead.
    if node_idx as usize >= config.nodes.len() {
        return;
    }

    let node = &config.nodes[node_idx as usize];
    let min_dim = if config.detail == MapDetail::Files {
        MIN_FILE_PIXELS / config.pixels_per_point.max(0.01)
    } else {
        PIXEL_PRECISION_LIMIT
    };
    if node.size == 0 || rect.width() + 0.0001 < min_dim || rect.height() + 0.0001 < min_dim {
        return;
    }

    if !node.is_directory() || depth >= config.max_depth {
        let name = config.string_pool.get(node.name_id).unwrap_or("");
        let color = if node.is_directory() {
            folder_color(config, node_idx)
        } else {
            let ext = crate::arena::get_ext_slice(name);
            colors::get_color_for_extension(ext)
        };

        blocks.push(TreemapBlock {
            rect,
            node_idx,
            color,
            is_frame: false,
            transition: 1.0,
            header_transition: 1.0,
        });
        return;
    }

    let geometry_scale = config.geometry_scale.max(0.0001);
    let base_size = rect.size() / geometry_scale;
    let reference_header =
        config.layout == MapLayout::Folders && base_size.x >= 70.0 && base_size.y >= 42.0;
    let has_header =
        config.layout == MapLayout::Folders && rect.width() >= 70.0 && rect.height() >= 42.0;
    // Keep the established outer geometry. A label that becomes visible with
    // zoom still needs its own 18-point strip plus a margin inside that frame.
    let side = if reference_header {
        2.0 * geometry_scale
    } else if has_header {
        2.0 / config.pixels_per_point.max(0.01)
    } else {
        0.0
    };
    let top = (if reference_header {
        20.0 * geometry_scale
    } else {
        0.0f32
    })
    .max(if has_header { 20.0 } else { 0.0 });
    let content_rect = Rect::from_min_max(
        rect.min + eframe::egui::vec2(side, top),
        rect.max - eframe::egui::vec2(side, side),
    );
    let siblings = sibling_layout(config, node_idx, content_rect);
    let children = &siblings.children;
    let child_total_size = siblings.total;

    // Only folder view aggregates dense directories. File view always lays out
    // their actual files, with tiny tiles rasterized instead of collapsed.
    if children.is_empty()
        || (config.detail == MapDetail::Folders
            && (rect.area() / (children.len().max(1) as f32) < 16.0
                || (config.layout == MapLayout::Stripes
                    && (if depth % 2 == 0 {
                        rect.width()
                    } else {
                        rect.height()
                    }) / (children.len().max(1) as f32)
                        < PIXEL_PRECISION_LIMIT)))
    {
        let color = folder_color(config, node_idx);
        blocks.push(TreemapBlock {
            rect,
            node_idx,
            color,
            is_frame: false,
            transition: 1.0,
            header_transition: 1.0,
        });
        return;
    }

    if has_header || config.detail == MapDetail::Files {
        blocks.push(TreemapBlock {
            rect,
            node_idx,
            color: folder_color(config, node_idx),
            is_frame: true,
            transition: 1.0,
            header_transition: 1.0,
        });
    }
    let rect = content_rect;

    let active_children = &siblings.children;

    if active_children.is_empty() {
        return;
    }

    let total_size = child_total_size;

    if total_size == 0.0 {
        return;
    }

    if config.layout == MapLayout::Stripes {
        let horizontal = depth % 2 == 0;
        let mut offset = if horizontal { rect.left() } else { rect.top() } as f64;
        for (item, &idx) in active_children.iter().enumerate() {
            let fraction = config.nodes[idx as usize].size as f64 / total_size;
            let mut child_rect = rect;
            if horizontal {
                child_rect.min.x = offset as f32;
                offset += rect.width() as f64 * fraction;
                child_rect.max.x = (offset as f32).min(rect.right());
            } else {
                child_rect.min.y = offset as f32;
                offset += rect.height() as f64 * fraction;
                child_rect.max.y = (offset as f32).min(rect.bottom());
            }
            if config.detail == MapDetail::Files && siblings.last_directory.is_none() {
                let end = if horizontal {
                    rect.right()
                } else {
                    rect.bottom()
                };
                let begin = if horizontal {
                    child_rect.left()
                } else {
                    child_rect.top()
                };
                if end - begin < min_dim {
                    break;
                }
                let proposed = (offset as f32).max(begin + min_dim);
                let edge = if end - proposed < min_dim || item + 1 == active_children.len() {
                    end
                } else {
                    proposed
                };
                offset = edge as f64;
                if horizontal {
                    child_rect.max.x = edge;
                } else {
                    child_rect.max.y = edge;
                }
            }
            recurse_child(config, idx, child_rect, depth, blocks);
        }
        return;
    }

    let total_area = (rect.width() * rect.height()) as f64;
    let area_scale = total_area / total_size;
    let child_area = |i: usize| config.nodes[active_children[i] as usize].size as f64 * area_scale;

    // The old guard disabled minimum-pixel packing whenever even one tiny
    // directory remained among the files. Preserve sizeable folder rectangles,
    // but allow the tiny mixed tail to use the same complete minimum-size grid.
    let dense_tail_allowed = |start: usize| {
        let position = siblings
            .directory_positions
            .partition_point(|&index| index < start);
        siblings
            .directory_positions
            .get(position)
            .is_none_or(|&index| child_area(index) < (70.0 * 42.0))
    };

    let mut remaining_rect = rect;
    let mut i = 0;

    for &(j, vertical_layout) in &siblings.rows {
        if remaining_rect.width() <= 0.0 || remaining_rect.height() <= 0.0 {
            break;
        }
        let row_sum = (siblings.prefix[j] - siblings.prefix[i]) * area_scale;
        if config.detail == MapDetail::Files {
            let side = if vertical_layout {
                remaining_rect.height()
            } else {
                remaining_rect.width()
            };
            let thickness = row_sum / side as f64;
            let smallest = side as f64 * child_area(j - 1) / row_sum;
            if (thickness < min_dim as f64 || smallest < min_dim as f64) && dense_tail_allowed(i) {
                fill_dense_tail(config, &active_children[i..], remaining_rect, depth, blocks);
                return;
            }
        }

        if vertical_layout {
            let h = remaining_rect.height() as f64;
            let mut thickness = if h > 0.0 { row_sum / h } else { 0.0 };
            if config.detail == MapDetail::Files
                && dense_tail_allowed(i)
                && remaining_rect.width() - (thickness as f32) < min_dim
            {
                thickness = remaining_rect.width() as f64;
            }
            let mut current_y = remaining_rect.min.y;

            for k in i..j {
                let area = child_area(k);
                let child_idx = active_children[k];
                let item_height = if row_sum > 0.0 {
                    h * (area / row_sum)
                } else {
                    0.0
                };

                let child_rect = Rect::from_min_max(
                    pos2(remaining_rect.min.x, current_y),
                    pos2(
                        (remaining_rect.min.x + thickness as f32).min(remaining_rect.max.x),
                        (current_y + item_height as f32).min(remaining_rect.max.y),
                    ),
                );

                recurse_child(config, child_idx, child_rect, depth, blocks);
                current_y += item_height as f32;
            }

            remaining_rect.min.x =
                (remaining_rect.min.x + thickness as f32).min(remaining_rect.max.x);
        } else {
            let width = remaining_rect.width() as f64;
            let mut thickness = if width > 0.0 { row_sum / width } else { 0.0 };
            if config.detail == MapDetail::Files
                && dense_tail_allowed(i)
                && remaining_rect.height() - (thickness as f32) < min_dim
            {
                thickness = remaining_rect.height() as f64;
            }
            let mut current_x = remaining_rect.min.x;

            for k in i..j {
                let area = child_area(k);
                let child_idx = active_children[k];
                let item_width = if row_sum > 0.0 {
                    width * (area / row_sum)
                } else {
                    0.0
                };

                let child_rect = Rect::from_min_max(
                    pos2(current_x, remaining_rect.min.y),
                    pos2(
                        (current_x + item_width as f32).min(remaining_rect.max.x),
                        (remaining_rect.min.y + thickness as f32).min(remaining_rect.max.y),
                    ),
                );

                recurse_child(config, child_idx, child_rect, depth, blocks);
                current_x += item_width as f32;
            }

            remaining_rect.min.y =
                (remaining_rect.min.y + thickness as f32).min(remaining_rect.max.y);
        }

        i = j;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use eframe::egui::Rect;

    use super::*;

    #[test]
    fn focus_reflows_skinny_folders_for_the_destination_aspect_and_preserves_parent_rows() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"folder");
        let file = pool.get_or_insert(b"file.bin");
        let mut nodes = vec![
            FileNode::new(name, None, true, false, 0, 0),
            FileNode::new(name, Some(0), true, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[0].size = 1600;
        nodes[1].first_child = 2;
        nodes[1].size = 1600;
        for i in 0..16 {
            let mut node = FileNode::new(file, Some(1), false, false, 0, 0);
            node.size = 100;
            if i < 15 {
                node.next_sibling = nodes.len() as u32 + 1;
            }
            nodes.push(node);
        }
        let snapshot = FileArenaSnapshot {
            dir_counts: Arc::new(precompute_dir_counts(&nodes)),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            extensions: Default::default(),
        };
        let aspect = |blocks: &[TreemapBlock]| {
            blocks
                .iter()
                .filter(|b| !b.is_frame)
                .map(|b| (b.rect.width() / b.rect.height()).max(b.rect.height() / b.rect.width()))
                .fold(1.0f32, f32::max)
        };
        for detail in [MapDetail::Folders, MapDetail::Files] {
            for (source_size, target_size) in [
                (
                    eframe::egui::vec2(100.0, 500.0),
                    eframe::egui::vec2(1600.0, 600.0),
                ),
                (
                    eframe::egui::vec2(500.0, 50.0),
                    eframe::egui::vec2(600.0, 1000.0),
                ),
            ] {
                let source = Rect::from_min_size(pos2(10.0, 20.0), source_size);
                let viewport = Rect::from_min_size(pos2(10.0, 20.0), target_size);
                let mut chart = TreemapChart::new();
                chart.detail = detail;
                fn config<'a>(
                    snapshot: &'a crate::arena::FileArenaSnapshot,
                    detail: MapDetail,
                    cache: Option<&'a std::cell::RefCell<DirectoryCache>>,
                ) -> TreemapConfig<'a> {
                    TreemapConfig {
                        skip_subtree: None,
                        layout_cache: cache,
                        geometry_scale: 1.0,
                        nodes: &snapshot.nodes,
                        string_pool: &snapshot.string_pool,
                        max_depth: 128,
                        pixels_per_point: 1.0,
                        directory_colors: &[],
                        layout: MapLayout::Folders,
                        detail,
                        clip: Rect::EVERYTHING,
                    }
                }
                build_treemap(
                    &config(&snapshot, detail, Some(&chart.directory_cache)),
                    1,
                    source,
                    1,
                    &mut Vec::new(),
                );
                let old_rows = chart
                    .directory_cache
                    .borrow()
                    .entries
                    .get(&1)
                    .unwrap()
                    .clone();
                let mut stretched = Vec::new();
                build_treemap(
                    &config(&snapshot, detail, Some(&chart.directory_cache)),
                    1,
                    viewport,
                    1,
                    &mut stretched,
                );
                assert!(
                    aspect(&stretched) > 5.0,
                    "fixture must reproduce the stretched old rows"
                );
                chart.enter_focus(1, source, viewport, &snapshot);
                assert!(Arc::ptr_eq(
                    &old_rows,
                    chart.focus_history[0]
                        .parent_cache
                        .borrow()
                        .entries
                        .get(&1)
                        .unwrap()
                ));
                assert!(!Arc::ptr_eq(
                    &old_rows,
                    chart.directory_cache.borrow().entries.get(&1).unwrap()
                ));
                let mut focused = Vec::new();
                let mut expected = Vec::new();
                build_treemap(
                    &config(&snapshot, detail, Some(&chart.directory_cache)),
                    1,
                    viewport,
                    1,
                    &mut focused,
                );
                build_treemap(
                    &config(&snapshot, detail, None),
                    1,
                    viewport,
                    1,
                    &mut expected,
                );
                assert!(
                    aspect(&focused) < 2.5,
                    "destination tiles should be compact, not stretched: {}",
                    aspect(&focused)
                );
                assert_eq!(focused.len(), expected.len());
                for (actual, expected) in focused.iter().zip(&expected) {
                    assert_eq!(actual.node_idx, expected.node_idx);
                    assert_eq!(actual.rect, expected.rect);
                    assert!(viewport.expand(0.01).contains_rect(actual.rect));
                }
                let saved = chart.focus_history.pop().unwrap().parent_cache.into_inner();
                *chart.directory_cache.borrow_mut() = saved;
                let mut restored = Vec::new();
                let mut original = Vec::new();
                build_treemap(
                    &config(&snapshot, detail, Some(&chart.directory_cache)),
                    1,
                    source,
                    1,
                    &mut restored,
                );
                build_treemap(
                    &config(&snapshot, detail, None),
                    1,
                    source,
                    1,
                    &mut original,
                );
                assert_eq!(restored.len(), original.len());
                for (a, b) in restored.iter().zip(original) {
                    assert_eq!(a.rect, b.rect);
                }
            }
        }
    }

    #[test]
    fn automatic_entry_accelerates_gently_and_does_not_overshoot() {
        let source = Rect::from_min_max(pos2(0.2, 0.2), pos2(0.4, 0.6));
        let mut drive = 0.0;
        let mut position = 0.0;
        let first_wheel = focus_motion_step(0.0, 1.0, source, 1.0 / 60.0);
        for frame in 0..120 {
            drive = focus_motion_step(drive, 1.0, source, 1.0 / 60.0);
            let next = focus_motion_step(position, drive, source, 1.0 / 60.0);
            assert!(next >= position && next <= 1.0);
            if frame == 0 {
                assert!(next < first_wheel * 0.3);
            }
            position = next;
        }
        assert_eq!(position, 1.0);
        assert_eq!(drive, 1.0);
    }

    #[test]
    fn focus_uses_wheel_ease_and_uniformly_moves_the_surrounding_scene() {
        let viewport = Rect::from_min_size(pos2(10.0, 20.0), eframe::egui::vec2(800.0, 500.0));
        let source = Rect::from_min_max(pos2(0.2, 0.2), pos2(0.4, 0.6));
        let sibling = Rect::from_min_size(pos2(400.0, 100.0), eframe::egui::vec2(100.0, 80.0));
        assert_eq!(focus_camera_rect(viewport, source, 0.0, sibling), sibling);
        assert_eq!(focus_rect(viewport, source, 1.0), viewport);
        let mut progress = 0.0;
        let mut ordinary_zoom = 1.0;
        let fit = focus_fit(source);
        for _ in 0..15 {
            progress = focus_motion_step(progress, 1.0, source, 1.0 / 60.0);
            ordinary_zoom = inertial_camera_step(ordinary_zoom, fit, 1.0 / 60.0);
            let moved = focus_camera_rect(viewport, source, progress, sibling);
            // The same normalized speed curve, with one uniform scale for every
            // sibling; no static screenshot behind an independently growing tile.
            assert!((moved.width() / sibling.width() - ordinary_zoom).abs() < 0.0001);
            assert!(
                (moved.width() / moved.height() - sibling.width() / sibling.height()).abs()
                    < 0.0001
            );
            assert_ne!(moved.center(), sibling.center());
        }
        let partial = inertial_camera_step(progress, 0.4, 1.0 / 60.0);
        assert!(partial < progress && partial > 0.4);
    }

    #[test]
    fn double_click_focus_morphs_and_ctrl_wheel_restores_parent_camera() {
        use eframe::egui::{Event, Modifiers, PointerButton, RawInput};
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"root");
        let mut nodes = vec![
            FileNode::new(name, None, true, false, 0, 0),
            FileNode::new(name, Some(0), true, false, 0, 0),
            FileNode::new(name, Some(0), true, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[1].next_sibling = 2;
        for dir in [1, 2] {
            nodes[dir].first_child = nodes.len() as u32;
            for n in 0..20 {
                let mut file = FileNode::new(name, Some(dir as u32), false, false, 0, 0);
                file.size = 100;
                if n < 19 {
                    file.next_sibling = nodes.len() as u32 + 1;
                }
                nodes.push(file);
                nodes[dir].size += 100;
                nodes[0].size += 100;
            }
        }
        let snapshot = FileArenaSnapshot {
            dir_counts: Arc::new(precompute_dir_counts(&nodes)),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            extensions: Default::default(),
        };
        let ctx = eframe::egui::Context::default();
        let mut chart = TreemapChart::new();
        chart.detail = MapDetail::Files;
        let mut selected = Default::default();
        let mut expanded = Default::default();
        let mut scroll = false;
        let mut context = StatContext {
            selected_nodes: &mut selected,
            expanded_nodes: &mut expanded,
            scroll_to_selected: &mut scroll,
            deduplicator_results: None,
        };
        let screen = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(800.0, 500.0));
        let mut frame = |chart: &mut TreemapChart, time: f64, events: Vec<Event>| {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.input_mut(|i| i.smooth_scroll_delta = eframe::egui::Vec2::ZERO);
                    chart.render(ui, &snapshot, &mut context);
                },
            )
        };
        frame(&mut chart, 0.0, vec![]);
        chart.scale = 1.5;
        chart.target_scale = 1.5;
        chart.offset = eframe::egui::vec2(-60.0, -30.0);
        frame(&mut chart, 0.016, vec![]);
        let parent_camera = (chart.scale, chart.offset);
        let source = chart
            .display_blocks
            .iter()
            .find(|b| b.node_idx == 1 && b.is_frame)
            .unwrap()
            .rect;
        let pointer = source.intersect(screen.shrink(20.0)).center();
        for (i, pressed) in [true, false, true, false].into_iter().enumerate() {
            frame(
                &mut chart,
                0.032 + i as f64 * 0.04,
                vec![
                    Event::PointerMoved(pointer),
                    Event::PointerButton {
                        pos: pointer,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::default(),
                    },
                ],
            );
        }
        assert_eq!(chart.zoom_root, 1);
        assert_eq!(chart.focus_history.len(), 1);
        frame(&mut chart, 0.184, vec![]);
        let mut width = chart.last_rect.width();
        for i in 0..45 {
            frame(&mut chart, 0.20 + i as f64 / 60.0, vec![]);
            assert!(chart.last_rect.width() >= width - 0.01);
            width = chart.last_rect.width();
        }
        assert_eq!(chart.focus_history[0].progress, 1.0);
        let viewport = chart.last_rect;
        assert!((viewport.width() - (screen.width() - 20.0)).abs() < 20.0);
        frame(
            &mut chart,
            1.0,
            vec![
                Event::PointerMoved(viewport.center()),
                Event::ModifiersChanged(Modifiers {
                    ctrl: true,
                    ..Default::default()
                }),
                Event::MouseWheel {
                    unit: eframe::egui::MouseWheelUnit::Line,
                    delta: eframe::egui::vec2(0.0, -3.0),
                    phase: eframe::egui::TouchPhase::Move,
                    modifiers: Modifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                },
            ],
        );
        assert!(chart.focus_history[0].target_progress < 1.0);
        assert!(chart.focus_history[0].target_progress > 0.0);
        for i in 0..30 {
            frame(&mut chart, 1.016 + i as f64 / 60.0, vec![]);
        }
        assert_eq!(chart.zoom_root, 1);
        assert!(chart.focus_history[0].progress > 0.0 && chart.focus_history[0].progress < 1.0);
        assert!(
            chart
                .background_display_blocks
                .iter()
                .any(|b| b.node_idx == 0)
        );
        // Reproduce the user's paused reverse zoom at several fractions. Every
        // background phase must finish before the clipped focus backing starts.
        let mut case = 0;
        for detail in [MapDetail::Folders, MapDetail::Files] {
            for style in [
                TreemapStyle::Flat,
                TreemapStyle::VerticalGradient,
                TreemapStyle::OffsetVerticalGradient,
                TreemapStyle::DiagonalGradient,
                TreemapStyle::Cushion,
                TreemapStyle::Bevel,
                TreemapStyle::Glass,
                TreemapStyle::Neon,
                TreemapStyle::Outline,
            ] {
                for fraction in [0.2, 0.5, 0.8] {
                    chart.detail = detail;
                    chart.style = style;
                    let focus = chart.focus_history.last_mut().unwrap();
                    focus.auto_entering = false;
                    focus.progress = fraction;
                    focus.target_progress = fraction;
                    let output = frame(
                        &mut chart,
                        1.52 + case as f64 / 60.0,
                        vec![Event::PointerGone],
                    );
                    case += 1;
                    let clip = chart.last_rect.intersect(viewport);
                    let foreground = output.shapes.iter().position(|shape| shape.clip_rect == clip
                        && matches!(&shape.shape, eframe::egui::Shape::Mesh(mesh) if !mesh.vertices.is_empty()))
                        .expect("focus has its own clipped opaque backing");
                    assert!(foreground > 0, "background must be painted first");
                    for shape in &output.shapes[foreground..] {
                        if !shape.clip_rect.is_positive() {
                            continue;
                        }
                        assert!(
                            clip.expand(0.01).contains_rect(shape.clip_rect),
                            "background text/borders must never reappear above focus: {detail:?}/{style:?}/{fraction}; expected {clip:?}, actual {:?}, shape {:?}",
                            shape.clip_rect,
                            std::mem::discriminant(&shape.shape)
                        );
                    }
                    assert!(chart.display_blocks.iter().all(|b| is_descendant(
                        &snapshot.nodes,
                        b.node_idx,
                        1
                    )));
                    assert!(
                        chart
                            .background_display_blocks
                            .iter()
                            .all(|b| !is_descendant(&snapshot.nodes, b.node_idx, 1))
                    );
                    if detail == MapDetail::Files {
                        assert_ne!(
                            chart.detail_texture.as_ref().unwrap().id(),
                            chart.background_texture.as_ref().unwrap().id()
                        );
                    }
                }
            }
        }
        chart.detail = MapDetail::Files;
        chart.style = TreemapStyle::VerticalGradient;

        for i in 0..12 {
            frame(
                &mut chart,
                3.0 + i as f64 / 60.0,
                vec![
                    Event::PointerMoved(viewport.center()),
                    Event::ModifiersChanged(Modifiers {
                        ctrl: true,
                        ..Default::default()
                    }),
                    Event::MouseWheel {
                        unit: eframe::egui::MouseWheelUnit::Point,
                        delta: eframe::egui::vec2(0.0, -90.0),
                        phase: eframe::egui::TouchPhase::Move,
                        modifiers: Modifiers {
                            ctrl: true,
                            ..Default::default()
                        },
                    },
                ],
            );
        }
        let mut restored = false;
        for i in 0..65 {
            let before = chart.zoom_root;
            frame(&mut chart, 3.4 + i as f64 / 60.0, vec![]);
            if before == 1 && chart.zoom_root == 0 {
                restored = true;
                assert_eq!(chart.scale, parent_camera.0);
                assert!((chart.offset - parent_camera.1).length() < 0.01);
            }
        }
        assert!(restored);
        assert_eq!(chart.zoom_root, 0);
        assert!(chart.focus_history.is_empty());
        // The wheel's remaining smoothed delta keeps zooming the parent rather
        // than being discarded at the navigation boundary.
        assert!(chart.scale >= 1.0 && chart.scale <= parent_camera.0);
        frame(&mut chart, 5.0, vec![]);
        assert_eq!(chart.scale, chart.target_scale);
    }

    #[test]
    fn original_camera_ease_is_restored_and_keeps_world_cursor_point() {
        let pivot = eframe::egui::vec2(320.0, 210.0);
        let mut offset = eframe::egui::vec2(-100.0, -70.0);
        let mut scale = 2.0;
        let world = (pivot - offset) / scale;
        for _ in 0..12 {
            let next = inertial_camera_step(scale, 4.0, 1.0 / 60.0);
            offset = zoom_about(offset, pivot, scale, next);
            scale = next;
            assert!(((pivot - offset) / scale - world).length() < 0.001);
        }
        assert!(scale < 4.0, "long ease-out must remain after 180 ms");
        for _ in 0..120 {
            scale = inertial_camera_step(scale, 4.0, 1.0 / 60.0);
        }
        assert_eq!(scale, 4.0);
    }

    #[test]
    fn folder_shapes_match_content_squarification_and_only_scale_when_details_open() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"BV SOLO");
        let file = pool.get_or_insert(b"photo.png");
        let mut nodes = vec![FileNode::new(name, None, true, false, 0, 0)];
        nodes[0].size = 4000;
        nodes[0].first_child = 1;
        for i in 1..=4 {
            let mut dir = FileNode::new(name, Some(0), true, false, 0, 0);
            dir.size = 1000;
            if i < 4 {
                dir.next_sibling = i + 1;
            }
            nodes.push(dir);
        }
        for dir in 1..=4 {
            nodes[dir].first_child = nodes.len() as u32;
            for i in 0..1000 {
                let mut node = FileNode::new(file, Some(dir as u32), false, false, 0, 0);
                node.size = 1;
                if i < 999 {
                    node.next_sibling = nodes.len() as u32 + 1;
                }
                nodes.push(node);
            }
        }
        // Legacy padding gives a 100x100 content area. Four equal folders must
        // occupy four 50x50 squares, not rows derived from the 104x122 outer box.
        let base = Rect::from_min_size(pos2(10.0, 20.0), eframe::egui::vec2(104.0, 122.0));
        for detail in [MapDetail::Folders, MapDetail::Files] {
            let cache = std::cell::RefCell::new(DirectoryCache::default());
            let mut reference: Vec<(u32, Rect)> = Vec::new();
            for scale in [1.0, 1.2, 2.0, 8.0, 1.0] {
                let config = TreemapConfig {
                    skip_subtree: None,
                    geometry_scale: scale,
                    layout_cache: Some(&cache),
                    nodes: &nodes,
                    string_pool: &pool,
                    max_depth: 128,
                    pixels_per_point: 1.0,
                    directory_colors: &[],
                    layout: MapLayout::Folders,
                    detail,
                    clip: Rect::EVERYTHING,
                };
                let map = Rect::from_min_size(base.min, base.size() * scale);
                let mut blocks = Vec::new();
                build_treemap(&config, 0, map, 0, &mut blocks);
                for idx in 1..=4 {
                    let block = blocks.iter().find(|b| b.node_idx == idx).unwrap();
                    assert!((block.rect.width() - 50.0 * scale).abs() < 0.01);
                    assert!((block.rect.height() - 50.0 * scale).abs() < 0.01);
                    if reference.len() < 4 {
                        reference.push((idx, block.rect));
                    } else {
                        let original = reference.iter().find(|(id, _)| *id == idx).unwrap().1;
                        let expected = Rect::from_min_size(
                            base.min + (original.min - base.min) * scale,
                            original.size() * scale,
                        );
                        assert!(block.rect.min.distance(expected.min) < 0.01);
                    }
                    for child in blocks
                        .iter()
                        .filter(|b| nodes[b.node_idx as usize].parent == idx)
                    {
                        assert!(
                            block.rect.expand(0.001).contains_rect(child.rect),
                            "details escaped their parent"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn directory_cache_keeps_rows_stable_and_reuses_sorted_children() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"file.png");
        let mut nodes = vec![FileNode::new(name, None, true, false, 0, 0)];
        nodes[0].first_child = 1;
        for i in 1..=100 {
            let mut file = FileNode::new(name, Some(0), false, false, 0, 0);
            file.size = 101 - i as u64;
            if i < 100 {
                file.next_sibling = i + 1;
            }
            nodes[0].size += file.size;
            nodes.push(file);
        }
        let cache = std::cell::RefCell::new(DirectoryCache::default());
        let config = TreemapConfig {
            skip_subtree: None,
            nodes: &nodes,
            string_pool: &pool,
            max_depth: 128,
            pixels_per_point: 1.0,
            directory_colors: &[],
            layout: MapLayout::Folders,
            detail: MapDetail::Files,
            clip: Rect::EVERYTHING,
            layout_cache: Some(&cache),
            geometry_scale: 1.0,
        };
        let first = sibling_layout(
            &config,
            0,
            Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(100.0, 80.0)),
        );
        let next = sibling_layout(
            &config,
            0,
            Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(800.0, 90.0)),
        );
        assert!(Arc::ptr_eq(&first, &next));
        assert_eq!(first.rows, next.rows);
        assert_eq!(first.children.len(), 100);
        assert_eq!(*first.prefix.last().unwrap(), nodes[0].size as f64);
    }

    #[test]
    fn wheel_zoom_keeps_screenshots_folder_shape_and_cursor_world_point() {
        let mut pool = StringPool::new();
        let root = pool.get_or_insert(b"root");
        let screenshots = pool.get_or_insert(b"screenshots");
        let saves = pool.get_or_insert(b"saves");
        let file = pool.get_or_insert(b"photo.png");
        let mut nodes = vec![
            FileNode::new(root, None, true, false, 0, 0),
            FileNode::new(screenshots, Some(0), true, false, 0, 0),
            FileNode::new(saves, Some(0), true, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[1].next_sibling = 2;
        for dir in [1, 2] {
            nodes[dir].first_child = nodes.len() as u32;
            for i in 0..1000 {
                let mut node = FileNode::new(file, Some(dir as u32), false, false, 0, 0);
                node.size = 100;
                if i < 999 {
                    node.next_sibling = nodes.len() as u32 + 1;
                }
                nodes[dir].size += node.size;
                nodes[0].size += node.size;
                nodes.push(node);
            }
        }
        let snapshot = FileArenaSnapshot {
            dir_counts: Arc::new(precompute_dir_counts(&nodes)),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            extensions: Default::default(),
        };
        let mut chart = TreemapChart::new();
        chart.detail = MapDetail::Files;
        let ctx = eframe::egui::Context::default();
        let mut selected = Default::default();
        let mut expanded = Default::default();
        let mut scroll = false;
        let mut context = StatContext {
            selected_nodes: &mut selected,
            expanded_nodes: &mut expanded,
            scroll_to_selected: &mut scroll,
            deduplicator_results: None,
        };
        let screen = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(800.0, 500.0));
        let mut frame = |chart: &mut TreemapChart,
                         time: f64,
                         pointer: Option<eframe::egui::Pos2>,
                         zoom: bool| {
            let mut events = vec![eframe::egui::Event::ModifiersChanged(
                eframe::egui::Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            )];
            if let Some(pos) = pointer {
                events.push(eframe::egui::Event::PointerMoved(pos));
            }
            if zoom {
                events.push(eframe::egui::Event::MouseWheel {
                    unit: eframe::egui::MouseWheelUnit::Point,
                    delta: eframe::egui::vec2(0.0, 94.0),
                    phase: eframe::egui::TouchPhase::Move,
                    modifiers: eframe::egui::Modifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                });
            }
            let _ = ctx.run_ui(
                eframe::egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    chart.render(ui, &snapshot, &mut context);
                },
            );
        };
        frame(&mut chart, 0.0, None, false);
        let folder = chart
            .display_blocks
            .iter()
            .find(|b| b.node_idx == 1)
            .unwrap()
            .rect;
        let pointer = folder.center();
        frame(&mut chart, 0.016, Some(pointer), false);
        let base_map = chart.last_rect;
        let base_folder = chart
            .cached_blocks
            .iter()
            .find(|b| b.node_idx == 1)
            .unwrap()
            .rect;
        let folder_uv = (pointer - base_folder.min) / base_folder.size();
        for i in 2..80 {
            frame(
                &mut chart,
                i as f64 / 60.0,
                Some(pointer),
                [2, 5, 8, 11].contains(&i),
            );
            let current = chart
                .cached_blocks
                .iter()
                .find(|b| b.node_idx == 1)
                .unwrap()
                .rect;
            let expected = Rect::from_min_size(
                chart.last_rect.min + (base_folder.min - base_map.min) * chart.scale,
                base_folder.size() * chart.scale,
            );
            assert!(current.min.distance(expected.min) < 0.02);
            assert!((current.size() - expected.size()).length() < 0.02);
            assert!((current.min + folder_uv * current.size()).distance(pointer) < 0.02);
        }
        assert!(
            chart.target_scale > 5.0,
            "wheel events must actually reach the map"
        );
        assert!(!chart.animation_running);
        let settled: Vec<_> = chart
            .display_blocks
            .iter()
            .map(|b| (b.node_idx, b.rect))
            .collect();
        frame(&mut chart, 2.0, Some(pointer), false);
        assert_eq!(
            settled,
            chart
                .display_blocks
                .iter()
                .map(|b| (b.node_idx, b.rect))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn gpu_tiny_files_match_cpu_and_benchmark() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"file.png");
        let snapshot = FileArenaSnapshot {
            nodes: Arc::new(NodeStorage::Owned(vec![FileNode::new(
                name, None, false, false, 0, 0,
            )])),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(vec![0]),
            extensions: Default::default(),
        };
        let mut gpu = super::super::treemap_gpu::GpuCheck::new();
        let styles = [
            TreemapStyle::VerticalGradient,
            TreemapStyle::OffsetVerticalGradient,
            TreemapStyle::DiagonalGradient,
            TreemapStyle::Cushion,
            TreemapStyle::Flat,
            TreemapStyle::Bevel,
            TreemapStyle::Glass,
            TreemapStyle::Neon,
            TreemapStyle::Outline,
        ];
        let mut checked = 0;
        for dpi in [1., 1.25, 2.] {
            let viewport =
                Rect::from_min_size(pos2(17., 25.), eframe::egui::vec2(64. / dpi, 48. / dpi));
            for style in styles {
                let blocks: Vec<_> = (0..100)
                    .map(|i| {
                        // Avoid a distance of exactly 1.0: the old CPU path can
                        // classify that boundary differently after logical-DPI division.
                        let x = (i % 10) as f32 * 5.27 - 1.23;
                        let y = (i / 10) as f32 * 4.47 - 0.83;
                        TreemapBlock {
                            rect: Rect::from_min_size(
                                viewport.min + eframe::egui::vec2(x / dpi, y / dpi),
                                eframe::egui::vec2(5.27 / dpi, 4.47 / dpi),
                            ),
                            node_idx: 0,
                            color: Color32::from_rgb(81, 173, 209),
                            is_frame: false,
                            transition: (i % 4) as f32 / 3.,
                            header_transition: 0.,
                        }
                    })
                    .collect();
                let expected = rasterize_files(
                    &blocks,
                    &snapshot,
                    viewport,
                    dpi,
                    MapColor::Extension,
                    MapPalette::Pastel,
                    style,
                    &[],
                );
                let tiles = gpu_tiles(
                    &blocks,
                    &snapshot,
                    viewport,
                    dpi,
                    MapColor::Extension,
                    MapPalette::Pastel,
                    style,
                    &[],
                );
                let actual = gpu.render(tiles, 64, 48);
                for (i, (a, b)) in actual.iter().zip(&expected.pixels).enumerate() {
                    assert!(
                        a.to_array()
                            .iter()
                            .zip(b.to_array())
                            .all(|(x, y)| x.abs_diff(y) <= 1),
                        "{style:?} dpi={dpi} pixel={i}: {a:?} != {b:?}"
                    );
                }
                checked += 1;
            }
        }
        println!(
            "GPU_CHECK {checked} style/DPI/animation/clipping images match CPU (channel tolerance 1/255)"
        );
        let viewport = Rect::from_min_size(pos2(0., 0.), eframe::egui::vec2(2560., 1440.));
        for cell in [3., 5.] {
            let cols = (viewport.width() / cell) as usize;
            let rows = (viewport.height() / cell) as usize;
            let blocks: Vec<_> = (0..cols * rows)
                .map(|i| TreemapBlock {
                    rect: Rect::from_min_size(
                        pos2((i % cols) as f32 * cell, (i / cols) as f32 * cell),
                        eframe::egui::vec2(cell, cell),
                    ),
                    node_idx: 0,
                    color: Color32::from_rgb(81, 173, 209),
                    is_frame: false,
                    transition: 0.6,
                    header_transition: 0.,
                })
                .collect();
            let mut timings = Vec::new();
            for _ in 0..9 {
                let start = std::time::Instant::now();
                std::hint::black_box(rasterize_files(
                    &blocks,
                    &snapshot,
                    viewport,
                    1.,
                    MapColor::Extension,
                    MapPalette::Pastel,
                    TreemapStyle::Cushion,
                    &[],
                ));
                let old = start.elapsed().as_secs_f64() * 1000.;
                let start = std::time::Instant::now();
                std::hint::black_box(gpu_tiles(
                    &blocks,
                    &snapshot,
                    viewport,
                    1.,
                    MapColor::Extension,
                    MapPalette::Pastel,
                    TreemapStyle::Cushion,
                    &[],
                ));
                timings.push((old, start.elapsed().as_secs_f64() * 1000.));
            }
            let mut old: Vec<_> = timings.iter().map(|t| t.0).collect();
            old.sort_by(f64::total_cmp);
            let mut new: Vec<_> = timings.iter().map(|t| t.1).collect();
            new.sort_by(f64::total_cmp);
            println!(
                "GPU_PREP QHD cell={cell} tiles={} median CPU old={:.3}ms new={:.3}ms speedup={:.2}x image_bytes={} instance_bytes={}",
                blocks.len(),
                old[4],
                new[4],
                old[4] / new[4],
                2560 * 1440 * 4,
                blocks.len() * std::mem::size_of::<super::super::treemap_gpu::Tile>()
            );
        }
    }

    #[test]
    fn benchmark_dense_zoom_cache_and_clipped_grid() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"file.png");
        let mut nodes = vec![FileNode::new(name, None, true, false, 0, 0)];
        nodes[0].first_child = 1;
        nodes[0].size = 200_000;
        for i in 1..=200_000 {
            let mut file = FileNode::new(name, Some(0), false, false, 0, 0);
            file.size = 1;
            if i < 200_000 {
                file.next_sibling = i + 1;
            }
            nodes.push(file);
        }
        let cache = std::cell::RefCell::new(DirectoryCache::default());
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(1000.0, 600.0));
        let mut config = TreemapConfig {
            skip_subtree: None,
            nodes: &nodes,
            string_pool: &pool,
            max_depth: 128,
            pixels_per_point: 1.0,
            directory_colors: &[],
            layout: MapLayout::Classic,
            detail: MapDetail::Files,
            clip: viewport,
            layout_cache: Some(&cache),
            geometry_scale: 1.0,
        };
        let mut blocks = Vec::new();
        build_treemap(&config, 0, viewport, 0, &mut blocks); // warm cache
        let mut elapsed = Vec::new();
        for cached in [false, true] {
            config.layout_cache = if cached { Some(&cache) } else { None };
            let start = std::time::Instant::now();
            for frame in 0..12 {
                blocks.clear();
                let scale = 1.0 + frame as f32 * 0.1;
                let map = Rect::from_min_size(
                    pos2(-100.0, -60.0) * (scale - 1.0),
                    viewport.size() * scale,
                );
                build_treemap(&config, 0, map, 0, &mut blocks);
            }
            elapsed.push(start.elapsed().as_secs_f64() * 1000.0 / 12.0);
        }
        println!(
            "ZOOM_LAYOUT 200000 files, per-frame ms: uncached={:.3}, cached={:.3}",
            elapsed[0], elapsed[1]
        );
        let mut cells = Vec::new();
        let siblings = sibling_layout(&config, 0, viewport);
        let large = Rect::from_min_size(
            pos2(-10000.0, -10000.0),
            eframe::egui::vec2(50000.0, 50000.0),
        );
        fill_dense_tail(&config, &siblings.children, large, 0, &mut cells);
        assert!(
            cells.len() < 1000,
            "offscreen cells must not enter the renderer"
        );
        assert!(cells.iter().all(|b| b.rect.intersects(viewport)));
    }

    #[test]
    fn file_borders_remain_visible_through_color_animation_and_at_all_dpi() {
        let mut pool = StringPool::new();
        let dir = pool.get_or_insert(b"folder");
        let name = pool.get_or_insert(b"photo.png");
        let mut file = FileNode::new(name, Some(0), false, false, 0, 0);
        file.size = 1;
        let snapshot = FileArenaSnapshot {
            nodes: Arc::new(NodeStorage::Owned(vec![
                FileNode::new(dir, None, true, false, 0, 0),
                file,
            ])),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(vec![1, 0]),
            extensions: Default::default(),
        };
        for dpi in [1.0, 1.25, 2.0] {
            for progress in [0.0, 0.1, 0.5, 1.0] {
                let rect =
                    Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(3.0 / dpi, 3.0 / dpi));
                let tile = TreemapBlock {
                    rect,
                    node_idx: 1,
                    color: colors::get_color_for_extension("png"),
                    is_frame: false,
                    transition: progress,
                    header_transition: progress,
                };
                let fill = block_color(
                    &tile,
                    &snapshot,
                    MapColor::Extension,
                    MapPalette::Pastel,
                    &[],
                );
                let image = rasterize_files(
                    &[tile],
                    &snapshot,
                    rect,
                    dpi,
                    MapColor::Extension,
                    MapPalette::Pastel,
                    TreemapStyle::Flat,
                    &[],
                );
                assert_eq!(image.size, [3, 3]);
                assert_eq!(image.pixels[4], fill);
                assert_eq!(
                    image
                        .pixels
                        .iter()
                        .filter(|&&c| c == file_border_color(fill))
                        .count(),
                    8
                );
                let mut mesh = eframe::egui::Mesh::default();
                append_file_border(&mut mesh, rect, dpi, file_border_color(fill));
                let painted_area: f32 = mesh
                    .indices
                    .chunks_exact(3)
                    .map(|tri| {
                        let a = mesh.vertices[tri[0] as usize].pos;
                        let b = mesh.vertices[tri[1] as usize].pos;
                        let c = mesh.vertices[tri[2] as usize].pos;
                        ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() * 0.5
                    })
                    .sum();
                assert!((painted_area * dpi * dpi - 8.0).abs() < 0.001);
            }
        }
    }

    #[test]
    fn file_mode_outlines_folder_backing_even_without_header_or_border_checkbox() {
        let mut pool = StringPool::new();
        let dir = pool.get_or_insert(b"screenshots");
        let name = pool.get_or_insert(b"photo.png");
        let mut nodes = vec![FileNode::new(dir, None, true, false, 0, 0)];
        nodes[0].size = 10_000;
        nodes[0].first_child = 1;
        for i in 0..10_000 {
            let mut file = FileNode::new(name, Some(0), false, false, 0, 0);
            file.size = 1;
            if i < 9_999 {
                file.next_sibling = i + 2;
            }
            nodes.push(file);
        }
        let snapshot = FileArenaSnapshot {
            dir_counts: Arc::new(precompute_dir_counts(&nodes)),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            extensions: Default::default(),
        };
        let mut chart = TreemapChart::new();
        chart.detail = MapDetail::Files;
        chart.layout = MapLayout::Classic; // No folder headers in this layout.
        chart.draw_borders = false;
        let ctx = eframe::egui::Context::default();
        let mut selected = Default::default();
        let mut expanded = Default::default();
        let mut scroll = false;
        let mut context = StatContext {
            selected_nodes: &mut selected,
            expanded_nodes: &mut expanded,
            scroll_to_selected: &mut scroll,
            deduplicator_results: None,
        };
        let output = ctx.run_ui(
            eframe::egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    eframe::egui::vec2(120.0, 120.0),
                )),
                ..Default::default()
            },
            |ui| {
                chart.render(ui, &snapshot, &mut context);
            },
        );
        assert!(chart.cached_blocks.iter().any(|b| !b.is_frame));
        let border = file_border_color(colors::get_color_for_extension("png"));
        assert!(output.shapes.iter().any(|s| match &s.shape {
            eframe::egui::Shape::Mesh(mesh) =>
                mesh.vertices.len() >= 16 && mesh.vertices.iter().all(|v| v.color == border),
            _ => false,
        }));
    }

    #[test]
    fn soft_dividers_open_close_and_only_small_local_movements_are_animated() {
        let map = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(800.0, 600.0));
        let mut tile = TreemapBlock {
            rect: Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(100.0, 100.0)),
            node_idx: 1,
            color: Color32::BLUE,
            is_frame: false,
            transition: 1.0,
            header_transition: 1.0,
        };
        let mut states = Default::default();
        animate_layout(
            &mut states,
            &[tile.clone()],
            0.016,
            true,
            MapAnimation::Soft,
            map,
            1.0,
        );
        tile.is_frame = true;
        tile.rect = tile.rect.translate(eframe::egui::vec2(4.0, 0.0));
        let (first, running) = animate_layout(
            &mut states,
            &[tile.clone()],
            0.016,
            false,
            MapAnimation::Soft,
            map,
            1.0,
        );
        assert!(running);
        assert!(first[0].rect.left() > 0.0 && first[0].rect.left() < 4.0);
        assert!(first[0].header_transition > 0.0 && first[0].header_transition < 1.0);
        for _ in 0..60 {
            animate_layout(
                &mut states,
                &[tile.clone()],
                0.016,
                false,
                MapAnimation::Soft,
                map,
                1.0,
            );
        }
        let (settled, running) = animate_layout(
            &mut states,
            &[tile.clone()],
            0.016,
            false,
            MapAnimation::Soft,
            map,
            1.0,
        );
        assert!(!running);
        assert_eq!(settled[0].rect, tile.rect);
        assert_eq!(settled[0].header_transition, 1.0);
        tile.is_frame = false;
        let (closing, running) = animate_layout(
            &mut states,
            &[tile.clone()],
            0.016,
            false,
            MapAnimation::Soft,
            map,
            1.0,
        );
        assert!(running);
        assert!(
            closing
                .iter()
                .any(|b| b.is_frame && b.header_transition > 0.0 && b.header_transition < 1.0)
        );
        for _ in 0..20 {
            animate_layout(
                &mut states,
                &[tile.clone()],
                0.016,
                false,
                MapAnimation::Soft,
                map,
                1.0,
            );
        }
        tile.rect = tile.rect.translate(eframe::egui::vec2(500.0, 0.0));
        let (far, _) = animate_layout(
            &mut states,
            &[tile.clone()],
            0.016,
            false,
            MapAnimation::Soft,
            map,
            1.0,
        );
        assert_eq!(far[0].rect, tile.rect); // A large reflow never flies across the map.
        let zoomed = Rect::from_min_size(map.min, map.size() * 2.0);
        tile.rect = Rect::from_min_max(
            pos2(tile.rect.left() * 2.0, 0.0),
            pos2(tile.rect.right() * 2.0, 200.0),
        );
        let (camera, running) = animate_layout(
            &mut states,
            &[tile.clone()],
            0.016,
            false,
            MapAnimation::Soft,
            zoomed,
            1.0,
        );
        assert_eq!(camera[0].rect, tile.rect);
        assert!(!running); // Camera zoom does not create a second trailing animation.
    }

    #[test]
    fn calm_animation_keeps_geometry_exact_and_off_discards_animation_state() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(100.0, 100.0));
        let tile = |id, x| TreemapBlock {
            rect: rect.translate(eframe::egui::vec2(x, 0.0)),
            node_idx: id,
            color: Color32::GREEN,
            is_frame: false,
            transition: 1.0,
            header_transition: 1.0,
        };
        let mut states = Default::default();
        animate_layout(
            &mut states,
            &[tile(1, 0.0)],
            0.016,
            true,
            MapAnimation::Colors,
            rect,
            1.0,
        );
        let targets = vec![tile(1, 500.0), tile(2, 0.0)];
        let (first, running) = animate_layout(
            &mut states,
            &targets,
            0.016,
            false,
            MapAnimation::Colors,
            rect,
            1.0,
        );
        assert!(running);
        assert_eq!(first[0].rect, targets[0].rect); // No cross-map flights.
        assert_eq!(first[1].rect, targets[1].rect); // No expanding/shrinking holes.
        assert!(first[1].transition > 0.0 && first[1].transition < 1.0);
        let (off, running) = animate_layout(
            &mut states,
            &targets,
            0.016,
            false,
            MapAnimation::Off,
            rect,
            1.0,
        );
        assert!(!running);
        assert!(states.is_empty());
        assert!(off.iter().all(|b| b.transition == 1.0));
        assert_eq!(off[0].rect, targets[0].rect);
        let (remaining, _) = animate_layout(
            &mut states,
            &targets[..1],
            0.016,
            false,
            MapAnimation::Colors,
            rect,
            1.0,
        );
        assert_eq!(remaining.len(), 1); // Departed tiles leave no moving ghosts.
        assert_eq!(TreemapChart::new().animation, MapAnimation::Soft);
    }

    #[test]
    fn suppressed_files_and_collapsed_folders_have_content_colored_backing() {
        let mut pool = StringPool::new();
        let root = pool.get_or_insert(b"objects");
        let name = pool.get_or_insert(b"sample.png");
        let mut nodes = vec![FileNode::new(root, None, true, false, 0, 0)];
        nodes[0].size = 100;
        nodes[0].first_child = 1;
        for i in 0..100 {
            let mut file = FileNode::new(name, Some(0), false, false, 0, 0);
            file.size = 1;
            if i < 99 {
                file.next_sibling = i + 2;
            }
            nodes.push(file);
        }
        let colors = directory_fill_colors(&nodes, &pool);
        let expected = colors::get_color_for_extension("png");
        assert_eq!(colors[0], expected);
        assert_ne!(expected, colors::get_treemap_dir_color("objects"));
        let rect = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(12.0, 12.0));
        for detail in [MapDetail::Files, MapDetail::Folders] {
            let config = TreemapConfig {
                skip_subtree: None,
                layout_cache: None,
                geometry_scale: 1.0,
                nodes: &nodes,
                string_pool: &pool,
                max_depth: 128,
                pixels_per_point: 1.0,
                directory_colors: &colors,
                layout: MapLayout::Classic,
                detail,
                clip: rect,
            };
            let mut blocks = Vec::new();
            build_treemap(&config, 0, rect, 0, &mut blocks);
            assert_eq!(
                blocks.len(),
                if detail == MapDetail::Files { 17 } else { 1 }
            );
            assert_eq!(blocks[0].rect, rect);
            assert_eq!(blocks[0].color, expected);
        }
        // A directory stored after its file must still inherit its color.
        let mut reordered = vec![
            FileNode::new(name, Some(1), false, false, 0, 0),
            FileNode::new(root, None, true, false, 0, 0),
        ];
        reordered[0].size = 10;
        assert_eq!(directory_fill_colors(&reordered, &pool)[1], expected);
    }

    #[test]
    fn new_tiles_blend_from_the_actual_folder_backing_color() {
        let mut pool = StringPool::new();
        let dir = pool.get_or_insert(b"folder");
        let file = pool.get_or_insert(b"file.png");
        let nodes = vec![
            FileNode::new(dir, None, true, false, 0, 0),
            FileNode::new(file, Some(0), false, false, 0, 0),
        ];
        let snapshot = FileArenaSnapshot {
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(vec![1, 0]),
            extensions: Default::default(),
        };
        let backing = Color32::from_rgb(70, 90, 120);
        let mut block = TreemapBlock {
            rect: Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(10.0, 10.0)),
            node_idx: 1,
            color: colors::get_color_for_extension("png"),
            is_frame: false,
            transition: 0.0,
            header_transition: 1.0,
        };
        assert_eq!(
            block_color(
                &block,
                &snapshot,
                MapColor::Extension,
                MapPalette::Pastel,
                &[backing]
            ),
            backing
        );
        block.transition = 0.5;
        let middle = block_color(
            &block,
            &snapshot,
            MapColor::Extension,
            MapPalette::Pastel,
            &[backing],
        );
        assert_ne!(middle, backing);
        assert_ne!(middle, Color32::BLACK);
        block.transition = 1.0;
        assert_eq!(
            block_color(
                &block,
                &snapshot,
                MapColor::Extension,
                MapPalette::Pastel,
                &[backing]
            ),
            block.color
        );
    }

    #[test]
    fn smallest_tile_has_one_content_pixel_and_eight_border_pixels_at_any_dpi() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"photo.png");
        let mut node = FileNode::new(name, None, false, false, 0, 0);
        node.size = 1;
        let snapshot = FileArenaSnapshot {
            dir_counts: Arc::new(vec![0]),
            nodes: Arc::new(NodeStorage::Owned(vec![node])),
            string_pool: Arc::new(pool),
            extensions: Default::default(),
        };
        let base = colors::get_color_for_extension("png");
        for dpi in [1.0, 1.25, 2.0] {
            let rect =
                Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(3.0 / dpi, 3.0 / dpi));
            let config = TreemapConfig {
                skip_subtree: None,
                layout_cache: None,
                geometry_scale: 1.0,
                nodes: &snapshot.nodes,
                string_pool: &snapshot.string_pool,
                max_depth: 128,
                pixels_per_point: dpi,
                directory_colors: &[],
                layout: MapLayout::Classic,
                detail: MapDetail::Files,
                clip: rect,
            };
            let mut blocks = Vec::new();
            build_treemap(&config, 0, rect, 0, &mut blocks);
            assert_eq!(blocks.len(), 1);
            let image = rasterize_files(
                &blocks,
                &snapshot,
                rect,
                dpi,
                MapColor::Extension,
                MapPalette::Pastel,
                TreemapStyle::Flat,
                &[],
            );
            assert_eq!(image.size, [3, 3]);
            assert_eq!(image.pixels[4], base);
            assert_eq!(
                image
                    .pixels
                    .iter()
                    .filter(|&&c| c == scale_rgb(base, 0.72))
                    .count(),
                8
            );
            blocks.clear();
            let too_small =
                Rect::from_min_size(rect.min, eframe::egui::vec2(2.99 / dpi, 3.0 / dpi));
            build_treemap(&config, 0, too_small, 0, &mut blocks);
            assert!(blocks.is_empty());
        }
    }

    #[test]
    fn dense_minimum_tiles_fill_parent_without_overlap_and_keep_largest_first() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"file.png");
        let mut nodes = vec![FileNode::new(name, None, true, false, 0, 0)];
        nodes[0].first_child = 1;
        for i in 0..10_000 {
            let mut node = FileNode::new(name, Some(0), false, false, 0, 0);
            node.size = if i < 3 { 3000 - i as u64 * 500 } else { 1 };
            nodes[0].size += node.size;
            if i < 9999 {
                node.next_sibling = i + 2;
            }
            nodes.push(node);
        }
        for dpi in [1.0, 1.25, 2.0] {
            for layout in [MapLayout::Classic, MapLayout::Stripes] {
                let rect = Rect::from_min_size(pos2(0.37, 0.19), eframe::egui::vec2(53.0, 41.0));
                let config = TreemapConfig {
                    skip_subtree: None,
                    layout_cache: None,
                    geometry_scale: 1.0,
                    nodes: &nodes,
                    string_pool: &pool,
                    max_depth: 128,
                    pixels_per_point: dpi,
                    directory_colors: &[],
                    layout,
                    detail: MapDetail::Files,
                    clip: rect,
                };
                let mut blocks = Vec::new();
                build_treemap(&config, 0, rect, 0, &mut blocks);
                let tiles: Vec<_> = blocks.iter().filter(|b| !b.is_frame).collect();
                assert!(!tiles.is_empty());
                assert_eq!(tiles[0].node_idx, 1);
                let area: f32 = tiles.iter().map(|b| b.rect.area()).sum();
                assert!(
                    (area - rect.area()).abs() < 0.1,
                    "unfilled area: {}",
                    rect.area() - area
                );
                for (i, tile) in tiles.iter().enumerate() {
                    assert!(rect.contains_rect(tile.rect));
                    assert!(tile.rect.width() * dpi >= 2.999 && tile.rect.height() * dpi >= 2.999);
                    for other in &tiles[i + 1..] {
                        let intersection = tile.rect.intersect(other.rect);
                        assert!(intersection.width() <= 0.0001 || intersection.height() <= 0.0001);
                    }
                }
                let largest_shown = tiles.iter().map(|b| b.node_idx).max().unwrap();
                assert_eq!(
                    largest_shown as usize,
                    tiles.len(),
                    "largest siblings must be kept before smaller ones"
                );
            }
        }
    }

    #[test]
    fn pixel_limit_fills_dense_files_at_each_zoom_and_preserves_full_color() {
        let mut pool = StringPool::new();
        let dir_name = pool.get_or_insert(b"screenshots");
        let file_name = pool.get_or_insert(b"photo.png");
        let count = 20_000;
        let mut nodes = vec![FileNode::new(dir_name, None, true, false, 0, 0)];
        nodes[0].first_child = 1;
        nodes[0].size = count;
        for i in 0..count {
            let mut file = FileNode::new(file_name, Some(0), false, false, 0, 0);
            file.size = 1;
            if i + 1 < count {
                file.next_sibling = i as u32 + 2;
            }
            nodes.push(file);
        }
        let snapshot = FileArenaSnapshot {
            dir_counts: Arc::new(precompute_dir_counts(&nodes)),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            extensions: Default::default(),
        };
        // Less than one pixel per file: previously both modes became one beige block.
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(120.0, 120.0));
        for scale in [1.0, 4.0, 1.0] {
            let rect = Rect::from_min_size(viewport.min, viewport.size() * scale);
            let mut config = TreemapConfig {
                skip_subtree: None,
                layout_cache: None,
                geometry_scale: 1.0,
                nodes: &snapshot.nodes,
                string_pool: &snapshot.string_pool,
                max_depth: 128,
                pixels_per_point: 1.0,
                directory_colors: &[],
                layout: MapLayout::Classic,
                detail: MapDetail::Folders,
                clip: viewport,
            };
            let mut grouped = Vec::new();
            build_treemap(&config, 0, viewport, 0, &mut grouped);
            assert_eq!(grouped.len(), 1);
            config.detail = MapDetail::Files;
            let mut files = Vec::new();
            build_treemap(&config, 0, rect, 0, &mut files);
            if scale == 1.0 {
                assert!(
                    files.iter().filter(|b| !b.is_frame).count() == 1600,
                    "Dense files must fill all available three-pixel cells"
                );
            } else {
                assert!(files.len() > 500);
                assert!(
                    files
                        .iter()
                        .filter(|b| !b.is_frame)
                        .all(|b| b.node_idx > 0 && b.rect.width() >= 3.0 && b.rect.height() >= 3.0)
                );
                let image = rasterize_files(
                    &files,
                    &snapshot,
                    viewport,
                    1.0,
                    MapColor::Extension,
                    MapPalette::Pastel,
                    TreemapStyle::Flat,
                    &[],
                );
                let expected = colors::get_color_for_extension("png");
                assert!(image.pixels.iter().filter(|&&c| c == expected).count() > 500);
                assert!(image.pixels.iter().all(|c| c.a() == 0 || c.a() == 255));
                assert_eq!(image.pixels.len(), 120 * 120);
            }
        }
        for len in 1..32 {
            let areas: Vec<_> = (1..=len).rev().map(|i| i as f64).collect();
            assert!(
                (worst_aspect_ratio(&areas, 10.0)
                    - sorted_aspect_ratio(
                        areas.iter().sum(),
                        areas[0],
                        *areas.last().unwrap(),
                        10.0
                    ))
                .abs()
                    < 1e-9
            );
        }
    }

    #[test]
    fn dense_folders_show_files_while_folder_mode_keeps_nested_frames() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"screenshots");
        let mut nodes = vec![
            FileNode::new(name, None, true, false, 0, 0),
            FileNode::new(name, Some(0), true, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[0].size = 2000;
        nodes[1].first_child = 2;
        nodes[1].size = 2000;
        for i in 0..2000 {
            let mut file = FileNode::new(name, Some(1), false, false, 0, 0);
            file.size = 1;
            if i < 1999 {
                file.next_sibling = i + 3;
            }
            nodes.push(file);
        }
        let rect = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(190.0, 190.0));
        // Less than 16 pixels² per file used to collapse this entire directory.
        let mut config = TreemapConfig {
            skip_subtree: None,
            layout_cache: None,
            geometry_scale: 1.0,
            nodes: &nodes,
            string_pool: &pool,
            max_depth: 128,
            pixels_per_point: 1.0,
            directory_colors: &[],
            layout: MapLayout::Folders,
            detail: MapDetail::Files,
            clip: rect,
        };
        let mut blocks = Vec::new();
        build_treemap(&config, 0, rect, 0, &mut blocks);
        assert!(
            blocks
                .iter()
                .filter(|b| !b.is_frame && b.node_idx >= 2)
                .count()
                > 1500
        );
        config.detail = MapDetail::Folders;
        blocks.clear();
        build_treemap(&config, 0, rect, 0, &mut blocks);
        assert_eq!(blocks.iter().filter(|b| !b.is_frame).count(), 1);
        assert!(blocks.iter().any(|b| b.node_idx == 1 && !b.is_frame));
        assert!(blocks.iter().any(|b| b.node_idx == 0 && b.is_frame));

        // Folder mode must recurse beyond the first level as in the original map.
        let zoomed = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(800.0, 800.0));
        config.clip = zoomed;
        blocks.clear();
        build_treemap(&config, 0, zoomed, 0, &mut blocks);
        assert!(blocks.iter().any(|b| b.node_idx == 1 && b.is_frame));
        assert_eq!(blocks.iter().filter(|b| !b.is_frame).count(), 2000);
        assert_eq!(MapDetail::default(), MapDetail::Folders);
        assert_eq!(TreemapChart::new().detail, MapDetail::Folders);
    }

    #[test]
    fn massive_tree_keeps_every_branch_and_fills_the_viewport() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"test");
        let mut nodes = vec![FileNode::new(name, None, true, false, 0, 0)];
        let mut branches = Vec::new();
        // More than the old 80,000-block cutoff, across multiple sibling branches.
        for _ in 0..4 {
            let dir = nodes.len() as u32;
            branches.push(dir);
            nodes.push(FileNode::new(name, Some(0), true, false, 0, 0));
            nodes[dir as usize].first_child = dir + 1;
            nodes[dir as usize].size = 30_000;
            for i in 0..30_000 {
                let mut file = FileNode::new(name, Some(dir), false, false, 0, 0);
                file.size = 1;
                if i < 29_999 {
                    file.next_sibling = dir + i + 2;
                }
                nodes.push(file);
            }
        }
        nodes[0].size = 120_000;
        nodes[0].first_child = branches[0];
        for pair in branches.windows(2) {
            nodes[pair[0] as usize].next_sibling = pair[1];
        }
        let rect = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(2000.0, 1000.0));
        for detail in [MapDetail::Folders, MapDetail::Files] {
            for layout in [MapLayout::Classic, MapLayout::Folders, MapLayout::Stripes] {
                let config = TreemapConfig {
                    skip_subtree: None,
                    layout_cache: None,
                    geometry_scale: 1.0,
                    nodes: &nodes,
                    string_pool: &pool,
                    max_depth: 128,
                    pixels_per_point: 1.0,
                    directory_colors: &[],
                    layout,
                    detail,
                    clip: rect,
                };
                let mut blocks = Vec::new();
                build_treemap(&config, 0, rect, 0, &mut blocks);
                if detail == MapDetail::Folders {
                    assert!(blocks.len() <= TRUNCATE_DEPTH);
                } else {
                    let files: Vec<_> = blocks.iter().filter(|b| !b.is_frame).collect();
                    assert!(
                        files
                            .iter()
                            .all(|b| !nodes[b.node_idx as usize].is_directory()
                                && b.rect.width() >= 3.0
                                && b.rect.height() >= 3.0)
                    );
                    if layout == MapLayout::Stripes {
                        assert!(!files.is_empty());
                    } else {
                        assert!(files.len() > 100_000);
                    }
                    continue; // Gaps are intentional where files are below the pixel threshold.
                }
                for &branch in &branches {
                    assert!(blocks.iter().any(
                        |b| b.node_idx == branch || nodes[b.node_idx as usize].parent == branch
                    ));
                }
                // Sample the entire map, including the rightmost/bottommost regions
                // which the old traversal silently left black.
                for y in 0..20 {
                    for x in 0..40 {
                        let point = pos2(x as f32 * 50.0 + 25.0, y as f32 * 50.0 + 25.0);
                        assert!(
                            blocks.iter().any(|b| b.rect.contains(point)
                                && (!b.is_frame
                                    || point.y <= b.rect.top() + 20.0
                                    || point.x <= b.rect.left() + 2.0
                                    || point.x >= b.rect.right() - 2.0
                                    || point.y >= b.rect.bottom() - 2.0)),
                            "Unpainted {point:?}: {detail:?} {layout:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn zoom_keeps_cursor_anchor_and_culls_offscreen_files() {
        let offset = eframe::egui::vec2(-20.0, -40.0);
        let pivot = eframe::egui::vec2(100.0, 150.0);
        let new = zoom_about(offset, pivot, 2.0, 4.0);
        assert!(((pivot - offset) / 2.0 - (pivot - new) / 4.0).length() < 0.001);
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"root");
        let mut nodes = vec![
            FileNode::new(name, None, true, false, 0, 0),
            FileNode::new(name, Some(0), false, false, 0, 0),
            FileNode::new(name, Some(0), false, false, 0, 0),
        ];
        nodes[0].size = 2;
        nodes[0].first_child = 1;
        nodes[1].size = 1;
        nodes[1].next_sibling = 2;
        nodes[2].size = 1;
        let rect = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(200.0, 100.0));
        let config = TreemapConfig {
            skip_subtree: None,
            layout_cache: None,
            geometry_scale: 1.0,
            nodes: &nodes,
            string_pool: &pool,
            max_depth: 128,
            pixels_per_point: 1.0,
            directory_colors: &[],
            layout: MapLayout::Stripes,
            detail: MapDetail::Files,
            clip: Rect::from_min_max(pos2(110.0, 0.0), pos2(200.0, 100.0)),
        };
        let mut blocks = Vec::new();
        build_treemap(&config, 0, rect, 0, &mut blocks);
        let files: Vec<_> = blocks.iter().filter(|b| !b.is_frame).collect();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].node_idx, 2);
    }

    #[test]
    fn palettes_and_all_tile_styles_render_without_native_window() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"image.png");
        let mut nodes = vec![
            FileNode::new(name, None, true, false, 0, 0),
            FileNode::new(name, Some(0), false, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[0].size = 1024;
        nodes[1].size = 1024;
        let snapshot = FileArenaSnapshot {
            dir_counts: Arc::new(precompute_dir_counts(&nodes)),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            extensions: Default::default(),
        };
        let ctx = eframe::egui::Context::default();
        let mut chart = TreemapChart::new();
        chart.detail = MapDetail::Files;
        let mut selected = std::collections::HashSet::new();
        let mut expanded = std::collections::HashSet::new();
        let mut scroll = false;
        let mut context = StatContext {
            selected_nodes: &mut selected,
            expanded_nodes: &mut expanded,
            scroll_to_selected: &mut scroll,
            deduplicator_results: None,
        };
        for palette in [
            MapPalette::Pastel,
            MapPalette::Vivid,
            MapPalette::Ocean,
            MapPalette::Earth,
            MapPalette::Mono,
        ] {
            chart.palette = palette;
            for style in [
                TreemapStyle::Flat,
                TreemapStyle::VerticalGradient,
                TreemapStyle::OffsetVerticalGradient,
                TreemapStyle::DiagonalGradient,
                TreemapStyle::Cushion,
                TreemapStyle::Bevel,
                TreemapStyle::Glass,
                TreemapStyle::Neon,
                TreemapStyle::Outline,
            ] {
                chart.style = style;
                let output = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
                    chart.render(ui, &snapshot, &mut context)
                });
                assert!(!output.shapes.is_empty());
            }
        }
        let base = Color32::from_rgb(100, 150, 180);
        assert_ne!(
            palette_color(base, MapPalette::Pastel),
            palette_color(base, MapPalette::Vivid)
        );
    }
    use crate::{
        arena::{FileArenaSnapshot, FileNode, NodeStorage, StringPool, precompute_dir_counts},
        stats::StatsChart,
    };

    #[test]
    fn test_is_descendant() {
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let d_id = pool.get_or_insert(b"dir");
        let f_id = pool.get_or_insert(b"file");
        let nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(d_id, Some(0), true, false, 0, 0),
            FileNode::new(f_id, Some(1), false, false, 0, 0),
        ];

        assert!(is_descendant(&nodes, 2, 1));
        assert!(is_descendant(&nodes, 2, 0));
        assert!(is_descendant(&nodes, 1, 0));
        assert!(is_descendant(&nodes, 2, 2));
        assert!(!is_descendant(&nodes, 1, 2));
    }

    #[test]
    fn test_get_selection_roots() {
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let d_id = pool.get_or_insert(b"dir");
        let f_id = pool.get_or_insert(b"file");
        let nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(d_id, Some(0), true, false, 0, 0),
            FileNode::new(f_id, Some(1), false, false, 0, 0),
        ];

        let mut selected = std::collections::HashSet::new();
        selected.insert(1);
        selected.insert(2);

        let roots = get_selection_roots(&nodes, &selected);
        assert_eq!(roots.len(), 1);
        assert!(roots.contains(&1));
    }

    #[allow(clippy::float_cmp)]
    #[test]
    fn test_worst_aspect_ratio() {
        assert_eq!(worst_aspect_ratio(&[], 10.0), f64::INFINITY);
        assert_eq!(worst_aspect_ratio(&[10.0], 0.0), f64::INFINITY);

        let ratio = worst_aspect_ratio(&[10.0], 5.0);
        assert_eq!(ratio, 2.5);
    }

    #[test]
    fn test_treemap_compute_empty() {
        let pool = StringPool::new();
        let snapshot = FileArenaSnapshot {
            extensions: crate::extensions::ExtensionStore::default(),
            nodes: Arc::new(NodeStorage::Owned(vec![])),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(vec![]),
        };
        let mut chart = TreemapChart::new();
        chart.layout = MapLayout::Classic;
        let blocks = chart.compute(&snapshot);
        assert!(blocks.is_empty());
    }

    #[allow(clippy::float_cmp)]
    #[test]
    fn test_treemap_compute_standard() {
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let f1_id = pool.get_or_insert(b"f1.png");

        let mut nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(f1_id, Some(0), false, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[0].size = 1000;
        nodes[1].size = 1000;

        let dir_counts = precompute_dir_counts(&nodes);
        let snapshot = FileArenaSnapshot {
            extensions: crate::extensions::ExtensionStore::default(),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(dir_counts),
        };

        let mut chart = TreemapChart::new();
        chart.layout = MapLayout::Classic;
        chart.last_rect =
            Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(100.0, 100.0));
        let blocks = chart.compute(&snapshot);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].node_idx, 1);
        assert_eq!(blocks[0].rect.width(), 100.0);
        assert_eq!(blocks[0].rect.height(), 100.0);
    }

    #[allow(clippy::float_cmp)]
    #[test]
    fn test_treemap_zoom_subfolder() {
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let d1_id = pool.get_or_insert(b"dir1");
        let d2_id = pool.get_or_insert(b"dir2");
        let f1_id = pool.get_or_insert(b"f1.txt");
        let f2_id = pool.get_or_insert(b"f2.txt");

        // root -> dir1, dir2
        // dir1 -> f1.txt
        // dir2 -> f2.txt
        let mut nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(d1_id, Some(0), true, false, 0, 0),
            FileNode::new(d2_id, Some(0), true, false, 0, 0),
            FileNode::new(f1_id, Some(1), false, false, 0, 0),
            FileNode::new(f2_id, Some(2), false, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[1].next_sibling = 2;
        nodes[1].first_child = 3;
        nodes[2].first_child = 4;
        nodes[0].size = 1000;
        nodes[1].size = 600;
        nodes[2].size = 400;
        nodes[3].size = 600;
        nodes[4].size = 400;

        let dir_counts = precompute_dir_counts(&nodes);
        let snapshot = FileArenaSnapshot {
            extensions: crate::extensions::ExtensionStore::default(),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(dir_counts),
        };

        // When zoomed to dir1 (index 1), only f1.txt (index 3) should be laid out, taking 100% of the canvas
        let mut chart = TreemapChart::new();
        chart.layout = MapLayout::Classic;
        chart.zoom_root = 1;
        chart.last_rect =
            Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(200.0, 200.0));
        let blocks = chart.compute(&snapshot);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].node_idx, 3);
        assert_eq!(blocks[0].rect.width(), 200.0);
        assert_eq!(blocks[0].rect.height(), 200.0);
    }

    #[test]
    fn test_treemap_zoom_invalid_bounds_fallback() {
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let f_id = pool.get_or_insert(b"file.txt");

        let mut nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(f_id, Some(0), false, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[0].size = 500;
        nodes[1].size = 500;

        let dir_counts = precompute_dir_counts(&nodes);
        let snapshot = FileArenaSnapshot {
            extensions: crate::extensions::ExtensionStore::default(),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(dir_counts),
        };

        let mut chart = TreemapChart::new();
        chart.layout = MapLayout::Classic;
        chart.zoom_root = 9999; // Out-of-bounds index
        chart.last_rect =
            Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(100.0, 100.0));
        let blocks = chart.compute(&snapshot);

        // Safely falls back to root (index 0)
        assert_eq!(chart.zoom_root, 0);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].node_idx, 1);
    }

    #[test]
    fn test_treemap_context_menu_target_resolution() {
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let d1_id = pool.get_or_insert(b"dir1");
        let f1_id = pool.get_or_insert(b"root_file.txt");
        let f2_id = pool.get_or_insert(b"sub_file.txt");

        // root(0) -> dir1(1), root_file.txt(2)
        // dir1(1) -> sub_file.txt(3)
        let mut nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(d1_id, Some(0), true, false, 0, 0),
            FileNode::new(f1_id, Some(0), false, false, 0, 0),
            FileNode::new(f2_id, Some(1), false, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[1].next_sibling = 2;
        nodes[1].first_child = 3;
        nodes[0].size = 1000;
        nodes[1].size = 500;
        nodes[2].size = 500;
        nodes[3].size = 500;

        let dir_counts = precompute_dir_counts(&nodes);
        let snapshot = FileArenaSnapshot {
            extensions: crate::extensions::ExtensionStore::default(),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(dir_counts),
        };

        let mut chart = TreemapChart::new();
        chart.layout = MapLayout::Classic;
        assert_eq!(chart.zoom_root, 0);
        assert_eq!(chart.context_menu_target, None);

        // If context_menu_target is a root file (node 2), target is its parent (0 == zoom_root)
        chart.context_menu_target = Some(2);
        let node = &snapshot.nodes[2];
        let target = if node.is_directory() { 2 } else { node.parent };
        assert_eq!(target, 0);
        assert_eq!(target, chart.zoom_root); // Disabled when already at root

        // If context_menu_target is a directory in root (node 1), target is node 1
        chart.context_menu_target = Some(1);
        let node = &snapshot.nodes[1];
        let target = if node.is_directory() { 1 } else { node.parent };
        assert_eq!(target, 1);
        assert_ne!(target, chart.zoom_root); // Enabled to zoom into dir1
    }

    #[test]
    fn test_build_treemap_out_of_bounds_root_no_panic() {
        let pool = StringPool::new();
        let nodes: Vec<FileNode> = vec![];
        let config = TreemapConfig {
            skip_subtree: None,
            layout_cache: None,
            geometry_scale: 1.0,
            nodes: &nodes,
            string_pool: &pool,
            max_depth: 128,
            pixels_per_point: 1.0,
            directory_colors: &[],
            layout: MapLayout::Classic,
            detail: MapDetail::Files,
            clip: Rect::EVERYTHING,
        };
        let mut blocks = Vec::new();
        // A stale node_idx into an empty arena must bail out, not panic.
        build_treemap(&config, 5, Rect::NOTHING, 0, &mut blocks);
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_build_treemap_corrupt_child_index_no_panic() {
        let mut pool = StringPool::new();
        let r = pool.get_or_insert(b"r");
        let mut nodes = vec![FileNode::new(r, None, true, false, 0, 0)];
        nodes[0].size = 1000;
        nodes[0].first_child = 999_999; // out-of-bounds sibling — must not panic
        let config = TreemapConfig {
            skip_subtree: None,
            layout_cache: None,
            geometry_scale: 1.0,
            nodes: &nodes,
            string_pool: &pool,
            max_depth: 128,
            pixels_per_point: 1.0,
            directory_colors: &[],
            layout: MapLayout::Classic,
            detail: MapDetail::Files,
            clip: Rect::EVERYTHING,
        };
        let mut blocks = Vec::new();
        let rect = Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(100.0, 100.0));
        build_treemap(&config, 0, rect, 0, &mut blocks);
        // No valid children reachable → root rendered as a single fallback block.
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].node_idx, 0);
    }

    #[test]
    fn test_build_treemap_happy_path_unchanged() {
        let mut pool = StringPool::new();
        let r = pool.get_or_insert(b"r");
        let f = pool.get_or_insert(b"f.png");
        let mut nodes = vec![
            FileNode::new(r, None, true, false, 0, 0),
            FileNode::new(f, Some(0), false, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[0].size = 1000;
        nodes[1].size = 1000;
        let config = TreemapConfig {
            skip_subtree: None,
            layout_cache: None,
            geometry_scale: 1.0,
            nodes: &nodes,
            string_pool: &pool,
            max_depth: 128,
            pixels_per_point: 1.0,
            directory_colors: &[],
            layout: MapLayout::Classic,
            detail: MapDetail::Files,
            clip: Rect::EVERYTHING,
        };
        let mut blocks = Vec::new();
        let rect = Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(100.0, 100.0));
        build_treemap(&config, 0, rect, 0, &mut blocks);
        let files: Vec<_> = blocks.iter().filter(|b| !b.is_frame).collect();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].node_idx, 1);
    }

    #[test]
    fn folder_headers_reserve_space_and_stripes_keep_area_ratios() {
        let mut pool = StringPool::new();
        let name = pool.get_or_insert(b"root");
        let mut nodes = vec![
            FileNode::new(name, None, true, false, 0, 0),
            FileNode::new(name, Some(0), false, false, 0, 0),
            FileNode::new(name, Some(0), false, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[0].size = 400;
        nodes[1].next_sibling = 2;
        nodes[1].size = 300;
        nodes[2].size = 100;
        let rect = Rect::from_min_size(pos2(0.0, 0.0), eframe::egui::vec2(400.0, 300.0));
        for layout in [MapLayout::Folders, MapLayout::Stripes] {
            let config = TreemapConfig {
                skip_subtree: None,
                layout_cache: None,
                geometry_scale: 1.0,
                nodes: &nodes,
                string_pool: &pool,
                max_depth: 128,
                pixels_per_point: 1.0,
                directory_colors: &[],
                layout,
                detail: MapDetail::Files,
                clip: Rect::EVERYTHING,
            };
            let mut blocks = Vec::new();
            build_treemap(&config, 0, rect, 0, &mut blocks);
            let leaves: Vec<_> = blocks.iter().filter(|b| !b.is_frame).collect();
            assert_eq!(leaves.len(), 2);
            assert!((leaves[0].rect.area() / leaves[1].rect.area() - 3.0).abs() < 0.01);
            assert!(!leaves[0].rect.intersects(leaves[1].rect.shrink(0.01)));
            if layout == MapLayout::Folders {
                assert!(blocks[0].is_frame);
                assert!(
                    leaves
                        .iter()
                        .all(|b| b.rect.top() >= 20.0 && rect.contains_rect(b.rect))
                );
            } else {
                assert!(leaves.iter().all(|b| rect.contains_rect(b.rect)));
            }
        }
    }

    #[test]
    fn test_treemap_area_proportional_to_size() {
        // Child areas scale with size: 300 vs 100 in a 400x400 rect → ~3:1 block areas.
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let f1_id = pool.get_or_insert(b"big.bin");
        let f2_id = pool.get_or_insert(b"small.bin");

        let mut nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(f1_id, Some(0), false, false, 0, 0),
            FileNode::new(f2_id, Some(0), false, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[1].next_sibling = 2;
        nodes[0].size = 400;
        nodes[1].size = 300;
        nodes[2].size = 100;

        let dir_counts = precompute_dir_counts(&nodes);
        let snapshot = FileArenaSnapshot {
            extensions: crate::extensions::ExtensionStore::default(),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(dir_counts),
        };

        let mut chart = TreemapChart::new();
        chart.layout = MapLayout::Classic;
        chart.last_rect =
            Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(400.0, 400.0));
        let blocks = chart.compute(&snapshot);

        assert_eq!(blocks.len(), 2);
        let area_of = |idx: u32| -> f32 {
            blocks
                .iter()
                .find(|b| b.node_idx == idx)
                .map_or(0.0, |b| b.rect.width() * b.rect.height())
        };
        let big = area_of(1);
        let small = area_of(2);
        assert!(big > 0.0 && small > 0.0);
        let ratio = big / small;
        assert!(
            (ratio - 3.0).abs() / 3.0 < 0.05,
            "area ratio {ratio} deviates from 3:1"
        );
    }

    #[test]
    fn test_treemap_zero_size_child_gets_no_block() {
        // Zero-size children are excluded from the layout entirely.
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let f1_id = pool.get_or_insert(b"f1.png");
        let f2_id = pool.get_or_insert(b"f2.png");

        let mut nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(f1_id, Some(0), false, false, 0, 0),
            FileNode::new(f2_id, Some(0), false, false, 0, 0),
        ];
        nodes[0].first_child = 1;
        nodes[1].next_sibling = 2;
        nodes[0].size = 100;
        nodes[1].size = 100;
        nodes[2].size = 0;

        let dir_counts = precompute_dir_counts(&nodes);
        let snapshot = FileArenaSnapshot {
            extensions: crate::extensions::ExtensionStore::default(),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            dir_counts: Arc::new(dir_counts),
        };

        let mut chart = TreemapChart::new();
        chart.layout = MapLayout::Classic;
        chart.last_rect =
            Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(100.0, 100.0));
        let blocks = chart.compute(&snapshot);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].node_idx, 1);
        assert!(blocks.iter().all(|b| b.node_idx != 2));
    }

    #[allow(clippy::float_cmp)]
    #[test]
    fn test_worst_aspect_ratio_degenerate_inputs() {
        assert_eq!(worst_aspect_ratio(&[], 10.0), f64::INFINITY);
        assert_eq!(worst_aspect_ratio(&[10.0], 0.0), f64::INFINITY);
        assert_eq!(worst_aspect_ratio(&[10.0], -5.0), f64::INFINITY);
        assert_eq!(worst_aspect_ratio(&[0.0, 0.0], 5.0), f64::INFINITY);
    }

    #[test]
    fn test_get_selection_roots_oob_and_disjoint() {
        // root(0) → dir_a(1) → file(3); root(0) → dir_b(2).
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let a_id = pool.get_or_insert(b"dir_a");
        let b_id = pool.get_or_insert(b"dir_b");
        let f_id = pool.get_or_insert(b"file");
        let nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(a_id, Some(0), true, false, 0, 0),
            FileNode::new(b_id, Some(0), true, false, 0, 0),
            FileNode::new(f_id, Some(1), false, false, 0, 0),
        ];

        let mut selected = std::collections::HashSet::new();
        selected.insert(1);
        selected.insert(2);
        selected.insert(3);
        selected.insert(999_999); // out of bounds — must be skipped, not panic

        let roots = get_selection_roots(&nodes, &selected);
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&1));
        assert!(roots.contains(&2));
        // Node 3 sits under selected node 1, so it is not a root.
        assert!(!roots.contains(&3));
    }

    #[test]
    fn test_is_descendant_edge_cases() {
        let mut pool = StringPool::new();
        let r_id = pool.get_or_insert(b"root");
        let a_id = pool.get_or_insert(b"dir_a");
        let b_id = pool.get_or_insert(b"dir_b");
        let nodes = vec![
            FileNode::new(r_id, None, true, false, 0, 0),
            FileNode::new(a_id, Some(0), true, false, 0, 0),
            FileNode::new(b_id, Some(0), true, false, 0, 0),
        ];

        assert!(is_descendant(&nodes, 1, 1)); // reflexive
        assert!(!is_descendant(&nodes, 1, 2)); // not a descendant of its own sibling
        assert!(!is_descendant(&nodes, 2, 1));
        assert!(!is_descendant(&nodes, 999_999, 0)); // out-of-bounds child → false
    }
}
