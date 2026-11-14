//! Visual signatures: the "Sign" window (draw, type or load a picture, reuse a saved one)
//! and placing the signature on a page (click, move, resize, apply).
//!
//! The preview on the page is drawn by the same engine code that stamps it, so what you
//! place is what you get.

use std::path::PathBuf;

use egui::{Color32, CursorIcon, Pos2, Rect, Response, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use rogeha_core::{INK_BLACK, INK_BLUE, Request, SignatureArt, load_signature_image, render_signature};

use super::ReaderApp;
use crate::theme;

const PAD_SIZE: Vec2 = Vec2::new(440.0, 150.0);
const MAX_SAVED: usize = 6;
/// Width of a freshly placed signature, in page points (about 6 cm).
const DEFAULT_WIDTH: f32 = 170.0;
const MIN_WIDTH: f32 = 30.0;
const HANDLE: f32 = 12.0;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Draw,
    Type,
    Picture,
    Certificate,
}

/// A signature ready to use: what it looks like, its shape and its ink.
#[derive(Clone)]
struct Ready {
    art: SignatureArt,
    aspect: f32,
    ink: [u8; 3],
    preview: Option<TextureHandle>,
}

enum Drag {
    Move(Vec2),
    Resize,
}

pub(super) struct Placement {
    sig: Ready,
    /// Page and box (page points, as displayed) once the user has clicked.
    at: Option<(usize, Rect)>,
    drag: Option<Drag>,
}

pub(super) struct SignUi {
    pub open: bool,
    tab: Tab,
    /// Pen strokes in pad coordinates (0..PAD_SIZE).
    strokes: Vec<Vec<Pos2>>,
    typed: String,
    picture: Option<Ready>,
    picture_source: Option<(Vec<u8>, String)>,
    picture_error: Option<String>,
    remove_paper: bool,
    blue: bool,
    remember: bool,
    saved: Vec<Ready>,
    saved_loaded: bool,
    pub placing: Option<Placement>,
}

impl Default for SignUi {
    fn default() -> Self {
        Self {
            open: false,
            tab: Tab::Draw,
            strokes: Vec::new(),
            typed: String::new(),
            picture: None,
            picture_source: None,
            picture_error: None,
            remove_paper: true,
            blue: true,
            remember: true,
            saved: Vec::new(),
            saved_loaded: false,
            placing: None,
        }
    }
}

fn texture(ctx: &egui::Context, sig: &Ready, width_px: usize) -> Option<TextureHandle> {
    let (rgba, w, h) = render_signature(&sig.art, sig.aspect, sig.ink, width_px).ok()?;
    let image = egui::ColorImage::from_rgba_premultiplied([w, h], &rgba);
    Some(ctx.load_texture("signature", image, TextureOptions::LINEAR))
}

// ------------------------------------------------------------------ saved signatures

fn store_file() -> PathBuf {
    super::app_data_dir().join("signatures.json")
}

fn load_saved() -> Vec<Ready> {
    let Ok(text) = std::fs::read_to_string(store_file()) else { return Vec::new() };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str(&text) else { return Vec::new() };
    let mut out = Vec::new();
    for item in items {
        let ink = match item["ink"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64()).collect::<Vec<_>>()) {
            Some(v) if v.len() == 3 => [v[0] as u8, v[1] as u8, v[2] as u8],
            _ => INK_BLUE,
        };
        let aspect = item["aspect"].as_f64().unwrap_or(3.0) as f32;
        let art = match item["kind"].as_str() {
            Some("strokes") => {
                let strokes = item["strokes"]
                    .as_array()
                    .map(|strokes| {
                        strokes
                            .iter()
                            .map(|s| {
                                s.as_array()
                                    .map(|pts| {
                                        pts.iter()
                                            .filter_map(|p| Some((p[0].as_f64()? as f32, p[1].as_f64()? as f32)))
                                            .collect()
                                    })
                                    .unwrap_or_default()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                SignatureArt::Strokes(strokes)
            }
            Some("text") => SignatureArt::Text(item["text"].as_str().unwrap_or_default().to_owned()),
            Some("image") => {
                let (Some(w), Some(h), Some(file)) = (item["width"].as_u64(), item["height"].as_u64(), item["file"].as_str()) else {
                    continue;
                };
                let Ok(rgba) = std::fs::read(super::app_data_dir().join(file)) else { continue };
                if rgba.len() as u64 != w * h * 4 {
                    continue;
                }
                SignatureArt::Image { rgba, width: w as usize, height: h as usize }
            }
            _ => continue,
        };
        if !art.is_empty() {
            out.push(Ready { art, aspect, ink, preview: None });
        }
    }
    out
}

fn store_saved(saved: &[Ready]) -> Result<(), String> {
    let dir = super::app_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for (i, sig) in saved.iter().enumerate() {
        let mut item = serde_json::json!({ "aspect": sig.aspect, "ink": sig.ink });
        match &sig.art {
            SignatureArt::Strokes(strokes) => {
                item["kind"] = "strokes".into();
                item["strokes"] = serde_json::json!(
                    strokes.iter().map(|s| s.iter().map(|p| [p.0, p.1]).collect::<Vec<_>>()).collect::<Vec<_>>()
                );
            }
            SignatureArt::Text(text) => {
                item["kind"] = "text".into();
                item["text"] = text.clone().into();
            }
            SignatureArt::Image { rgba, width, height } => {
                let file = format!("signature-{i}.rgba");
                std::fs::write(dir.join(&file), rgba).map_err(|e| e.to_string())?;
                item["kind"] = "image".into();
                item["width"] = (*width).into();
                item["height"] = (*height).into();
                item["file"] = file.into();
            }
        }
        items.push(item);
    }
    std::fs::write(store_file(), serde_json::Value::Array(items).to_string()).map_err(|e| e.to_string())
}

impl ReaderApp {
    // ---------------------------------------------------------------- the Sign window

    /// The signature currently set up in the window, if there is one.
    fn signature_from_window(&self) -> Option<Ready> {
        let ink = if self.sign.blue { INK_BLUE } else { INK_BLACK };
        match self.sign.tab {
            Tab::Draw => {
                let points = self.sign.strokes.iter().flatten();
                let (mut min, mut max) = (Pos2::new(f32::MAX, f32::MAX), Pos2::new(f32::MIN, f32::MIN));
                for p in points {
                    min = min.min(*p);
                    max = max.max(*p);
                }
                if self.sign.strokes.iter().all(|s| s.len() < 2) {
                    return None;
                }
                // Normalise to the ink's own box (a little margin for the pen's width).
                let size = (max - min).max(Vec2::new(24.0, 12.0));
                let (min, size) = (min - size * 0.03, size * 1.06);
                let strokes = self
                    .sign
                    .strokes
                    .iter()
                    .filter(|s| s.len() >= 2)
                    .map(|s| s.iter().map(|p| ((p.x - min.x) / size.x, (p.y - min.y) / size.y)).collect())
                    .collect();
                Some(Ready { art: SignatureArt::Strokes(strokes), aspect: (size.x / size.y).clamp(0.5, 12.0), ink, preview: None })
            }
            Tab::Type => {
                let art = SignatureArt::Text(self.sign.typed.trim().to_owned());
                (!art.is_empty()).then(|| Ready { aspect: art.aspect(), art, ink, preview: None })
            }
            Tab::Picture => self.sign.picture.clone(),
            Tab::Certificate => None,
        }
    }

    fn load_picture(&mut self) {
        let Some((bytes, extension)) = &self.sign.picture_source else { return };
        match load_signature_image(bytes, extension, self.sign.remove_paper) {
            Ok(art) => {
                self.sign.picture = Some(Ready { aspect: art.aspect(), art, ink: INK_BLACK, preview: None });
                self.sign.picture_error = None;
            }
            Err(e) => {
                self.sign.picture = None;
                self.sign.picture_error = Some(e);
            }
        }
    }

    fn start_placing(&mut self, sig: Ready) {
        self.sign.open = false;
        self.comments.selection = None;
        self.comments.note_tool = false;
        self.sign.placing = Some(Placement { sig, at: None, drag: None });
        self.info("Click on the page where the signature should go");
    }

    pub(super) fn sign_window(&mut self, ctx: &egui::Context) {
        if !self.sign.open {
            return;
        }
        if !std::mem::replace(&mut self.sign.saved_loaded, true) {
            self.sign.saved = load_saved();
        }
        let t = theme::tokens(ctx.theme() == egui::Theme::Dark);
        let mut open = true;
        let mut place: Option<Ready> = None;
        let mut forget: Option<usize> = None;
        let mut choose_picture = false;
        let mut reload_picture = false;
        egui::Window::new("Sign")
            .id(egui::Id::unique("sign-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(self.layer.clip.min + Vec2::new(40.0, 30.0))
            .show(ctx, |ui| {
                ui.set_width(PAD_SIZE.x);
                // Saved signatures.
                if !self.sign.saved.is_empty() {
                    ui.label("Your signatures:");
                    ui.horizontal_wrapped(|ui| {
                        for i in 0..self.sign.saved.len() {
                            if self.sign.saved[i].preview.is_none() {
                                self.sign.saved[i].preview = texture(ui.ctx(), &self.sign.saved[i], 360);
                            }
                            let sig = &self.sign.saved[i];
                            let size = Vec2::new((40.0 * sig.aspect).clamp(50.0, 150.0), 40.0);
                            ui.vertical(|ui| {
                                let (rect, resp) = ui.allocate_exact_size(size + Vec2::splat(8.0), Sense::click());
                                // Signatures are shown on white, like paper, in both themes.
                                ui.painter().rect_filled(rect, theme::RADIUS_SM, Color32::WHITE);
                                ui.painter().rect_stroke(
                                    rect,
                                    theme::RADIUS_SM,
                                    Stroke::new(1.0, if resp.hovered() { t.steel_500 } else { t.border }),
                                    StrokeKind::Inside,
                                );
                                if let Some(tex) = &sig.preview {
                                    let fit = fit_inside(rect.shrink(4.0), sig.aspect);
                                    ui.painter().image(tex.id(), fit, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                                }
                                if resp.on_hover_text("Use this signature").on_hover_cursor(CursorIcon::PointingHand).clicked() {
                                    place = Some(sig.clone());
                                }
                                if ui.small_button("Remove").clicked() {
                                    forget = Some(i);
                                }
                            });
                        }
                    });
                    ui.separator();
                    ui.label("Or make a new one:");
                }

                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.sign.tab, Tab::Draw, "Draw");
                    ui.selectable_value(&mut self.sign.tab, Tab::Type, "Type");
                    ui.selectable_value(&mut self.sign.tab, Tab::Picture, "Picture");
                    ui.separator();
                    ui.selectable_value(&mut self.sign.tab, Tab::Certificate, "Certificate (digital)");
                });
                ui.add_space(4.0);
                if self.sign.tab == Tab::Certificate {
                    self.certificate_tab(ui);
                    return;
                }
                let ink = if self.sign.blue { INK_BLUE } else { INK_BLACK };
                let ink_color = Color32::from_rgb(ink[0], ink[1], ink[2]);

                match self.sign.tab {
                    Tab::Draw => {
                        let (pad, resp) = ui.allocate_exact_size(PAD_SIZE, Sense::drag());
                        let painter = ui.painter_at(pad);
                        painter.rect_filled(pad, theme::RADIUS_SM, Color32::WHITE);
                        painter.rect_stroke(pad, theme::RADIUS_SM, Stroke::new(1.0, t.border), StrokeKind::Inside);
                        // A guide line to sign on.
                        let y = pad.top() + PAD_SIZE.y * 0.72;
                        painter.line_segment(
                            [Pos2::new(pad.left() + 20.0, y), Pos2::new(pad.right() - 20.0, y)],
                            Stroke::new(1.0, Color32::from_gray(200)),
                        );
                        if resp.drag_started() {
                            self.sign.strokes.push(Vec::new());
                        }
                        if resp.dragged() {
                            if let (Some(pos), Some(stroke)) = (resp.interact_pointer_pos(), self.sign.strokes.last_mut()) {
                                let p = (pos - pad.min).to_pos2().clamp(Pos2::ZERO, PAD_SIZE.to_pos2());
                                if stroke.last().is_none_or(|last| last.distance(p) > 1.0) {
                                    stroke.push(p);
                                }
                            }
                        }
                        for stroke in &self.sign.strokes {
                            let points: Vec<Pos2> = stroke.iter().map(|p| pad.min + p.to_vec2()).collect();
                            if points.len() >= 2 {
                                painter.add(egui::Shape::line(points, Stroke::new(2.2, ink_color)));
                            }
                        }
                        if self.sign.strokes.is_empty() {
                            painter.text(pad.center(), egui::Align2::CENTER_CENTER, "Sign here with the mouse or a pen", egui::FontId::proportional(14.0), Color32::from_gray(150));
                        }
                        resp.on_hover_cursor(CursorIcon::Crosshair);
                        if ui.button("Clear").clicked() {
                            self.sign.strokes.clear();
                        }
                    }
                    Tab::Type => {
                        ui.add(egui::TextEdit::singleline(&mut self.sign.typed).desired_width(PAD_SIZE.x).hint_text("Your name"));
                        let (pad, _) = ui.allocate_exact_size(Vec2::new(PAD_SIZE.x, 90.0), Sense::hover());
                        ui.painter().rect_filled(pad, theme::RADIUS_SM, Color32::WHITE);
                        ui.painter().rect_stroke(pad, theme::RADIUS_SM, Stroke::new(1.0, t.border), StrokeKind::Inside);
                        // Shown exactly as it will be stamped (Times Italic).
                        if let Some(sig) = self.signature_from_window() {
                            if let Some(tex) = texture(ui.ctx(), &sig, 800) {
                                let fit = fit_inside(pad.shrink(10.0), sig.aspect);
                                ui.painter().image(tex.id(), fit, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                            }
                        }
                    }
                    Tab::Certificate => {}
                    Tab::Picture => {
                        ui.horizontal(|ui| {
                            if ui.button("Choose a picture…").clicked() {
                                choose_picture = true;
                            }
                            if ui.checkbox(&mut self.sign.remove_paper, "Remove the paper around the ink").changed() {
                                reload_picture = true;
                            }
                        });
                        let (pad, _) = ui.allocate_exact_size(Vec2::new(PAD_SIZE.x, 120.0), Sense::hover());
                        ui.painter().rect_filled(pad, theme::RADIUS_SM, Color32::WHITE);
                        ui.painter().rect_stroke(pad, theme::RADIUS_SM, Stroke::new(1.0, t.border), StrokeKind::Inside);
                        if let Some(sig) = &mut self.sign.picture {
                            if sig.preview.is_none() {
                                sig.preview = texture(ui.ctx(), sig, 800);
                            }
                            if let Some(tex) = &sig.preview {
                                let fit = fit_inside(pad.shrink(10.0), sig.aspect);
                                ui.painter().image(tex.id(), fit, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                            }
                        } else {
                            ui.painter().text(pad.center(), egui::Align2::CENTER_CENTER, "A photo or scan of your signature on white paper", egui::FontId::proportional(14.0), Color32::from_gray(150));
                        }
                        if let Some(e) = &self.sign.picture_error {
                            ui.colored_label(ui.visuals().error_fg_color, e);
                        }
                    }
                }

                ui.add_space(4.0);
                if self.sign.tab != Tab::Picture {
                    ui.horizontal(|ui| {
                        ui.label("Ink:");
                        ui.radio_value(&mut self.sign.blue, true, "Blue");
                        ui.radio_value(&mut self.sign.blue, false, "Black");
                    });
                }
                ui.checkbox(&mut self.sign.remember, "Remember this signature on this computer")
                    .on_hover_text("Stored in your user profile, not encrypted. Anyone using your Windows account can use it.");
                ui.add_space(4.0);
                let current = self.signature_from_window();
                ui.horizontal(|ui| {
                    let button = egui::Button::new(egui::RichText::new("Place on the page").color(t.on_steel)).fill(t.steel_500);
                    if ui.add_enabled(current.is_some(), button).clicked() {
                        place = current.clone();
                        if self.sign.remember {
                            if let Some(sig) = current {
                                self.sign.saved.retain(|s| s.art != sig.art);
                                self.sign.saved.insert(0, sig);
                                self.sign.saved.truncate(MAX_SAVED);
                                if let Err(e) = store_saved(&self.sign.saved) {
                                    self.sign.picture_error = Some(format!("Could not remember the signature: {e}"));
                                }
                            }
                        }
                    }
                    ui.label(egui::RichText::new("This adds a picture of your signature. It is not a digital certificate signature.").small().weak());
                });
            });

        if choose_picture {
            let picked = rfd::FileDialog::new()
                .set_title("Choose a picture of your signature")
                .add_filter("Pictures", &["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp"])
                .pick_file();
            if let Some(path) = picked {
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "png".into());
                        self.sign.picture_source = Some((bytes, ext));
                        reload_picture = true;
                    }
                    Err(e) => self.sign.picture_error = Some(format!("Could not read the picture: {e}")),
                }
            }
        }
        if reload_picture {
            self.load_picture();
        }
        if let Some(i) = forget {
            self.sign.saved.remove(i);
            let _ = store_saved(&self.sign.saved);
        }
        if let Some(sig) = place {
            self.start_placing(sig);
        } else if !open {
            self.sign.open = false;
        }
    }

    /// The most recently saved signature (art, shape, ink), for the assistant.
    pub(super) fn first_saved_signature(&mut self) -> Option<(SignatureArt, f32, [u8; 3])> {
        if !std::mem::replace(&mut self.sign.saved_loaded, true) {
            self.sign.saved = load_saved();
        }
        self.sign.saved.first().map(|s| (s.art.clone(), s.aspect, s.ink))
    }

    // --------------------------------------------------------------- placing on a page

    /// Mouse handling while a signature is being placed. Replaces text selection.
    pub(super) fn placement_interactions(&mut self, ctx: &egui::Context, pages: &[(usize, Rect, Response)], scale: f32) {
        let Some(doc) = &self.doc else { return };
        let sizes = doc.page_sizes.clone();
        let Some(placing) = &mut self.sign.placing else { return };

        for (page, page_rect, response) in pages {
            let page = *page;
            let (pw, ph) = sizes[page];
            let bounds = Rect::from_min_size(Pos2::ZERO, Vec2::new(pw, ph));
            let to_page = |pos: Pos2| ((pos - page_rect.min) / scale).to_pos2();
            let pointer = response.interact_pointer_pos().or(response.hover_pos());
            let here = placing.at.filter(|(p, _)| *p == page).map(|(_, r)| r);

            // Cursor feedback.
            if let (true, Some(pos)) = (response.hovered(), pointer) {
                let p = to_page(pos);
                ctx.set_cursor_icon(match here {
                    Some(r) if handle_rect(r, scale).contains(p) => CursorIcon::ResizeNwSe,
                    Some(r) if r.contains(p) => CursorIcon::Move,
                    _ => CursorIcon::Crosshair,
                });
            }

            let new_box = |centre: Pos2, aspect: f32| {
                let w = DEFAULT_WIDTH.min(pw * 0.6);
                keep_inside(Rect::from_center_size(centre, Vec2::new(w, w / aspect)), bounds)
            };

            if response.drag_started() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let p = to_page(pos);
                    placing.drag = match here {
                        Some(r) if handle_rect(r, scale).contains(p) => Some(Drag::Resize),
                        Some(r) if r.contains(p) => Some(Drag::Move(p - r.min)),
                        _ => {
                            // Start somewhere new and keep dragging it from its middle.
                            let r = new_box(p, placing.sig.aspect);
                            placing.at = Some((page, r));
                            Some(Drag::Move(p - r.min))
                        }
                    };
                }
            }
            if response.dragged() {
                if let (Some(pos), Some((at_page, r))) = (response.interact_pointer_pos(), placing.at) {
                    if at_page == page {
                        let p = to_page(pos);
                        let moved = match &placing.drag {
                            Some(Drag::Move(grab)) => Some(keep_inside(Rect::from_min_size(p - *grab, r.size()), bounds)),
                            Some(Drag::Resize) => {
                                let max_w = (pw - r.min.x).min((ph - r.min.y) * placing.sig.aspect);
                                let w = (p.x - r.min.x).clamp(MIN_WIDTH.min(max_w), max_w);
                                Some(Rect::from_min_size(r.min, Vec2::new(w, w / placing.sig.aspect)))
                            }
                            None => None,
                        };
                        if let Some(moved) = moved {
                            placing.at = Some((page, moved));
                        }
                    }
                }
            }
            if response.drag_stopped() {
                placing.drag = None;
            }
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let p = to_page(pos);
                    if !here.is_some_and(|r| r.contains(p)) {
                        placing.at = Some((page, new_box(p, placing.sig.aspect)));
                    }
                }
            }
        }
    }

    /// The signature preview on the page, its frame and handle, and the Apply / Cancel bar.
    pub(super) fn placement_overlay(&mut self, ui: &egui::Ui, pages: &[(usize, Rect, Response)], scale: f32) {
        let clip = self.layer.clip;
        let t = theme::tokens(ui.visuals().dark_mode);
        let Some(placing) = &mut self.sign.placing else { return };
        if placing.sig.preview.is_none() {
            placing.sig.preview = texture(ui.ctx(), &placing.sig, 900);
        }
        let mut apply = ui.input(|i| i.key_pressed(egui::Key::Enter)) && placing.at.is_some();
        let mut cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));

        if let Some((page, r)) = placing.at {
            if let Some((_, page_rect, _)) = pages.iter().find(|(p, ..)| *p == page) {
                let screen = Rect::from_min_max(page_rect.min + r.min.to_vec2() * scale, page_rect.min + r.max.to_vec2() * scale);
                let painter = ui.painter().with_clip_rect(clip);
                if let Some(tex) = &placing.sig.preview {
                    painter.image(tex.id(), screen, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                painter.rect_stroke(screen, 0, Stroke::new(1.0, t.steel_500), StrokeKind::Outside);
                let handle = Rect::from_center_size(screen.max, Vec2::splat(HANDLE));
                painter.rect_filled(handle, theme::RADIUS_SM - 2, t.steel_500);

                let pos = Pos2::new(screen.center().x, screen.max.y + 12.0);
                egui::Area::new(egui::Id::unique("signature-bar"))
                    .order(egui::Order::Foreground)
                    .pivot(egui::Align2::CENTER_TOP)
                    .fixed_pos(pos.clamp(clip.min + Vec2::new(120.0, 0.0), clip.max - Vec2::new(120.0, 44.0)))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let button = egui::Button::new(egui::RichText::new("Apply signature").color(t.on_steel)).fill(t.steel_500);
                                if ui.add(button).on_hover_text("Stamp it into the page (Enter)").clicked() {
                                    apply = true;
                                }
                                if ui.button("Cancel").clicked() {
                                    cancel = true;
                                }
                            });
                        });
                    });
            }
        }

        if apply {
            self.apply_signature();
        } else if cancel {
            self.sign.placing = None;
        }
    }

    pub(super) fn apply_signature(&mut self) {
        let Some(placing) = self.sign.placing.take() else { return };
        let Some((page, r)) = placing.at else { return };
        self.worker.send(Request::StampSignature {
            page,
            rect: (r.min.x, r.min.y, r.max.x, r.max.y),
            art: placing.sig.art,
            ink: placing.sig.ink,
        });
        self.info("Signature added. Save the document to keep it.");
    }

    // ------------------------------------------------------------- scripted checks

    pub(super) fn dev_certificate_tab(&mut self) {
        self.sign.open = true;
        self.sign.tab = Tab::Certificate;
    }

    /// `kind` is "draw" or a name to type; places it at (x, y) on the current page.
    pub(super) fn dev_sign(&mut self, kind: &str, at: Option<(f32, f32)>) {
        let sig = if kind == "draw" {
            let strokes = vec![
                (0..=40).map(|i| { let t = i as f32 / 40.0; (0.02 + t * 0.96, 0.5 + 0.42 * (t * 14.0).sin() * (1.0 - t * 0.5)) }).collect(),
                vec![(0.05, 0.92), (0.95, 0.86)],
            ];
            Ready { art: SignatureArt::Strokes(strokes), aspect: 3.2, ink: INK_BLUE, preview: None }
        } else {
            let art = SignatureArt::Text(kind.to_owned());
            Ready { aspect: art.aspect(), art, ink: INK_BLACK, preview: None }
        };
        self.start_placing(sig);
        if let (Some((x, y)), Some(placing)) = (at, &mut self.sign.placing) {
            let w = DEFAULT_WIDTH;
            placing.at = Some((self.current_page, Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, w / placing.sig.aspect))));
        }
    }
}

/// The largest box of the given shape centred inside `outer`.
fn fit_inside(outer: Rect, aspect: f32) -> Rect {
    let size = if outer.width() / outer.height() > aspect {
        Vec2::new(outer.height() * aspect, outer.height())
    } else {
        Vec2::new(outer.width(), outer.width() / aspect)
    };
    Rect::from_center_size(outer.center(), size)
}

/// Move `r` so that it lies inside `bounds` (page points).
fn keep_inside(r: Rect, bounds: Rect) -> Rect {
    let size = r.size().min(bounds.size());
    let min = r.min.clamp(bounds.min, bounds.max - size);
    Rect::from_min_size(min, size)
}

/// The resize handle at the box's bottom-right corner, in page points.
fn handle_rect(r: Rect, scale: f32) -> Rect {
    Rect::from_center_size(r.max, Vec2::splat((HANDLE + 6.0) / scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_signatures_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("rogeha-reader-sign-test-{}", std::process::id()));
        // SAFETY: this is the only test in this binary that touches the environment.
        unsafe { std::env::set_var("ROGEHA_DATA_DIR", &dir) };
        assert!(load_saved().is_empty());

        let drawn = Ready {
            art: SignatureArt::Strokes(vec![vec![(0.0, 0.5), (0.5, 0.1), (1.0, 0.9)]]),
            aspect: 2.5,
            ink: INK_BLUE,
            preview: None,
        };
        let typed = Ready { art: SignatureArt::Text("Ada Lovelace".into()), aspect: 4.0, ink: INK_BLACK, preview: None };
        let picture = Ready {
            art: SignatureArt::Image { rgba: vec![10, 20, 30, 255, 0, 0, 0, 0], width: 2, height: 1 },
            aspect: 2.0,
            ink: INK_BLACK,
            preview: None,
        };
        store_saved(&[drawn.clone(), typed.clone(), picture.clone()]).unwrap();

        let loaded = load_saved();
        assert_eq!(loaded.len(), 3);
        for (a, b) in loaded.iter().zip([&drawn, &typed, &picture]) {
            assert_eq!(a.art, b.art);
            assert_eq!((a.aspect, a.ink), (b.aspect, b.ink));
        }
        // Forgetting them all leaves nothing behind to load.
        store_saved(&[]).unwrap();
        assert!(load_saved().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn boxes_stay_on_the_page_and_keep_their_shape() {
        let page = Rect::from_min_size(Pos2::ZERO, Vec2::new(595.0, 842.0));
        let r = keep_inside(Rect::from_min_size(Pos2::new(550.0, 830.0), Vec2::new(170.0, 50.0)), page);
        assert_eq!(r.max, Pos2::new(595.0, 842.0));
        assert_eq!(r.size(), Vec2::new(170.0, 50.0));
        let fit = fit_inside(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 100.0)), 2.0);
        assert_eq!(fit.size(), Vec2::new(200.0, 100.0));
    }
}
