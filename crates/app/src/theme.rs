//! The Rogeha Studio design system, as far as this app uses it: colour tokens for both themes,
//! IBM Plex type, and the radii. Values come from the design system's `tokens.json`
//! (source of truth: the Rogeha Studio design system on claude.ai). Change them there first.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Theme};

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// One theme's colour tokens (a few are kept for upcoming screens).
#[allow(dead_code)]
pub struct Tokens {
    pub canvas: Color32,
    pub surface: Color32,
    pub surface_raised: Color32,
    pub surface_sunken: Color32,
    pub border_subtle: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub ink: Color32,
    pub ink_muted: Color32,
    pub ink_subtle: Color32,
    pub ink_disabled: Color32,
    pub steel_100: Color32,
    pub steel_500: Color32,
    pub steel_600: Color32,
    pub on_steel: Color32,
    /// The accent. One per screen.
    pub signal_500: Color32,
    pub danger_700: Color32,
    pub warning_700: Color32,
}

pub const LIGHT: Tokens = Tokens {
    canvas: hex(0xf4f5f7),
    surface: hex(0xffffff),
    surface_raised: hex(0xffffff),
    surface_sunken: hex(0xe9ecef),
    border_subtle: hex(0xe3e6ea),
    border: hex(0x838d97),
    border_strong: hex(0x576169),
    ink: hex(0x14181c),
    ink_muted: hex(0x4d5862),
    ink_subtle: hex(0x626c75),
    ink_disabled: hex(0x98a1aa),
    steel_100: hex(0xe4ecf5),
    steel_500: hex(0x2f5c8a),
    steel_600: hex(0x244a71),
    on_steel: hex(0xffffff),
    signal_500: hex(0xe0670f),
    danger_700: hex(0x9c211a),
    warning_700: hex(0x7a5200),
};

pub const DARK: Tokens = Tokens {
    canvas: hex(0x101417),
    surface: hex(0x171c20),
    surface_raised: hex(0x1e242a),
    surface_sunken: hex(0x0b0e10),
    border_subtle: hex(0x252c33),
    border: hex(0x69737c),
    border_strong: hex(0x939da6),
    ink: hex(0xeef1f4),
    ink_muted: hex(0xa6b0b9),
    ink_subtle: hex(0x8b959e),
    ink_disabled: hex(0x68727b),
    steel_100: hex(0x1c2a38),
    steel_500: hex(0x5b93cc),
    steel_600: hex(0x7dabdc),
    on_steel: hex(0x0b1218),
    signal_500: hex(0xf28b3d),
    danger_700: hex(0xff8a80),
    warning_700: hex(0xe8c268),
};

pub fn tokens(dark: bool) -> &'static Tokens {
    if dark { &DARK } else { &LIGHT }
}

/// Font family for headings and names: IBM Plex Sans SemiBold (600).
pub fn semibold() -> FontFamily {
    FontFamily::Name("plex-semibold".into())
}

// Radii, from the logo tile (28 of 128).
pub const RADIUS_SM: u8 = 5; // buttons, inputs
pub const RADIUS_MD: u8 = 8; // panels, popovers, windows

fn visuals(dark: bool) -> egui::Visuals {
    let t = tokens(dark);
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = t.surface;
    v.window_fill = t.surface_raised;
    v.extreme_bg_color = t.surface_sunken;
    v.faint_bg_color = t.surface_sunken;
    v.code_bg_color = t.surface_sunken;
    v.window_stroke = Stroke::new(1.0, t.border_subtle);
    v.weak_text_color = Some(t.ink_muted);
    v.hyperlink_color = t.steel_500;
    v.error_fg_color = t.danger_700;
    v.warn_fg_color = t.warning_700;
    v.selection.bg_fill = t.steel_500;
    v.selection.stroke = Stroke::new(1.0, t.on_steel);
    v.window_corner_radius = CornerRadius::same(RADIUS_MD);
    v.menu_corner_radius = CornerRadius::same(RADIUS_MD);

    let radius = CornerRadius::same(RADIUS_SM);
    let w = &mut v.widgets;
    // Labels, separators, frames.
    w.noninteractive.bg_fill = t.surface;
    w.noninteractive.weak_bg_fill = t.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border_subtle);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.ink);
    w.noninteractive.corner_radius = radius;
    // Controls at rest: the border is what marks the control.
    w.inactive.bg_fill = t.surface_sunken;
    w.inactive.weak_bg_fill = t.surface;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border);
    w.inactive.fg_stroke = Stroke::new(1.0, t.ink);
    w.inactive.corner_radius = radius;
    w.hovered.bg_fill = t.steel_100;
    w.hovered.weak_bg_fill = t.steel_100;
    w.hovered.bg_stroke = Stroke::new(1.0, t.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, t.ink);
    w.hovered.corner_radius = radius;
    w.active.bg_fill = t.steel_100;
    w.active.weak_bg_fill = t.steel_100;
    w.active.bg_stroke = Stroke::new(1.0, t.steel_500);
    w.active.fg_stroke = Stroke::new(1.0, t.ink);
    w.active.corner_radius = radius;
    w.open.bg_fill = t.steel_100;
    w.open.weak_bg_fill = t.steel_100;
    w.open.bg_stroke = Stroke::new(1.0, t.border_strong);
    w.open.fg_stroke = Stroke::new(1.0, t.ink);
    w.open.corner_radius = radius;
    v
}

/// Install the fonts and both themes. The app then follows the system's light/dark setting.
pub fn apply(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add("plex-sans", include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"));
    add("plex-sans-semibold", include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"));
    add("plex-mono", include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"));
    // Plex first; egui's built-in fonts stay as a fallback for symbols Plex lacks.
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "plex-sans".to_owned());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "plex-mono".to_owned());
    let mut semibold_chain = vec!["plex-sans-semibold".to_owned()];
    semibold_chain.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(semibold(), semibold_chain);
    ctx.set_fonts(fonts);

    ctx.set_visuals_of(Theme::Dark, visuals(true));
    ctx.set_visuals_of(Theme::Light, visuals(false));
    ctx.all_styles_mut(|style| {
        // The design system's text scale, at the sizes a compact desktop tool uses.
        style.text_styles = [
            (TextStyle::Heading, FontId::new(19.0, semibold())), // heading-m
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)), // label
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)), // data-s
        ]
        .into();
        // The 4px grid.
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
    });
}
