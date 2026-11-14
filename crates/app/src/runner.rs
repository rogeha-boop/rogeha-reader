//! Window + event loop, drawn entirely on the CPU.
//!
//! Why not OpenGL/eframe: on Windows the GPU driver alone adds 60-100 MB to the process
//! (measured: an empty eframe/glow window was 83 MB working set, 112 MB private). MuPDF
//! already renders pages on the CPU, so the UI is rasterised on the CPU too and the finished
//! frame is handed to the OS with `softbuffer`. No graphics driver is loaded into the app.
//!
//! Frames are only drawn when something changes (input, a finished page render, a timer).

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use egui::{ViewportBuilder, ViewportCommand, ViewportEvent, ViewportId};
use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle};
use winit::window::{Window, WindowId};

use crate::app::{PageLayer, ReaderApp};

type AppFactory = Box<dyn FnOnce(&egui::Context) -> ReaderApp>;

enum UserEvent {
    /// Another thread (or egui itself) wants a repaint at this time.
    Repaint(Instant),
}

struct Gfx {
    window: Rc<Window>,
    surface: softbuffer::Surface<OwnedDisplayHandle, Rc<Window>>,
    state: egui_winit::State,
    shown: bool,
}

struct Runner {
    ctx: egui::Context,
    builder: ViewportBuilder,
    sb_context: softbuffer::Context<OwnedDisplayHandle>,
    renderer: EguiSoftwareRender,
    factory: Option<AppFactory>,
    app: Option<ReaderApp>,
    gfx: Option<Gfx>,
    close_requested: bool,
    repaint_at: Option<Instant>,
    error: Option<String>,
}

pub fn run(
    builder: ViewportBuilder,
    factory: impl FnOnce(&egui::Context) -> ReaderApp + 'static,
) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    let ctx = egui::Context::default();
    let proxy = event_loop.create_proxy();
    ctx.set_request_repaint_callback(move |info| {
        let _ = proxy.send_event(UserEvent::Repaint(Instant::now() + info.delay));
    });
    let sb_context = softbuffer::Context::new(event_loop.owned_display_handle())
        .map_err(|e| e.to_string())?;

    let mut runner = Runner {
        ctx,
        // Created hidden and shown after the first frame is ready, so there is no white flash.
        builder: builder.with_visible(false),
        sb_context,
        // softbuffer wants 0RGB u32 pixels, which is B, G, R, X in memory.
        renderer: EguiSoftwareRender::new(ColorFieldOrder::Bgra),
        factory: Some(Box::new(factory)),
        app: None,
        gfx: None,
        close_requested: false,
        repaint_at: None,
        error: None,
    };
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl Runner {
    fn fail(&mut self, el: &ActiveEventLoop, message: String) {
        self.error = Some(message);
        el.exit();
    }

    fn paint(&mut self, el: &ActiveEventLoop) {
        let (Some(gfx), Some(app)) = (&mut self.gfx, &mut self.app) else {
            return;
        };
        let size = gfx.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return; // minimised
        };

        let mut raw = gfx.state.take_egui_input(&gfx.window);
        let info = raw.viewports.entry(ViewportId::ROOT).or_default();
        egui_winit::update_viewport_info(info, &self.ctx, &gfx.window, false);
        if self.close_requested {
            info.events.push(ViewportEvent::Close);
        }

        let out = self.ctx.run_ui(raw, |ui| app.ui(ui));
        gfx.state.handle_platform_output(&gfx.window, out.platform_output);

        // The few viewport commands the reader uses.
        let mut cancel_close = false;
        let mut screenshots = Vec::new();
        for (_, viewport) in out.viewport_output {
            for command in viewport.commands {
                match command {
                    ViewportCommand::Title(title) => gfx.window.set_title(&title),
                    ViewportCommand::Close => {
                        self.close_requested = true;
                        gfx.window.request_redraw();
                    }
                    ViewportCommand::CancelClose => cancel_close = true,
                    ViewportCommand::Fullscreen(on) => {
                        // Borderless full screen on the monitor the window is on.
                        gfx.window.set_fullscreen(on.then_some(winit::window::Fullscreen::Borderless(None)));
                        gfx.window.request_redraw();
                    }
                    ViewportCommand::Screenshot(callback) => screenshots.push(callback),
                    _ => {}
                }
            }
        }
        if self.close_requested {
            if cancel_close {
                self.close_requested = false;
            } else if !self.ctx.input(|i| i.viewport().close_requested()) {
                // The app has not seen the close request yet; it gets it next frame.
            } else {
                el.exit();
                return;
            }
        }

        let primitives = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        if let Err(e) = gfx.surface.resize(w, h) {
            return self.fail(el, format!("Could not resize the window surface: {e}"));
        }
        let mut buffer = match gfx.surface.buffer_mut() {
            Ok(b) => b,
            Err(e) => return self.fail(el, format!("Could not get the window surface: {e}")),
        };
        // Pages first, straight into the frame buffer; the UI is then blended on top.
        buffer.fill(0);
        draw_page_layer(
            &mut buffer,
            size.width as usize,
            size.height as usize,
            app.page_layer(),
            out.pixels_per_point,
        );
        {
            let pixels: &mut [u32] = &mut buffer;
            // SAFETY: [u32] and [[u8; 4]] have the same size and [u8; 4] has a weaker alignment.
            let bytes: &mut [[u8; 4]] = unsafe {
                std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast(), pixels.len())
            };
            let mut target = BufferMutRef::new(bytes, size.width as usize, size.height as usize);
            self.renderer
                .render(&mut target, &primitives, &out.textures_delta, out.pixels_per_point);

            if !screenshots.is_empty() {
                let image = egui::ColorImage::new(
                    [size.width as usize, size.height as usize],
                    target
                        .data
                        .iter()
                        .map(|p| egui::Color32::from_rgb(p[2], p[1], p[0]))
                        .collect(),
                );
                let image = Arc::new(image);
                for callback in screenshots {
                    callback.complete(image.clone());
                }
            }
        }
        if let Err(e) = buffer.present() {
            return self.fail(el, format!("Could not show the frame: {e}"));
        }
        if !gfx.shown {
            gfx.shown = true;
            gfx.window.set_visible(true);
        }
    }
}

impl ApplicationHandler<UserEvent> for Runner {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let window = match egui_winit::create_window(&self.ctx, el, &self.builder) {
            Ok(w) => Rc::new(w),
            Err(e) => return self.fail(el, format!("Could not create the window: {e}")),
        };
        let surface = match softbuffer::Surface::new(&self.sb_context, window.clone()) {
            Ok(s) => s,
            Err(e) => return self.fail(el, format!("Could not create the window surface: {e}")),
        };
        let state = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        if let Some(factory) = self.factory.take() {
            self.app = Some(factory(&self.ctx));
        }
        window.request_redraw();
        self.gfx = Some(Gfx { window, surface, state, shown: false });
        // A hidden window gets no redraw events on some platforms; draw the first frame now.
        self.paint(el);
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Repaint(when) = event;
        if when <= Instant::now() {
            if let Some(gfx) = &self.gfx {
                gfx.window.request_redraw();
            }
        } else {
            self.repaint_at = Some(self.repaint_at.map_or(when, |t| t.min(when)));
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gfx) = &mut self.gfx else { return };
        match &event {
            WindowEvent::RedrawRequested => {
                self.paint(el);
                return;
            }
            WindowEvent::CloseRequested => self.close_requested = true,
            _ => {}
        }
        let response = gfx.state.on_window_event(&gfx.window, &event);
        if response.repaint || matches!(event, WindowEvent::CloseRequested | WindowEvent::Resized(_)) {
            gfx.window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        match self.repaint_at {
            Some(when) if when <= Instant::now() => {
                self.repaint_at = None;
                if let Some(gfx) = &self.gfx {
                    gfx.window.request_redraw();
                }
                el.set_control_flow(ControlFlow::Wait);
            }
            Some(when) => el.set_control_flow(ControlFlow::WaitUntil(when)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }
}

/// Paint the document background and pages into a `0x00RRGGBB` frame buffer.
fn draw_page_layer(buf: &mut [u32], width: usize, height: usize, layer: &PageLayer, ppp: f32) {
    const WHITE: u32 = 0x00FF_FFFF;
    let px = |v: f32| (v * ppp).round() as i64;
    let (cx0, cx1) = (
        px(layer.clip.min.x).clamp(0, width as i64),
        px(layer.clip.max.x).clamp(0, width as i64),
    );
    let (cy0, cy1) = (
        px(layer.clip.min.y).clamp(0, height as i64),
        px(layer.clip.max.y).clamp(0, height as i64),
    );
    if cx0 >= cx1 || cy0 >= cy1 {
        return;
    }
    let bg = layer.background;
    let bg = (bg.r() as u32) << 16 | (bg.g() as u32) << 8 | bg.b() as u32;
    for y in cy0 as usize..cy1 as usize {
        buf[y * width + cx0 as usize..y * width + cx1 as usize].fill(bg);
    }

    for (rect, image) in &layer.pages {
        // Destination rectangle in pixels (may extend outside the clip area).
        let (dx0, dy0) = (px(rect.min.x), px(rect.min.y));
        let (dx1, dy1) = (px(rect.max.x), px(rect.max.y));
        let (dw, dh) = ((dx1 - dx0).max(1) as usize, (dy1 - dy0).max(1) as usize);
        // Visible part.
        let (x0, x1) = (dx0.clamp(cx0, cx1) as usize, dx1.clamp(cx0, cx1) as usize);
        let (y0, y1) = (dy0.clamp(cy0, cy1) as usize, dy1.clamp(cy0, cy1) as usize);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let Some(img) = image.as_deref().filter(|i| i.width > 0 && i.height > 0) else {
            for y in y0..y1 {
                buf[y * width + x0..y * width + x1].fill(WHITE);
            }
            continue;
        };

        if img.width.abs_diff(dw) <= 2 && img.height.abs_diff(dh) <= 2 {
            // 1:1: copy rows. Pad with white if the image is a pixel short.
            let sx = (x0 as i64 - dx0) as usize;
            for y in y0..y1 {
                let sy = (y as i64 - dy0) as usize;
                let out = &mut buf[y * width + x0..y * width + x1];
                if sy >= img.height || sx >= img.width {
                    out.fill(WHITE);
                    continue;
                }
                let n = out.len().min(img.width - sx);
                out[..n].copy_from_slice(&img.pixels[sy * img.width + sx..][..n]);
                out[n..].fill(WHITE);
            }
        } else {
            // Different zoom than the image was rendered at (while zooming, or beyond the
            // render size limit): bilinear scaling in 16.16 fixed point.
            let step_x = ((img.width as u64) << 16) / dw as u64;
            let step_y = ((img.height as u64) << 16) / dh as u64;
            let max_fx = ((img.width - 1) as u64) << 16;
            let max_fy = ((img.height - 1) as u64) << 16;
            for y in y0..y1 {
                let fy = ((y as i64 - dy0) as u64 * step_y).min(max_fy);
                let (sy, wy) = ((fy >> 16) as usize, (fy >> 8 & 255) as u32);
                let row0 = &img.pixels[sy * img.width..][..img.width];
                let row1 = &img.pixels[(sy + 1).min(img.height - 1) * img.width..][..img.width];
                let out = &mut buf[y * width + x0..y * width + x1];
                for (i, o) in out.iter_mut().enumerate() {
                    let fx = (((x0 + i) as i64 - dx0) as u64 * step_x).min(max_fx);
                    let (sx, wx) = ((fx >> 16) as usize, (fx >> 8 & 255) as u32);
                    let sx1 = (sx + 1).min(img.width - 1);
                    *o = lerp_px(
                        lerp_px(row0[sx], row0[sx1], wx),
                        lerp_px(row1[sx], row1[sx1], wx),
                        wy,
                    );
                }
            }
        }
    }
}

/// Blend two 0x00RRGGBB pixels; `t` is 0..=255.
#[inline]
fn lerp_px(a: u32, b: u32, t: u32) -> u32 {
    // Red and blue are blended together in one u64 so the multiply cannot overflow.
    let (s, t) = (256 - t as u64, t as u64);
    let (a, b) = (a as u64, b as u64);
    let rb = (((a & 0x00FF_00FF) * s + (b & 0x00FF_00FF) * t) >> 8) & 0x00FF_00FF;
    let g = (((a & 0x0000_FF00) * s + (b & 0x0000_FF00) * t) >> 8) & 0x0000_FF00;
    (rb | g) as u32
}

#[cfg(test)]
mod tests {
    use super::lerp_px;

    #[test]
    fn blending_keeps_white_white_and_hits_the_endpoints() {
        for t in [0, 1, 128, 255] {
            assert_eq!(lerp_px(0x00FF_FFFF, 0x00FF_FFFF, t), 0x00FF_FFFF);
            assert_eq!(lerp_px(0, 0, t), 0);
        }
        assert_eq!(lerp_px(0x0012_3456, 0x00FF_FFFF, 0), 0x0012_3456);
        assert_eq!(lerp_px(0x0000_0000, 0x00FF_FFFF, 128), 0x007F_7F7F);
    }
}
