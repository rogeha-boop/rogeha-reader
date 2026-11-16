//! The About window, following the Rogeha Studio design system: the Studio cover banner on
//! its own `canvas` ground (never rounded, cropped or framed), IBM Plex type, links in
//! `steel-500`, and no accent besides the banner's own probe.

use egui::{Color32, RichText, TextureHandle, TextureOptions, Vec2};

use super::ReaderApp;
use crate::theme;

const APP_NAME: &str = "Rogeha Reader";
const COPYRIGHT: &str = "© 2026 Rogeha Studio";
const SUPPORT_EMAIL: &str = "support@rogehastudio.com";
const SOURCE_URL: &str = "https://github.com/rogeha-boop/rogeha-reader";
// Links to the studio's website carry `?from=app`, so the website's own visitor count can tell
// pages opened from the reader apart from other visits. A page all the studio's apps share (the
// legal notice) names the app: `?from=app-rogeha-reader`. The reader itself sends nothing.
const LEGAL_URL: &str = "https://rogehastudio.com/legal.html?from=app-rogeha-reader";
/// The page on the studio's website that shows what the reader can do.
pub(super) const FEATURES_URL: &str = "https://rogehastudio.com/projects/rogeha-reader/?from=app";

const BANNER_LIGHT: &[u8] = include_bytes!("../../assets/rogeha-studio-cover-light.svg");
const BANNER_DARK: &[u8] = include_bytes!("../../assets/rogeha-studio-cover-dark.svg");
const ICON: &[u8] = include_bytes!("../../assets/rogeha-reader-icon.svg");

/// The banner is 960 x 320; shown at half size it keeps its pluses and hairline readable.
const BANNER_SIZE: Vec2 = Vec2::new(480.0, 160.0);
const ICON_SIZE: f32 = 56.0;

#[derive(Default)]
pub(super) struct About {
    pub open: bool,
    /// Banner and icon textures, with the (dark, pixels per point) they were drawn for.
    art: Option<(bool, f32, TextureHandle, TextureHandle)>,
}

/// The version shown to people: MAJOR.MINOR from the package, then the build number (the
/// number of commits, set by build.rs), and the commit the build was made from.
/// For example `Version 0.1.31 (6f4ca52)`; a trailing `+` means uncommitted changes.
pub fn version_text() -> String {
    let base = concat!(env!("CARGO_PKG_VERSION_MAJOR"), ".", env!("CARGO_PKG_VERSION_MINOR"));
    let (build, commit) = (env!("ROGEHA_BUILD_NUMBER"), env!("ROGEHA_COMMIT"));
    if commit.is_empty() {
        format!("Version {base}.{build}")
    } else {
        format!("Version {base}.{build} ({commit})")
    }
}

/// Draw an SVG with MuPDF at exactly the pixels it will occupy, so it is crisp on any screen.
fn svg_texture(ctx: &egui::Context, name: &str, svg: &[u8], width_points: f32, cut_corners: bool) -> Option<TextureHandle> {
    let width = (width_points * ctx.pixels_per_point()).round().max(1.0) as usize;
    let image = rogeha_core::render_svg(svg, width).ok()?;
    let (w, h) = (image.width, image.height);
    let radius = w as f32 * 28.0 / 128.0;
    let pixels = image
        .pixels
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let color = Color32::from_rgb((p >> 16) as u8, (p >> 8) as u8, *p as u8);
            if !cut_corners {
                return color;
            }
            // The app tile has rounded corners; make what lies outside them transparent.
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let dx = (radius - x).max(x - (w as f32 - radius)).max(0.0);
            let dy = (radius - y).max(y - (h as f32 - radius)).max(0.0);
            let cover = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            color.gamma_multiply(cover)
        })
        .collect();
    Some(ctx.load_texture(name, egui::ColorImage::new([w, h], pixels), TextureOptions::LINEAR))
}

impl ReaderApp {
    pub(super) fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about.open {
            self.about.art = None; // free the textures while the window is closed
            return;
        }
        let dark = ctx.theme() == egui::Theme::Dark;
        let t = theme::tokens(dark);
        let ppp = ctx.pixels_per_point();
        if !self.about.art.as_ref().is_some_and(|(d, p, ..)| *d == dark && *p == ppp) {
            let banner = svg_texture(ctx, "about-banner", if dark { BANNER_DARK } else { BANNER_LIGHT }, BANNER_SIZE.x, false);
            let icon = svg_texture(ctx, "about-icon", ICON, ICON_SIZE, true);
            self.about.art = banner.zip(icon).map(|(b, i)| (dark, ppp, b, i));
        }

        let mut open = true;
        let mut close = false;
        // The window's ground is `canvas`, the ground the banner files are drawn on,
        // so the banner needs no frame.
        let frame = egui::Frame::window(&ctx.global_style()).fill(t.canvas).inner_margin(24.0);
        egui::Window::new(format!("About {APP_NAME}"))
            .id(egui::Id::unique("about-window"))
            .open(&mut open)
            .frame(frame)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(BANNER_SIZE.x);
                ui.spacing_mut().item_spacing.y = 8.0;
                if let Some((_, _, banner, icon)) = &self.about.art {
                    ui.add(egui::Image::new((banner.id(), BANNER_SIZE)));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Image::new((icon.id(), Vec2::splat(ICON_SIZE))));
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.add_space(4.0);
                            ui.label(RichText::new(APP_NAME).font(egui::FontId::new(24.0, theme::semibold())).color(t.ink));
                            // Versions are measured values: Plex Mono.
                            ui.label(RichText::new(version_text()).monospace().color(t.ink_muted));
                        });
                    });
                } else {
                    ui.label(RichText::new(APP_NAME).font(egui::FontId::new(24.0, theme::semibold())));
                }
                ui.add_space(8.0);
                ui.label(RichText::new("A fast, free reader for PDF and EPUB files.").color(t.ink));
                ui.label(RichText::new(COPYRIGHT).color(t.ink_muted));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new("Questions or feedback:").color(t.ink_muted));
                    ui.hyperlink_to(SUPPORT_EMAIL, format!("mailto:{SUPPORT_EMAIL}"));
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    ui.hyperlink_to("Source code", SOURCE_URL);
                    ui.hyperlink_to("Legal notice", LEGAL_URL);
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Free software under the GNU Affero General Public License, version 3 or later. \
                         Built on MuPDF by Artifex Software and on egui. Set in IBM Plex.",
                    )
                    .small()
                    .color(t.ink_muted),
                );
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Primary action: steel-500.
                    let ok = egui::Button::new(RichText::new("OK").color(t.on_steel))
                        .fill(t.steel_500)
                        .min_size(Vec2::new(88.0, 30.0));
                    if ui.add(ok).clicked() {
                        close = true;
                    }
                });
            });
        if close || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.about.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn website_links_say_they_come_from_the_app() {
        assert_eq!(super::FEATURES_URL, "https://rogehastudio.com/projects/rogeha-reader/?from=app");
        assert_eq!(super::LEGAL_URL, "https://rogehastudio.com/legal.html?from=app-rogeha-reader");
    }
}
