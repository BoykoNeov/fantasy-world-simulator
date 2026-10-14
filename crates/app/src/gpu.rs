//! GPU side of the map: layer textures, the line buffer, and the two drawing steps (the map itself,
//! then lines over it). Plain wgpu, so the picking test can build it without a window.

use crate::geometry::{Kind, LineInstance};
use crate::layers::{self, Rgba, TABLE_LAYERS, TABLE_WIDTH};
use crate::projection::Camera;
use eframe::egui_wgpu::wgpu;
use eframe::egui_wgpu::wgpu::util::DeviceExt as _;
use sim::sim::Sim;
use std::ops::Range;

pub const SHADER: &str = include_str!("map.wgsl");

/// Matches `MapParams` in map.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MapUniforms {
    pub view: [f32; 4],
    pub screen: [f32; 4],
    pub proj: [f32; 4],
    pub layers0: [f32; 4],
    pub layers1: [f32; 4],
    pub layers2: [f32; 4],
    pub ids: [u32; 4],
}

/// How strongly each layer shows, 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerSettings {
    pub terrain: f32,
    pub temperature: f32,
    pub rainfall: f32,
    pub population: f32,
    pub famine: f32,
    pub provinces: f32,
    pub coasts: bool,
    pub rivers: bool,
    pub borders: bool,
    pub graticule: bool,
}

impl Default for LayerSettings {
    fn default() -> Self {
        LayerSettings {
            terrain: 1.0,
            temperature: 0.0,
            rainfall: 0.0,
            population: 0.0,
            famine: 0.0,
            provinces: 0.0,
            coasts: true,
            rivers: true,
            borders: true,
            graticule: true,
        }
    }
}

/// Uniforms for one frame.
#[allow(clippy::too_many_arguments)]
pub fn uniforms(cam: &Camera, n: usize, w: f64, h: f64, origin: (f64, f64), ppp: f64, l: &LayerSettings, hover_province: u32) -> MapUniforms {
    let on = |b: bool| if b { 1.0 } else { 0.0 };
    let border_alpha = if l.borders { cam.province_line_alpha(h) as f32 } else { 0.0 };
    MapUniforms {
        view: [cam.lon0 as f32, cam.cy as f32, cam.scale as f32, n as f32],
        screen: [w as f32, h as f32, origin.0 as f32, origin.1 as f32],
        proj: [cam.projection.index() as f32, cam.visible_copies(w).0 as f32, ppp as f32, 0.0],
        layers0: [l.terrain, l.temperature, l.rainfall, l.population],
        layers1: [l.famine, l.provinces, on(l.coasts), on(l.rivers)],
        layers2: [border_alpha, on(l.graticule), 0.0, 0.0],
        ids: [hover_province, 0, 0, 0],
    }
}

/// Everything the GPU needs from a world, built on the CPU.
pub struct MapData {
    pub n: usize,
    pub provinces: usize,
    pub terrain: Vec<Rgba>,
    pub fields: Vec<Rgba>,
    pub province_ids: Vec<u32>,
    pub tables: Vec<Rgba>,
    pub lines: Vec<LineInstance>,
    pub line_ranges: [Range<u32>; 4],
}

impl MapData {
    pub fn build(sim: &Sim) -> Self {
        let w = &sim.world;
        let halo = layers::halo_cells(&w.grid);
        let (lines, line_ranges) = crate::geometry::instances(&crate::geometry::all_lines(w));
        MapData {
            n: w.grid.n,
            provinces: w.provinces.count,
            terrain: layers::spread(&halo, &layers::terrain(w)),
            fields: layers::spread(&halo, &layers::fields(w)),
            province_ids: layers::spread(&halo, &w.provinces.of_cell),
            tables: layers::province_tables(sim),
            lines,
            line_ranges,
        }
    }
}

pub struct MapGpu {
    pub map_pipeline: wgpu::RenderPipeline,
    /// Layout of `bind_group`, for extra pipelines over the same data (the picking test).
    pub layout: wgpu::PipelineLayout,
    pub line_pipeline: wgpu::RenderPipeline,
    pub bind_group: wgpu::BindGroup,
    pub uniforms: wgpu::Buffer,
    pub lines: wgpu::Buffer,
    pub line_ranges: [Range<u32>; 4],
    pub tables: wgpu::Texture,
    table_rows: u32,
}

fn array_texture(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, size: (u32, u32, u32), format: wgpu::TextureFormat, data: &[u8]) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: size.2 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        data,
    )
}

impl MapGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, target: wgpu::TextureFormat, data: &MapData) -> Self {
        let m = (data.n + 2) as u32;
        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        let terrain = array_texture(device, queue, "terrain", (m, m, 6), rgba, bytemuck::cast_slice(&data.terrain));
        let fields = array_texture(device, queue, "fields", (m, m, 6), rgba, bytemuck::cast_slice(&data.fields));
        let provinces = array_texture(device, queue, "provinces", (m, m, 6), wgpu::TextureFormat::R32Uint, bytemuck::cast_slice(&data.province_ids));
        let table_rows = layers::table_rows(data.provinces) as u32;
        let tables = array_texture(device, queue, "province tables", (TABLE_WIDTH as u32, table_rows, TABLE_LAYERS as u32), rgba, bytemuck::cast_slice(&data.tables));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("smooth"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let array_view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });

        let tex_entry = |binding: u32, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                tex_entry(1, float),
                tex_entry(2, float),
                tex_entry(3, wgpu::TextureSampleType::Uint),
                tex_entry(4, float),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map uniforms"),
            size: std::mem::size_of::<MapUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&array_view(&terrain)) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&array_view(&fields)) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&array_view(&provinces)) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&array_view(&tables)) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("map"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout>], blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), buffers, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: target, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let map_pipeline = pipeline("map", "vs_fullscreen", "fs_map", &[], None);
        let line_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Uint32, 3 => Float32, 4 => Float32],
        };
        let line_pipeline = pipeline("lines", "vs_line", "fs_line", &[Some(line_layout)], Some(wgpu::BlendState::ALPHA_BLENDING));
        // A buffer can't be empty; one invisible segment keeps the code path uniform.
        let line_data: &[LineInstance] = if data.lines.is_empty() { &[LineInstance { kind: Kind::Graticule as u32, ..Default::default() }] } else { &data.lines };
        let lines = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("lines"),
            contents: bytemuck::cast_slice(line_data),
            usage: wgpu::BufferUsages::VERTEX,
        });

        MapGpu { map_pipeline, layout, line_pipeline, bind_group, uniforms, lines, line_ranges: data.line_ranges.clone(), tables, table_rows }
    }

    /// Re-upload the per-province tables (after the simulation advanced). The per-cell layers are
    /// untouched.
    pub fn upload_tables(&self, queue: &wgpu::Queue, tables: &[Rgba]) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.tables, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(tables),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(TABLE_WIDTH as u32 * 4), rows_per_image: Some(self.table_rows) },
            wgpu::Extent3d { width: TABLE_WIDTH as u32, height: self.table_rows, depth_or_array_layers: TABLE_LAYERS as u32 },
        );
    }

    /// Draw the map, then the lines whose layer is visible. `copies` is how many copies of the
    /// world the lines are drawn for (1 except on the repeating rectangle).
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>, u: &MapUniforms, copies: u32) {
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_pipeline(&self.map_pipeline);
        pass.draw(0..3, 0..1);
        pass.set_pipeline(&self.line_pipeline);
        pass.set_vertex_buffer(0, self.lines.slice(..));
        let visible = [u.layers1[2] > 0.0, u.layers2[0] > 0.0, u.layers1[3] > 0.0, u.layers2[1] > 0.0];
        for kind in [Kind::Graticule, Kind::Province, Kind::River, Kind::Coast] {
            let range = &self.line_ranges[kind as usize];
            if visible[kind as usize] && !range.is_empty() {
                pass.draw(0..6 * copies, range.clone());
            }
        }
    }
}
