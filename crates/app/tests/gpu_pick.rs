//! The map shader and the CPU picking code must agree on which cell is under each pixel; otherwise
//! clicking the map would select something other than what is drawn there. This renders cell ids
//! with the real shader on the GPU and compares them with `Camera::pick`.
//!
//! The shader works in 32-bit floats and the CPU in 64-bit, so a pixel right on a cell boundary
//! may land on either side. A mismatch is accepted only when the two cells touch, and such pixels
//! must stay rare. Skips (with a message) when there is no GPU.

use app::gpu::{self, LayerSettings, SHADER};
use app::projection::{Camera, Projection};
use eframe::egui_wgpu::wgpu;
use sim::grid::{Grid, NONE};

const W: u32 = 256;
const H: u32 = 144;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// Cell id per pixel, as the shader computes it.
fn gpu_cells(device: &wgpu::Device, queue: &wgpu::Queue, u: &gpu::MapUniforms) -> Vec<u32> {
    use wgpu::util::DeviceExt as _;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("map"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let format = wgpu::TextureFormat::R32Uint;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("pick"),
        layout: None,
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_fullscreen"), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_pick"),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniforms"),
        contents: bytemuck::bytes_of(u),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pick"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cells"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (W * H * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let view = target.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pick"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: Some(H) } },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    readback.map_async(wgpu::MapMode::Read, .., |r| r.expect("map readback"));
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = readback.get_mapped_range(..).expect("mapped range");
    bytemuck::cast_slice(&data).to_vec()
}

#[test]
fn gpu_and_cpu_pick_the_same_cells() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter: skipping the GPU picking test");
        return;
    };
    let grid = Grid::new(96);
    let (w, h) = (W as f64, H as f64);
    // (projection, central meridian, latitude at the centre, zoom)
    let views = [
        (Projection::EqualEarth, 0.0, 0.0, 0.0),
        (Projection::EqualEarth, 2.9, 0.5, 3.0),
        (Projection::EqualEarth, -1.0, -1.35, 6.0),
        (Projection::Rectangle, 0.0, 0.0, 0.0),
        (Projection::Rectangle, 3.0, 1.2, 4.0),
        (Projection::Rectangle, 0.8, 0.62, 0.6),
    ];
    for (projection, lon0, lat, zoom) in views {
        let mut cam = Camera::fit(projection, w, h);
        cam.lon0 = lon0;
        cam.cy = projection.forward(0.0, lat).1;
        if zoom > 0.0 {
            cam.scale = cam.reference_scale(h) * zoom;
        }
        let u = gpu::uniforms(&cam, grid.n, w, h, (0.0, 0.0), 1.0, &LayerSettings::default(), NONE);
        let got = gpu_cells(&device, &queue, &u);
        let (mut same, mut touching, mut on_map) = (0, 0, 0);
        for y in 0..H {
            for x in 0..W {
                let g = got[(y * W + x) as usize];
                let c = cam.pick(&grid, x as f64 + 0.5, y as f64 + 0.5, w, h).map_or(NONE, |c| c as u32);
                on_map += (c != NONE) as u32;
                if g == c {
                    same += 1;
                } else if g != NONE && c != NONE && grid.neighbors8(c as usize).any(|m| m as u32 == g) {
                    touching += 1;
                } else {
                    // On and off the map may only disagree right at the oval's edge.
                    let near_edge = [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)]
                        .iter()
                        .any(|&(dx, dy)| cam.pick(&grid, x as f64 + 0.5 + dx, y as f64 + 0.5 + dy, w, h).is_none() != (c == NONE));
                    assert!(near_edge, "{projection:?} lon0 {lon0}: pixel ({x}, {y}) GPU cell {g}, CPU cell {c}");
                    touching += 1;
                }
            }
        }
        let total = W * H;
        assert!(on_map > total / 4, "{projection:?}: the view should show plenty of map");
        assert!(touching * 100 <= total, "{projection:?} lon0 {lon0}: {touching} of {total} pixels disagree");
        eprintln!("{projection:?} lon0 {lon0} zoom {zoom}: {same} same, {touching} on a boundary");
    }
}
