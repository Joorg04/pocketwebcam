//! Small line icons drawn with the painter, so they stay sharp at any scale and need no icon
//! font. Each is drawn in a square `rect`, in the style of a 24 px stroke icon set.

use std::f32::consts::TAU;

use eframe::egui::{pos2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy)]
pub enum Icon {
    Gear,
    ChevronDown,
    Mirror,
    Bolt,
    /// A filled dot (record).
    Record,
    /// A camera with a shutter (take a snapshot).
    Shutter,
    /// A filled triangle (video thumbnail badge).
    Play,
    Plus,
    Mic,
    /// A microphone with a slash through it.
    MicOff,
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    // Map the 24×24 design grid onto `rect`.
    let s = rect.width() / 24.0;
    let p = |x: f32, y: f32| -> Pos2 { pos2(rect.left() + x * s, rect.top() + y * s) };
    let stroke = Stroke::new((1.8 * s).max(1.2), color);
    match icon {
        Icon::Gear => {
            let c = p(12.0, 12.0);
            for i in 0..8 {
                let a = i as f32 / 8.0 * TAU;
                let (sin, cos) = a.sin_cos();
                painter.line_segment(
                    [pos2(c.x + cos * 7.0 * s, c.y + sin * 7.0 * s), pos2(c.x + cos * 10.0 * s, c.y + sin * 10.0 * s)],
                    Stroke::new(3.0 * s, color),
                );
            }
            painter.circle_stroke(c, 7.0 * s, stroke);
            painter.circle_stroke(c, 3.0 * s, stroke);
        }
        Icon::ChevronDown => {
            painter.add(Shape::line(vec![p(6.0, 9.0), p(12.0, 15.0), p(18.0, 9.0)], stroke));
        }
        Icon::Mirror => {
            // Two triangles facing apart, split by a dashed centre line.
            painter.add(Shape::closed_line(vec![p(9.0, 6.0), p(9.0, 18.0), p(3.0, 12.0)], stroke));
            painter.add(Shape::convex_polygon(vec![p(15.0, 6.0), p(21.0, 12.0), p(15.0, 18.0)], color, stroke));
            for y in [3.0, 9.0, 15.0] {
                painter.line_segment([p(12.0, y), p(12.0, y + 3.5)], stroke);
            }
        }
        Icon::Bolt => {
            painter.add(Shape::closed_line(
                vec![p(13.0, 2.0), p(3.0, 14.0), p(12.0, 14.0), p(11.0, 22.0), p(21.0, 10.0), p(12.0, 10.0)],
                stroke,
            ));
        }
        Icon::Record => {
            painter.circle_filled(p(12.0, 12.0), 6.0 * s, color);
        }
        Icon::Shutter => {
            // Camera body with a bump on top, and a lens.
            painter.add(Shape::closed_line(
                vec![
                    p(3.0, 8.0),
                    p(7.5, 8.0),
                    p(9.0, 5.0),
                    p(15.0, 5.0),
                    p(16.5, 8.0),
                    p(21.0, 8.0),
                    p(21.0, 19.0),
                    p(3.0, 19.0),
                ],
                stroke,
            ));
            painter.circle_stroke(p(12.0, 13.0), 3.5 * s, stroke);
        }
        Icon::Play => {
            painter.add(Shape::convex_polygon(vec![p(7.0, 5.0), p(19.0, 12.0), p(7.0, 19.0)], color, Stroke::NONE));
        }
        Icon::Mic | Icon::MicOff => {
            // Capsule, the U-shaped holder below it, and the stand.
            painter.rect_stroke(
                Rect::from_min_max(p(9.0, 3.0), p(15.0, 14.0)),
                3.0 * s,
                stroke,
                eframe::egui::StrokeKind::Middle,
            );
            let holder: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = i as f32 / 12.0 * TAU / 2.0;
                    p(12.0 - 6.5 * a.cos(), 11.0 + 6.5 * a.sin())
                })
                .collect();
            painter.add(Shape::line(holder, stroke));
            painter.line_segment([p(12.0, 17.5), p(12.0, 21.0)], stroke);
            if matches!(icon, Icon::MicOff) {
                painter.line_segment([p(4.0, 3.0), p(20.0, 21.0)], stroke);
            }
        }
        Icon::Plus => {
            painter.line_segment([p(12.0, 5.0), p(12.0, 19.0)], stroke);
            painter.line_segment([p(5.0, 12.0), p(19.0, 12.0)], stroke);
        }
    }
}
