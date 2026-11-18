use super::*;
use eframe::{egui_wgpu, wgpu};
use std::sync::Arc;

struct PreviewScanner;
impl crate::ScanController for PreviewScanner {
    fn start_scan(&self, _path: PathBuf, _same_filesystem: bool) {}
    fn num_threads(&self) -> usize {
        8
    }
}

fn capture(
    ctx: &egui::Context,
    app: &mut GuiApp,
    width: u32,
    height: u32,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &Path,
) {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(device, format, Default::default());
    let state = app.shared_state.clone();
    let snapshot = state.current_snapshot.load();
    let mut output = None;
    for frame in 0..6 {
        let result = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32, height as f32),
                )),
                time: Some(frame as f64 / 60.),
                ..Default::default()
            },
            |ui| {
                theme::setup_custom_style(ctx, app.theme);
                let scanning = app.shared_state.is_scanning.load(Ordering::Relaxed);
                app.render_shell_top(ui, &snapshot, scanning);
                app.render_shell_navigation(ui, &snapshot);
                app.render_shell_status(ui, &snapshot, scanning);
                if app.shell.page == Page::Workspace && !snapshot.nodes.is_empty() {
                    app.render_windirstat_layout(ui, &snapshot);
                    return;
                }
                egui::CentralPanel::default().show(ui, |ui| {
                    let available = ui.available_width();
                    match app.shell.page {
                        Page::Home => app.render_shell_home(ui, &snapshot),
                        Page::Settings => app.render_shell_settings(ui),
                        Page::Workspace => app.render_shell_loading(ui, scanning),
                    }
                    assert!(
                        ui.min_rect().width() <= available + 1.,
                        "page overflow: {} > {} at width {width}",
                        ui.min_rect().width(),
                        available
                    );
                });
            },
        );
        for (id, delta) in &result.textures_delta.set {
            for image in delta {
                renderer.update_texture(device, queue, *id, image);
            }
        }
        output = Some(result);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let output = output.unwrap();
    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point: output.pixels_per_point,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("UX preview"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let stride = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (stride * height) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    let buffers = renderer.update_buffers(device, queue, &mut encoder, &jobs, &screen);
    {
        let view = texture.create_view(&Default::default());
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    queue.submit(buffers.into_iter().chain([encoder.finish()]));
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in mapped.chunks(stride as usize) {
        rgba.extend_from_slice(&row[..width as usize * 4]);
    }
    image::save_buffer(path, &rgba, width, height, image::ColorType::Rgba8).unwrap();
}

#[test]
fn ux_preview_capture_and_layout() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../outputs/ux-preview-images");
    std::fs::create_dir_all(&root).unwrap();
    let mut count = 0;
    for (name, theme) in [
        ("dark", theme::ThemePreference::Dark),
        ("light", theme::ThemePreference::Light),
        ("contrast", theme::ThemePreference::HighContrast),
    ] {
        for width in [800, 1280] {
            for (page_name, page) in [("home", Page::Home), ("settings", Page::Settings)] {
                let ctx = egui::Context::default();
                egui_extras::install_image_loaders(&ctx);
                super::super::fonts::install_fonts(&ctx);
                let mut app = GuiApp::new(
                    Arc::new(crate::state::SharedState::new()),
                    Some(Arc::new(PreviewScanner)),
                    None,
                    false,
                );
                app.theme = theme;
                app.shell.page = page;
                app.shell.preferences = ShellPreferences::default();
                capture(
                    &ctx,
                    &mut app,
                    width,
                    900,
                    &device,
                    &queue,
                    &root.join(format!("{page_name}-{name}-{width}.png")),
                );
                count += 1;
            }
        }
    }
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    super::super::fonts::install_fonts(&ctx);
    let mut app = GuiApp::new(
        Arc::new(crate::state::SharedState::new()),
        Some(Arc::new(PreviewScanner)),
        None,
        false,
    );
    app.theme = theme::ThemePreference::Dark;
    app.shell.page = Page::Settings;
    for tab in 1..4 {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        super::super::fonts::install_fonts(&ctx);
        app.shell.settings_tab = tab;
        capture(
            &ctx,
            &mut app,
            1280,
            900,
            &device,
            &queue,
            &root.join(format!("settings-tab-{tab}.png")),
        );
        count += 1;
    }
    for ready in [false, true] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        super::super::fonts::install_fonts(&ctx);
        let mut app = GuiApp::new(
            Arc::new(crate::state::SharedState::new()),
            Some(Arc::new(PreviewScanner)),
            None,
            false,
        );
        app.theme = theme::ThemePreference::Dark;
        app.shell.page = Page::Workspace;
        app.current_scan_path = Some(PathBuf::from("C:\\Example"));
        if ready {
            use crate::arena::{FileNode, NodeStorage, StringPool, precompute_dir_counts};
            let mut pool = StringPool::new();
            let mut nodes = vec![FileNode::new(
                pool.get_or_insert(b"C:\\Example"),
                None,
                true,
                false,
                0,
                0,
            )];
            let mut previous_dir = None;
            for (name, ext, weight) in [
                ("Documents", "pdf", 20u64),
                ("Media", "mp4", 50),
                ("Applications", "exe", 30),
                ("Projects", "rs", 80),
            ] {
                let dir = nodes.len() as u32;
                nodes.push(FileNode::new(
                    pool.get_or_insert(name.as_bytes()),
                    Some(0),
                    true,
                    false,
                    0,
                    0,
                ));
                if let Some(previous) = previous_dir {
                    nodes[previous as usize].next_sibling = dir;
                } else {
                    nodes[0].first_child = dir;
                }
                previous_dir = Some(dir);
                nodes[dir as usize].first_child = dir + 1;
                let mut total = 0;
                for i in 0..60 {
                    let size = (60 - i) as u64 * weight * 1024 * 1024;
                    let mut file = FileNode::new(
                        pool.get_or_insert(format!("file-{i}.{ext}").as_bytes()),
                        Some(dir),
                        false,
                        false,
                        0,
                        0,
                    );
                    file.size = size;
                    if i < 59 {
                        file.next_sibling = nodes.len() as u32 + 1;
                    }
                    nodes.push(file);
                    total += size;
                }
                nodes[dir as usize].size = total;
                nodes[0].size += total;
            }
            let counts = Arc::new(precompute_dir_counts(&nodes));
            app.shared_state
                .current_snapshot
                .store(Arc::new(FileArenaSnapshot {
                    nodes: Arc::new(NodeStorage::Owned(nodes)),
                    string_pool: Arc::new(pool),
                    dir_counts: counts,
                    extensions: Default::default(),
                }));
            app.shared_state
                .scan_stats
                .files_scanned
                .store(240, Ordering::Relaxed);
            app.shared_state
                .scan_stats
                .dirs_scanned
                .store(5, Ordering::Relaxed);
        } else {
            app.shared_state.is_scanning.store(true, Ordering::Relaxed);
            app.shared_state
                .scan_stats
                .reading_mft
                .store(true, Ordering::Relaxed);
            app.shared_state
                .scan_stats
                .mft_total_records
                .store(100000, Ordering::Relaxed);
            app.shared_state
                .scan_stats
                .mft_records
                .store(45000, Ordering::Relaxed);
            app.shared_state
                .scan_stats
                .metadata_bytes
                .store(46080000, Ordering::Relaxed);
        }
        capture(
            &ctx,
            &mut app,
            1280,
            900,
            &device,
            &queue,
            &root.join(if ready {
                "workspace-dark.png"
            } else {
                "loading-dark.png"
            }),
        );
        count += 1;
    }
    let mut shell = ShellState::load();
    shell.preferences = ShellPreferences::default();
    for i in 0..10 {
        shell.remember(Path::new(&format!("C:\\folder-{i}")));
    }
    shell.remember(Path::new("C:\\folder-5"));
    assert_eq!(shell.preferences.recent.len(), 8);
    assert_eq!(shell.preferences.recent[0], "C:\\folder-5");
    let serialized = toml::to_string(&shell.preferences).unwrap();
    assert!(toml::from_str::<ShellPreferences>(&serialized).unwrap() == shell.preferences);
    println!(
        "UX_PREVIEW: {count} actual GPU captures, 800/1280 widths, dark/light/contrast, all settings tabs, no content overflow; recent-path dedup/bound and preference roundtrip passed. Images: {}",
        root.display()
    );
}
