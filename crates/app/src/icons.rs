//! Toolbar icons, drawn as vectors in the style of the Rogeha Studio icon set: single-ink
//! outlines on a 24-unit grid, 1.75-unit strokes, round caps and joins.
//!
//! `Find` and `About` are the design system's own `search` and `info-circle` icons, copied
//! point for point. The system has no icons for the other actions, so those are drawn here
//! to the same rules. Every button carries a tooltip with the action's name.

use std::f32::consts::PI;

use egui::{Color32, Pos2, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    Open,
    Save,
    SaveAs,
    Print,
    Undo,
    Redo,
    Outline,
    ZoomOut,
    ZoomIn,
    FitWidth,
    FitPage,
    RotateLeft,
    RotateRight,
    Pages,
    Sign,
    Comments,
    Find,
    Assistant,
    Features,
    About,
}

/// Size of the drawn icon in points (the 24-unit grid scaled to 20).
const ICON: f32 = 20.0;
const BUTTON: Vec2 = Vec2::new(32.0, 26.0);

/// A toolbar button showing an icon. `selected` draws it as the active choice.
pub fn icon_button(ui: &mut Ui, icon: Icon, selected: bool, enabled: bool) -> Response {
    icon_button_with_text(ui, icon, "", selected, enabled)
}

/// An icon button with a short text after the icon (for example a count).
pub fn icon_button_with_text(ui: &mut Ui, icon: Icon, text: &str, selected: bool, enabled: bool) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = (!text.is_empty()).then(|| ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER));
    let extra = galley.as_ref().map_or(0.0, |g| g.size().x + 6.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(BUTTON + Vec2::new(extra, 0.0), sense);
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        let ink = if !enabled {
            ui.visuals().widgets.noninteractive.fg_stroke.color.gamma_multiply(0.4)
        } else if selected {
            ui.visuals().selection.stroke.color
        } else {
            visuals.fg_stroke.color
        };
        let fill = if selected { ui.visuals().selection.bg_fill } else { visuals.weak_bg_fill };
        ui.painter().rect(rect, visuals.corner_radius, fill, visuals.bg_stroke, StrokeKind::Inside);
        let icon_centre = Pos2::new(rect.left() + BUTTON.x / 2.0, rect.center().y);
        draw(ui, icon, icon_centre, ink);
        if let Some(galley) = galley {
            let pos = Pos2::new(rect.left() + BUTTON.x - 2.0, rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(pos, galley, ink);
        }
    }
    response
}

/// A button that opens a menu: its label followed by the design system's down chevron,
/// which is what marks a menu. Returns the response to attach the menu to.
pub fn menu_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let width = 10.0 + galley.size().x + 4.0 + 14.0 + 6.0;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, BUTTON.y), sense);
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let ink = if enabled { visuals.fg_stroke.color } else { ui.visuals().widgets.noninteractive.fg_stroke.color.gamma_multiply(0.4) };
        ui.painter().rect(rect, visuals.corner_radius, visuals.weak_bg_fill, visuals.bg_stroke, StrokeKind::Inside);
        ui.painter().galley(Pos2::new(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, ink);
        // `chevron-down.svg`: M6 9 l6 6 l6 -6, here at 14 of its 24 units.
        let unit = 14.0 / 24.0;
        let centre = Pos2::new(rect.right() - 6.0 - 7.0, rect.center().y);
        let at = |x: f32, y: f32| centre + Vec2::new(x - 12.0, y - 12.0) * unit;
        ui.painter().add(Shape::line(vec![at(6.0, 9.0), at(12.0, 15.0), at(18.0, 9.0)], Stroke::new(1.75 * unit.max(0.8), ink)));
    }
    response
}

fn draw(ui: &Ui, icon: Icon, centre: Pos2, ink: Color32) {
    let unit = ICON / 24.0;
    let stroke = Stroke::new(1.75 * unit, ink);
    // A point on the 24-unit grid, in screen coordinates.
    let at = |x: f32, y: f32| centre + Vec2::new(x - 12.0, y - 12.0) * unit;
    let painter = ui.painter();
    let line = |points: &[(f32, f32)]| {
        painter.add(Shape::line(points.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };
    let closed = |points: &[(f32, f32)]| {
        painter.add(Shape::closed_line(points.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };
    let circle = |x: f32, y: f32, r: f32| {
        painter.circle_stroke(at(x, y), r * unit, stroke);
    };
    // The design system's magnifier (`search.svg`): circle at 10.5 with a handle to 21.
    let magnifier = || {
        circle(10.5, 10.5, 6.5);
        line(&[(15.4, 15.4), (21.0, 21.0)]);
    };

    match icon {
        Icon::Open => {
            // A folder.
            closed(&[(3.0, 6.0), (9.0, 6.0), (11.5, 9.0), (21.0, 9.0), (21.0, 19.0), (3.0, 19.0)]);
        }
        Icon::Save => {
            // A disk: outline, label slot at the top, shutter at the bottom.
            closed(&[(4.0, 4.0), (16.5, 4.0), (20.0, 7.5), (20.0, 20.0), (4.0, 20.0)]);
            line(&[(8.0, 4.0), (8.0, 9.0), (15.0, 9.0), (15.0, 4.0)]);
            line(&[(8.0, 20.0), (8.0, 14.0), (16.0, 14.0), (16.0, 20.0)]);
        }
        Icon::SaveAs => {
            // The disk with its corner opened for a plus: "save another copy".
            line(&[(12.5, 20.0), (4.0, 20.0), (4.0, 4.0), (16.5, 4.0), (20.0, 7.5), (20.0, 12.5)]);
            line(&[(8.0, 4.0), (8.0, 9.0), (15.0, 9.0), (15.0, 4.0)]);
            line(&[(18.0, 15.0), (18.0, 21.0)]);
            line(&[(15.0, 18.0), (21.0, 18.0)]);
        }
        Icon::Undo | Icon::Redo => {
            // A straight arrow that comes back out of a U-turn: pointing left for undo,
            // right for redo. (The rotate icons are circles; these are deliberately not.)
            let flip = |x: f32| if matches!(icon, Icon::Redo) { 24.0 - x } else { x };
            line(&[(flip(8.5), 4.5), (flip(4.0), 9.0), (flip(8.5), 13.5)]);
            let mut path = vec![at(flip(4.0), 9.0), at(flip(14.5), 9.0)];
            // Half a circle of radius 5 around (14.5, 14), from its top round to its bottom.
            path.extend((1..=12).map(|i| {
                let angle = -PI / 2.0 + PI * i as f32 / 12.0;
                at(flip(14.5 + 5.0 * angle.cos()), 14.0 + 5.0 * angle.sin())
            }));
            path.push(at(flip(9.5), 19.0));
            painter.add(Shape::line(path, stroke));
        }
        Icon::Print => {
            // A printer: paper going in at the top, the body, the printed sheet coming out.
            line(&[(7.0, 8.0), (7.0, 3.5), (17.0, 3.5), (17.0, 8.0)]);
            line(&[(7.0, 17.0), (3.5, 17.0), (3.5, 8.0), (20.5, 8.0), (20.5, 17.0), (17.0, 17.0)]);
            closed(&[(7.0, 13.5), (17.0, 13.5), (17.0, 20.5), (7.0, 20.5)]);
        }
        Icon::Outline => {
            // A table of contents: three entries, the lower two indented.
            line(&[(4.0, 6.0), (20.0, 6.0)]);
            line(&[(8.0, 12.0), (20.0, 12.0)]);
            line(&[(8.0, 18.0), (20.0, 18.0)]);
            painter.circle_filled(at(4.3, 12.0), 0.9 * unit, ink);
            painter.circle_filled(at(4.3, 18.0), 0.9 * unit, ink);
        }
        Icon::ZoomOut => {
            magnifier();
            line(&[(7.5, 10.5), (13.5, 10.5)]);
        }
        Icon::ZoomIn => {
            magnifier();
            line(&[(7.5, 10.5), (13.5, 10.5)]);
            line(&[(10.5, 7.5), (10.5, 13.5)]);
        }
        Icon::FitWidth => {
            // Two walls with a double-headed arrow between them.
            line(&[(3.0, 5.0), (3.0, 19.0)]);
            line(&[(21.0, 5.0), (21.0, 19.0)]);
            line(&[(6.5, 12.0), (17.5, 12.0)]);
            line(&[(9.5, 9.0), (6.5, 12.0), (9.5, 15.0)]);
            line(&[(14.5, 9.0), (17.5, 12.0), (14.5, 15.0)]);
        }
        Icon::FitPage => {
            // A page held by four frame corners.
            closed(&[(8.0, 6.5), (16.0, 6.5), (16.0, 17.5), (8.0, 17.5)]);
            line(&[(3.0, 7.0), (3.0, 3.0), (7.0, 3.0)]);
            line(&[(17.0, 3.0), (21.0, 3.0), (21.0, 7.0)]);
            line(&[(21.0, 17.0), (21.0, 21.0), (17.0, 21.0)]);
            line(&[(7.0, 21.0), (3.0, 21.0), (3.0, 17.0)]);
        }
        Icon::RotateLeft | Icon::RotateRight => {
            // Three quarters of a circle with an arrow head at the open end.
            let mirror = if matches!(icon, Icon::RotateLeft) { -1.0 } else { 1.0 };
            let radius = 7.0;
            let arc: Vec<Pos2> = (0..=30)
                .map(|i| {
                    // Clockwise for "right" (screen y points down): from the right side round
                    // the bottom and the left up to the top, where the arrow head sits.
                    let angle = (i as f32 / 30.0) * 1.5 * PI;
                    at(12.0 + mirror * radius * angle.cos(), 13.0 + radius * angle.sin())
                })
                .collect();
            painter.add(Shape::line(arc, stroke));
            // The arc ends at the top; the arrow head there points along the turn.
            let (x, y) = (12.0, 6.0);
            line(&[(x - mirror * 3.5, y - 3.5), (x + mirror * 0.5, y), (x - mirror * 3.5, y + 3.5)]);
        }
        Icon::Pages => {
            // Two sheets, one behind the other.
            closed(&[(4.5, 7.5), (15.5, 7.5), (15.5, 21.0), (4.5, 21.0)]);
            line(&[(8.5, 7.5), (8.5, 3.0), (19.5, 3.0), (19.5, 16.5), (15.5, 16.5)]);
        }
        Icon::Sign => {
            // A pen and the line to sign on.
            closed(&[(15.0, 3.5), (19.5, 8.0), (9.0, 18.5), (4.0, 19.0), (4.5, 14.0)]);
            line(&[(12.5, 6.0), (17.0, 10.5)]);
            line(&[(13.0, 21.0), (21.0, 21.0)]);
        }
        Icon::Comments => {
            // A speech bubble.
            closed(&[(4.0, 4.5), (20.0, 4.5), (20.0, 16.0), (12.0, 16.0), (7.5, 20.5), (7.5, 16.0), (4.0, 16.0)]);
        }
        Icon::Find => magnifier(),
        Icon::Assistant => {
            // Two four-point stars: the usual mark for an AI assistant.
            closed(&[(10.0, 4.0), (12.0, 11.0), (19.0, 13.0), (12.0, 15.0), (10.0, 22.0), (8.0, 15.0), (1.0, 13.0), (8.0, 11.0)]);
            closed(&[(19.0, 2.0), (20.0, 4.5), (22.5, 5.5), (20.0, 6.5), (19.0, 9.0), (18.0, 6.5), (15.5, 5.5), (18.0, 4.5)]);
        }
        Icon::Features => {
            // Four cards, as on the feature overview page.
            closed(&[(3.5, 3.5), (10.5, 3.5), (10.5, 10.5), (3.5, 10.5)]);
            closed(&[(13.5, 3.5), (20.5, 3.5), (20.5, 10.5), (13.5, 10.5)]);
            closed(&[(3.5, 13.5), (10.5, 13.5), (10.5, 20.5), (3.5, 20.5)]);
            closed(&[(13.5, 13.5), (20.5, 13.5), (20.5, 20.5), (13.5, 20.5)]);
        }
        Icon::About => {
            // The design system's `info-circle.svg`.
            circle(12.0, 12.0, 9.0);
            line(&[(12.0, 11.2), (12.0, 16.6)]);
            painter.circle_filled(at(12.0, 7.8), 0.9 * unit, ink);
        }
    }
}
