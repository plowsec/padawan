//! Headless egui: run frames without a window, feed input, render to a PNG (offscreen wgpu).
//! For tests, screenshots and frame-time checks. Dev-dependency only.
//!
//! ```ignore
//! let mut shot = Shot::new([1200.0, 800.0], 2.0);
//! theme.apply(shot.ctx()); margin_theme::fonts::install(shot.ctx());
//! shot.run(3, |ui| my_widget(ui));               // a few frames to settle
//! shot.key(egui::Key::A, egui::Modifiers::COMMAND);
//! shot.text("hello");
//! shot.run(1, |ui| my_widget(ui));
//! match shot.png("/abs/out.png") { Err(ShotError::NoGpu(why)) => eprintln!("skip: {why}"), r => r.unwrap() }
//! ```

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2, ViewportId};

mod gpu;
pub mod png;

#[derive(Debug)]
pub enum ShotError {
    /// No adapter (CI without a GPU): skip the screenshot.
    NoGpu(String),
    Gpu(String),
    /// `png()` before any frame.
    NoFrame,
    Io(std::io::Error),
}

impl std::fmt::Display for ShotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShotError::NoGpu(e) => write!(f, "no GPU adapter: {e}"),
            ShotError::Gpu(e) => write!(f, "GPU error: {e}"),
            ShotError::NoFrame => f.write_str("no frame rendered yet"),
            ShotError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ShotError {}

/// RGBA8 pixels, row-major.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// `[r, g, b, a]` at pixel `(x, y)`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.w as usize + x as usize) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

struct Frame {
    shapes: Vec<egui::epaint::ClippedShape>,
    pixels_per_point: f32,
}

pub struct Shot {
    ctx: egui::Context,
    size: Vec2,
    ppp: f32,
    time: f64,
    queued: Vec<Event>,
    textures: egui::TexturesDelta,
    last: Option<Frame>,
    pointer: Pos2,
    /// CPU time of the slowest frame so far (our code + egui layout, no tessellation).
    pub worst_frame: Duration,
    /// CPU time of the last frame.
    pub last_frame: Duration,
    /// Text the UI copied to the clipboard in the last frames.
    pub copied: Vec<String>,
    gpu: Option<gpu::Gpu>,
}

impl Shot {
    /// `size` in points, `ppp` pixels per point (2.0 = retina).
    pub fn new(size: [f32; 2], ppp: f32) -> Self {
        Self {
            ctx: egui::Context::default(),
            size: Vec2::new(size[0], size[1]),
            ppp,
            time: 0.0,
            queued: Vec::new(),
            textures: Default::default(),
            last: None,
            pointer: Pos2::new(-10.0, -10.0),
            worst_frame: Duration::ZERO,
            last_frame: Duration::ZERO,
            copied: Vec::new(),
            gpu: None,
        }
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    pub fn resize(&mut self, size: [f32; 2]) {
        self.size = Vec2::new(size[0], size[1]);
    }

    /// Queue a raw event for the next frame.
    pub fn event(&mut self, e: Event) -> &mut Self {
        self.queued.push(e);
        self
    }

    /// Key press + release with modifiers.
    pub fn key(&mut self, key: Key, modifiers: Modifiers) -> &mut Self {
        for pressed in [true, false] {
            self.queued.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        }
        self
    }

    /// Typed text.
    pub fn text(&mut self, s: &str) -> &mut Self {
        self.queued.push(Event::Text(s.to_owned()));
        self
    }

    pub fn paste(&mut self, s: &str) -> &mut Self {
        self.queued.push(Event::Paste(s.to_owned()));
        self
    }

    pub fn move_to(&mut self, pos: [f32; 2]) -> &mut Self {
        self.pointer = Pos2::new(pos[0], pos[1]);
        self.queued.push(Event::PointerMoved(self.pointer));
        self
    }

    /// Press or release the primary button at the current pointer position.
    pub fn button(&mut self, pressed: bool, modifiers: Modifiers) -> &mut Self {
        self.queued.push(Event::PointerButton { pos: self.pointer, button: PointerButton::Primary, pressed, modifiers });
        self
    }

    /// Queue move + press; the release must go in a later frame (`release`), as on real hardware.
    pub fn press_at(&mut self, pos: [f32; 2], modifiers: Modifiers) -> &mut Self {
        self.move_to(pos).button(true, modifiers)
    }

    pub fn release(&mut self) -> &mut Self {
        self.button(false, Modifiers::NONE)
    }

    /// Scroll by `delta` points (positive y = content moves down).
    pub fn scroll(&mut self, delta: [f32; 2]) -> &mut Self {
        self.queued.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(delta[0], delta[1]), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Modifiers::NONE });
        self
    }

    /// Run `n` frames of `ui`. Queued events go into the first one.
    pub fn run(&mut self, n: usize, mut ui: impl FnMut(&mut egui::Ui)) -> &mut Self {
        for _ in 0..n {
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                max_texture_side: Some(8192),
                time: Some(self.time),
                predicted_dt: 1.0 / 60.0,
                events: std::mem::take(&mut self.queued),
                focused: true,
                ..Default::default()
            };
            if let Some(vp) = input.viewports.get_mut(&ViewportId::ROOT) {
                vp.native_pixels_per_point = Some(self.ppp);
                vp.inner_rect = input.screen_rect;
                vp.focused = Some(true);
            }
            let t0 = Instant::now();
            let mut out = self.ctx.run_ui(input, &mut ui);
            self.last_frame = t0.elapsed();
            self.worst_frame = self.worst_frame.max(self.last_frame);
            for cmd in &out.platform_output.commands {
                if let egui::OutputCommand::CopyText(t) = cmd {
                    self.copied.push(t.clone());
                }
            }
            self.textures.append(std::mem::take(&mut out.textures_delta));
            self.last = Some(Frame { shapes: std::mem::take(&mut out.shapes), pixels_per_point: out.pixels_per_point });
            self.time += 1.0 / 60.0;
        }
        self
    }

    /// Render the last frame.
    pub fn image(&mut self) -> Result<Image, ShotError> {
        let f = self.last.as_ref().ok_or(ShotError::NoFrame)?;
        let prims = self.ctx.tessellate(f.shapes.clone(), f.pixels_per_point);
        let px = [(self.size.x * f.pixels_per_point).round() as u32, (self.size.y * f.pixels_per_point).round() as u32];
        if self.gpu.is_none() {
            self.gpu = Some(gpu::Gpu::new()?);
        }
        let clear = self.ctx.global_style().visuals.panel_fill;
        let delta = std::mem::take(&mut self.textures);
        self.gpu.as_mut().ok_or(ShotError::NoFrame)?.render(&prims, delta, px, f.pixels_per_point, clear)
    }

    /// Render the last frame to a PNG file. `path` must be absolute.
    pub fn png(&mut self, path: impl AsRef<Path>) -> Result<(), ShotError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(ShotError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, "absolute path required")));
        }
        let img = self.image()?;
        let bytes = png::encode(img.w, img.h, &img.rgba).map_err(ShotError::Io)?;
        std::fs::write(path, bytes).map_err(ShotError::Io)
    }
}

impl Drop for Shot {
    fn drop(&mut self) {
        // Frames that were never rendered: egui wants the pending texture uploads dropped explicitly.
        self.textures.clear();
    }
}

/// Minimal executor: polls on this thread, parks between wakes.
pub(crate) fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(v) => return v,
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_filled_frame() {
        let mut shot = Shot::new([64.0, 32.0], 1.0);
        shot.run(2, |ui| {
            let r = ui.max_rect();
            ui.painter().rect_filled(r, 0.0, egui::Color32::from_rgb(10, 200, 30));
        });
        match shot.image() {
            Ok(img) => {
                assert_eq!((img.w, img.h), (64, 32));
                assert_eq!(img.pixel(32, 16), [10, 200, 30, 255]);
            }
            Err(ShotError::NoGpu(why)) => eprintln!("skip: {why}"),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn records_input_and_time() {
        let mut shot = Shot::new([100.0, 100.0], 1.0);
        let mut typed = String::new();
        shot.text("hi").run(1, |ui| {
            ui.input(|i| {
                for e in &i.events {
                    if let Event::Text(t) = e {
                        typed.push_str(t);
                    }
                }
            });
        });
        assert_eq!(typed, "hi");
    }
}
