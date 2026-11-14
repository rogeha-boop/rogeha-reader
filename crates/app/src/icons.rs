//! Toolbar icons, drawn as vectors in the style of the Rogeha Studio icon set: single-ink
//! outlines on a 24-unit grid, 1.75-unit strokes, round caps and joins. The design system's
//! own set has no icons for these actions, so they are drawn here to the same rules.
//! Each button carries a tooltip with the action's name.

use std::f32::consts::PI;

use egui::{Color32, Pos2, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    FitWidth,
    FitPage,
    RotateLeft,
    RotateRight,
}

/// Size of the drawn icon in points (the 24-unit grid scaled to 20).
const ICON: f32 = 20.0;
const BUTTON: Vec2 = Vec2::new(32.0, 26.0);

/// A toolbar button showing an icon. `selected` draws it as the active choice.
pub fn icon_button(ui: &mut Ui, icon: Icon, selected: bool, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(BUTTON, sense);
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
        draw(ui, icon, rect.center(), ink);
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

    match icon {
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
            line(&[(8.0, 6.5), (16.0, 6.5), (16.0, 17.5), (8.0, 17.5), (8.0, 6.5)]);
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
    }
}
