//! Instanced tiny-file rendering. Layout and picking stay on the CPU; the GPU
//! shades each covered pixel without allocating/uploading a viewport-sized image.
use eframe::{egui, egui_wgpu, wgpu};
use wgpu::util::DeviceExt;

#[derive(Clone, Copy)]
struct Target(wgpu::TextureFormat);

pub fn initialize(ctx: &egui::Context, format: wgpu::TextureFormat) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("treemap-gpu-target"), Target(format)));
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Tile {
    pub original: [f32; 4],
    pub color: u32,
    pub border: u32,
    pub transition: f32,
    pub style: u32,
}

pub(super) fn target(ctx: &egui::Context) -> Option<wgpu::TextureFormat> {
    ctx.data(|data| data.get_temp::<Target>(egui::Id::new("treemap-gpu-target")))
        .map(|t| t.0)
}

pub(super) fn paint(
    painter: &egui::Painter,
    viewport: egui::Rect,
    format: wgpu::TextureFormat,
    slot: usize,
    tiles: std::sync::Arc<Vec<Tile>>,
) {
    if tiles.is_empty() {
        return;
    }
    painter.add(egui_wgpu::Callback::new_paint_callback(
        viewport,
        Draw {
            format,
            slot,
            tiles,
            viewport,
        },
    ));
}

struct Draw {
    format: wgpu::TextureFormat,
    slot: usize,
    tiles: std::sync::Arc<Vec<Tile>>,
    viewport: egui::Rect,
}
struct Batch {
    instances: wgpu::Buffer,
    capacity: usize,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    uploaded: Option<std::sync::Arc<Vec<Tile>>>,
}
struct Resources {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    batches: [Option<Batch>; 2],
}

impl Resources {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("treemap tiny files"),
            source: wgpu::ShaderSource::Wgsl(include_str!("treemap_gpu.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("treemap uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("treemap layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("treemap instances"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module, entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Tile>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Uint32, 2 => Uint32, 3 => Float32, 4 => Uint32],
                })], compilation_options: Default::default(),
            },
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module, entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL,
                })], compilation_options: Default::default(),
            }), multiview_mask: None, cache: None,
        });
        Self {
            pipeline,
            layout,
            batches: [None, None],
        }
    }
}

impl egui_wgpu::CallbackTrait for Draw {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<Resources>().is_none() {
            resources.insert(Resources::new(device, self.format));
        }
        let resources = resources.get_mut::<Resources>().unwrap();
        let bytes = bytemuck::cast_slice(self.tiles.as_slice());
        let batch = &mut resources.batches[self.slot];
        if batch
            .as_ref()
            .is_none_or(|batch| batch.capacity < bytes.len())
        {
            let capacity = bytes.len().next_power_of_two().max(256);
            let instances = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("treemap tile instances"),
                size: capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("treemap viewport"),
                contents: &[0; 16],
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("treemap viewport"),
                layout: &resources.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                }],
            });
            *batch = Some(Batch {
                instances,
                capacity,
                uniforms,
                bind_group,
                uploaded: None,
            });
        }
        let batch = batch.as_mut().unwrap();
        let size = self.viewport.size() * screen.pixels_per_point;
        if batch
            .uploaded
            .as_ref()
            .is_none_or(|old| !std::sync::Arc::ptr_eq(old, &self.tiles))
        {
            queue.write_buffer(&batch.instances, 0, bytes);
            batch.uploaded = Some(self.tiles.clone());
        }
        queue.write_buffer(
            &batch.uniforms,
            0,
            bytemuck::cast_slice(&[
                size.x,
                size.y,
                if self.format.is_srgb() { 1.0 } else { 0.0 },
                0.0,
            ]),
        );
        Vec::new()
    }
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let resources = resources.get::<Resources>().unwrap();
        let batch = resources.batches[self.slot].as_ref().unwrap();
        pass.set_pipeline(&resources.pipeline);
        pass.set_bind_group(0, &batch.bind_group, &[]);
        pass.set_vertex_buffer(0, batch.instances.slice(..));
        pass.draw(0..6, 0..self.tiles.len() as u32);
    }
}

#[cfg(test)]
pub(super) struct GpuCheck {
    device: wgpu::Device,
    queue: wgpu::Queue,
    resources: egui_wgpu::CallbackResources,
}
#[cfg(test)]
impl GpuCheck {
    pub fn new() -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&Default::default())).expect("GPU adapter");
        println!("GPU_CHECK adapter: {:?}", adapter.get_info());
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        Self {
            device,
            queue,
            resources: Default::default(),
        }
    }
    pub fn render(&mut self, tiles: Vec<Tile>, width: u32, height: u32) -> Vec<egui::Color32> {
        use egui_wgpu::CallbackTrait;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("treemap offscreen check"),
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
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (stride * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let draw = Draw {
            format,
            slot: 0,
            tiles: std::sync::Arc::new(tiles),
            viewport: egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width as f32, height as f32),
            ),
        };
        draw.prepare(
            &self.device,
            &self.queue,
            &egui_wgpu::ScreenDescriptor {
                size_in_pixels: [width, height],
                pixels_per_point: 1.,
            },
            &mut encoder,
            &mut self.resources,
        );
        {
            let view = texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            });
            let resources = self.resources.get::<Resources>().unwrap();
            let batch = resources.batches[0].as_ref().unwrap();
            pass.set_pipeline(&resources.pipeline);
            pass.set_bind_group(0, &batch.bind_group, &[]);
            pass.set_vertex_buffer(0, batch.instances.slice(..));
            pass.draw(0..6, 0..draw.tiles.len() as u32);
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
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).unwrap();
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = buffer.slice(..).get_mapped_range().unwrap();
        let mut result = Vec::new();
        for row in mapped.chunks(stride as usize) {
            for pixel in row[..width as usize * 4].chunks_exact(4) {
                result.push(egui::Color32::from_rgba_premultiplied(
                    pixel[0], pixel[1], pixel[2], pixel[3],
                ));
            }
        }
        result
    }
}
