//! Offscreen wgpu: adapter without surface, egui-wgpu `Renderer` into an `Rgba8Unorm` texture, read back.

use egui::epaint::{textures::TexturesDelta, ClippedPrimitive};
use egui::Color32;

use crate::{block_on, Image, ShotError};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub(crate) fn adapter() -> Result<wgpu::Adapter, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .map_err(|e| e.to_string())
}

pub(crate) struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
}

impl Gpu {
    pub(crate) fn new() -> Result<Self, ShotError> {
        let adapter = adapter().map_err(ShotError::NoGpu)?;
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("margin-shot"),
            ..Default::default()
        }))
        .map_err(|e| ShotError::NoGpu(e.to_string()))?;
        let renderer = egui_wgpu::Renderer::new(
            &device,
            FORMAT,
            egui_wgpu::RendererOptions {
                dithering: false,
                ..Default::default()
            },
        );
        Ok(Self {
            device,
            queue,
            renderer,
        })
    }

    pub(crate) fn render(
        &mut self,
        prims: &[ClippedPrimitive],
        mut delta: TexturesDelta,
        [w, h]: [u32; 2],
        ppp: f32,
        clear: Color32,
    ) -> Result<Image, ShotError> {
        let mut sets: Vec<_> = delta.set.drain().collect();
        sets.sort_by_key(|(id, _)| *id);
        for (id, deltas) in sets {
            for d in deltas {
                self.renderer
                    .update_texture(&self.device, &self.queue, id, &d);
            }
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("margin-shot-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: ppp,
        };
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("margin-shot"),
            });
        let user_bufs =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut enc, prims, &screen);
        {
            let [r, g, b, a] = clear.to_normalized_gamma_f32();
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("margin-shot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(r),
                            g: f64::from(g),
                            b: f64::from(b),
                            a: f64::from(a),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), prims, &screen);
        }
        let unpadded = w * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("margin-shot-readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue
            .submit(user_bufs.into_iter().chain([enc.finish()]));
        for id in delta.free.drain() {
            self.renderer.free_texture(&id);
        }

        let (tx, rx) = std::sync::mpsc::channel();
        buf.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|e| ShotError::Gpu(e.to_string()))?;
        rx.recv()
            .map_err(|e| ShotError::Gpu(e.to_string()))?
            .map_err(|e| ShotError::Gpu(e.to_string()))?;
        let mut rgba = Vec::with_capacity((unpadded * h) as usize);
        {
            let mapped = buf
                .get_mapped_range(..)
                .map_err(|e| ShotError::Gpu(e.to_string()))?;
            for row in mapped.chunks(padded as usize) {
                rgba.extend_from_slice(&row[..unpadded as usize]);
            }
        }
        buf.unmap();
        Ok(Image { w, h, rgba })
    }
}
