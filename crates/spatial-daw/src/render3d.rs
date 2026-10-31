//! Offscreen wgpu renderer for the spatial mix field.
//!
//! The scene is drawn into an sRGB texture and shown in egui, which gives us a
//! real depth buffer (egui's own render pass has none). Spheres are camera
//! facing impostors, so one unit quad plus an instance buffer is enough.

use egui::{Pos2, Rect, TextureId};
use egui_wgpu::wgpu;
use glam::{Mat4, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: 0.72,
            pitch: 0.34,
            distance: 17.5,
        }
    }
}

impl Camera {
    pub fn eye(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        ) * self.distance
    }

    pub fn matrices(&self, aspect: f32) -> (Mat4, Mat4, Mat4) {
        let eye = self.eye();
        let view = look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        let proj = perspective_rh(0.80, aspect.max(0.05), 0.05, 120.0);
        (proj * view, view, proj)
    }

    pub fn project(&self, world: Vec3, rect: Rect) -> Option<Pos2> {
        let (view_proj, _, _) = self.matrices(rect.width() / rect.height().max(1.0));
        let clip = view_proj * world.extend(1.0);
        if clip.w <= 0.0001 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.z < 0.0 || ndc.z > 1.0 || !ndc.x.is_finite() || !ndc.y.is_finite() {
            return None;
        }
        Some(Pos2::new(
            rect.left() + (ndc.x * 0.5 + 0.5) * rect.width(),
            rect.top() + (1.0 - (ndc.y * 0.5 + 0.5)) * rect.height(),
        ))
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.006).clamp(-1.45, 1.45);
    }

    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance * (1.0 - delta * 0.0015)).clamp(6.0, 45.0);
    }

    pub fn preset_3d(&mut self) {
        *self = Self::default();
    }

    pub fn preset_top(&mut self) {
        self.yaw = -1.5708;
        self.pitch = 1.45;
        self.distance = 18.0;
    }

    pub fn preset_front(&mut self) {
        self.yaw = 1.5708;
        self.pitch = 0.04;
        self.distance = 18.0;
    }

    pub fn preset_side(&mut self) {
        self.yaw = 0.0;
        self.pitch = 0.04;
        self.distance = 18.0;
    }
}

/// Maps pan/frequency/depth into the visual room.
fn look_at_rh(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let f = (target - eye).normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    Mat4::from_cols(
        glam::Vec4::new(s.x, u.x, -f.x, 0.0),
        glam::Vec4::new(s.y, u.y, -f.y, 0.0),
        glam::Vec4::new(s.z, u.z, -f.z, 0.0),
        glam::Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    )
}

fn perspective_rh(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    Mat4::from_cols(
        glam::Vec4::new(f / aspect, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, f, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, far / (near - far), -1.0),
        glam::Vec4::new(0.0, 0.0, (near * far) / (near - far), 0.0),
    )
}

pub fn world_position(pan: f32, frequency_height: f32, depth: f32) -> Vec3 {
    Vec3::new(
        pan.clamp(-1.0, 1.0) * 5.0,
        -3.0 + frequency_height.clamp(0.0, 1.0) * 6.0,
        (depth.clamp(0.0, 1.0) - 0.5) * 12.0,
    )
}

#[derive(Debug, Clone, Copy)]
pub struct Sphere {
    pub center: Vec3,
    pub radius: f32,
    pub color: [f32; 4],
}

#[derive(Default)]
pub struct Scene {
    pub spheres: Vec<Sphere>,
    pub ghosts: Vec<Sphere>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    center: [f32; 3],
    radius: f32,
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LineVertex {
    pos: [f32; 3],
    color: [f32; 4],
}

const MAX_INSTANCES: usize = 2048;
const MAX_LINE_VERTICES: usize = 8192;

pub struct Mixer3D {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sphere_pipeline: wgpu::RenderPipeline,
    ghost_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    ghost_buffer: wgpu::Buffer,
    line_buffer: wgpu::Buffer,
    line_count: u32,
    texture: Option<wgpu::TextureView>,
    texture_id: Option<TextureId>,
    size: (u32, u32),
    depth: Option<wgpu::TextureView>,
}

impl Mixer3D {
    pub fn new(render_state: &egui_wgpu::RenderState) -> Self {
        let device = render_state.device.clone();
        let queue = render_state.queue.clone();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("spatial scene shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mixer3d.wgsl").into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spatial uniforms layout"),
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
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spatial uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spatial uniforms bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("spatial pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 12,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 2,
                },
            ],
        };
        let line_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 12,
                    shader_location: 1,
                },
            ],
        };

        let target_format = wgpu::TextureFormat::Rgba8Unorm;
        let sphere_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("spatial spheres"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sphere"),
                compilation_options: Default::default(),
                buffers: &[Some(instance_layout.clone())],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sphere"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let ghost_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("spatial ghosts"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sphere"),
                compilation_options: Default::default(),
                buffers: &[Some(instance_layout.clone())],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sphere"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("spatial lines"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_line"),
                compilation_options: Default::default(),
                buffers: &[Some(line_layout)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_line"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sphere instances"),
            size: (MAX_INSTANCES * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ghost_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ghost instances"),
            size: (MAX_INSTANCES * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (lines, line_count) = room_lines();
        let line_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("room lines"),
            size: (lines.len() * std::mem::size_of::<LineVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&line_buffer, 0, bytemuck::cast_slice(&lines));

        Self {
            device,
            queue,
            sphere_pipeline,
            ghost_pipeline,
            line_pipeline,
            bind_group,
            uniform_buffer,
            instance_buffer,
            ghost_buffer,
            line_buffer,
            line_count,
            texture: None,
            texture_id: None,
            size: (0, 0),
            depth: None,
        }
    }

    fn ensure_target(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        width: u32,
        height: u32,
    ) -> TextureId {
        if self.texture.is_none() || self.size != (width, height) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("spatial field"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let depth = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("spatial depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
            let id = match self.texture_id {
                Some(id) => {
                    render_state
                        .renderer
                        .write()
                        .update_egui_texture_from_wgpu_texture(
                            &self.device,
                            &view,
                            wgpu::FilterMode::Linear,
                            id,
                        );
                    id
                }
                None => render_state.renderer.write().register_native_texture(
                    &self.device,
                    &view,
                    wgpu::FilterMode::Linear,
                ),
            };
            self.texture = Some(view);
            self.depth = Some(depth_view);
            self.texture_id = Some(id);
            self.size = (width, height);
        }
        self.texture_id.unwrap_or(TextureId::Managed(0))
    }

    /// Renders the scene and returns the egui texture holding it.
    pub fn render(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        physical_size: (u32, u32),
        camera: &Camera,
        scene: &Scene,
    ) -> TextureId {
        let width = physical_size.0.max(16);
        let height = physical_size.1.max(16);
        let id = self.ensure_target(render_state, width, height);
        let aspect = width as f32 / height as f32;
        let (view_proj, view, proj) = camera.matrices(aspect);
        let uniforms = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            proj: proj.to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let spheres: Vec<Instance> = scene
            .spheres
            .iter()
            .take(MAX_INSTANCES)
            .map(|s| Instance {
                center: s.center.to_array(),
                radius: s.radius,
                color: s.color,
            })
            .collect();
        let ghosts: Vec<Instance> = scene
            .ghosts
            .iter()
            .take(MAX_INSTANCES)
            .map(|s| Instance {
                center: s.center.to_array(),
                radius: s.radius,
                color: s.color,
            })
            .collect();
        if !spheres.is_empty() {
            self.queue
                .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&spheres));
        }
        if !ghosts.is_empty() {
            self.queue
                .write_buffer(&self.ghost_buffer, 0, bytemuck::cast_slice(&ghosts));
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("spatial encoder"),
            });
        {
            let color_view = self.texture.as_ref().unwrap();
            let depth_view = self.depth.as_ref().unwrap();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("spatial pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.025,
                            g: 0.035,
                            b: 0.055,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);

            if !ghosts.is_empty() {
                pass.set_pipeline(&self.ghost_pipeline);
                pass.set_vertex_buffer(0, self.ghost_buffer.slice(..));
                pass.draw(0..6, 0..ghosts.len() as u32);
            }

            pass.set_pipeline(&self.line_pipeline);
            pass.set_vertex_buffer(0, self.line_buffer.slice(..));
            pass.draw(0..self.line_count, 0..1);

            if !spheres.is_empty() {
                pass.set_pipeline(&self.sphere_pipeline);
                pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
                pass.draw(0..6, 0..spheres.len() as u32);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        id
    }
}

fn push_line(lines: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [f32; 4]) {
    lines.push(LineVertex {
        pos: a.to_array(),
        color,
    });
    lines.push(LineVertex {
        pos: b.to_array(),
        color,
    });
}

fn room_lines() -> (Vec<LineVertex>, u32) {
    let mut lines = Vec::new();
    let floor = -3.0;
    let grid = [0.18, 0.24, 0.34, 0.65];
    for i in -6..=6 {
        let x = i as f32;
        push_line(
            &mut lines,
            Vec3::new(x, floor, -6.0),
            Vec3::new(x, floor, 6.0),
            grid,
        );
        push_line(
            &mut lines,
            Vec3::new(-6.0, floor, x),
            Vec3::new(6.0, floor, x),
            grid,
        );
    }
    let shell = [0.22, 0.3, 0.42, 0.55];
    for (x, z) in [(-6.0, -6.0), (6.0, -6.0), (6.0, 6.0), (-6.0, 6.0)] {
        push_line(
            &mut lines,
            Vec3::new(x, floor, z),
            Vec3::new(x, 3.0, z),
            shell,
        );
    }
    push_line(
        &mut lines,
        Vec3::new(-6.0, 3.0, -6.0),
        Vec3::new(6.0, 3.0, -6.0),
        shell,
    );
    push_line(
        &mut lines,
        Vec3::new(-6.0, 3.0, 6.0),
        Vec3::new(6.0, 3.0, 6.0),
        shell,
    );
    push_line(
        &mut lines,
        Vec3::new(-6.0, 3.0, -6.0),
        Vec3::new(-6.0, 3.0, 6.0),
        shell,
    );
    push_line(
        &mut lines,
        Vec3::new(6.0, 3.0, -6.0),
        Vec3::new(6.0, 3.0, 6.0),
        shell,
    );

    // Pan axis (red), frequency axis (green), depth axis (blue).
    push_line(
        &mut lines,
        Vec3::new(-6.0, floor + 0.02, 0.0),
        Vec3::new(6.0, floor + 0.02, 0.0),
        [0.85, 0.3, 0.3, 0.9],
    );
    push_line(
        &mut lines,
        Vec3::new(0.0, floor, 0.0),
        Vec3::new(0.0, 3.0, 0.0),
        [0.25, 0.8, 0.45, 0.9],
    );
    push_line(
        &mut lines,
        Vec3::new(0.0, floor + 0.02, -6.0),
        Vec3::new(0.0, floor + 0.02, 6.0),
        [0.3, 0.55, 0.95, 0.9],
    );
    // Listener marker.
    push_line(
        &mut lines,
        Vec3::new(-0.5, floor + 0.05, 0.0),
        Vec3::new(0.5, floor + 0.05, 0.0),
        [0.95, 0.8, 0.3, 1.0],
    );
    push_line(
        &mut lines,
        Vec3::new(0.0, floor + 0.05, -0.5),
        Vec3::new(0.0, floor + 0.05, 0.5),
        [0.95, 0.8, 0.3, 1.0],
    );

    let count = lines.len() as u32;
    (lines, count)
}
