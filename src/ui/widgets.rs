//! Small reusable widgets: cards, chips, stat tiles, progress rows.

use eframe::egui;

use super::theme::*;
use crate::app::{LogKind, Progress};
use crate::util;

/// A rounded, bordered container used for every "panel" in the UI.
///
/// The card always spans the full width offered by its parent, so stacked
/// panels line up on both edges instead of hugging their content.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Card that highlights when the pointer is over it (used for clickable rows).
pub fn card_hover<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let frame = egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        });
    if ui.rect_contains_pointer(frame.response.rect) {
        ui.painter().rect_stroke(
            frame.response.rect,
            egui::CornerRadius::same(8),
            egui::Stroke::new(1.0, CARD_HOVER),
            egui::StrokeKind::Inside,
        );
    }
    frame.inner
}

/// Full-width list row with optional zebra background and hover highlight.
pub fn list_row<R>(
    ui: &mut egui::Ui,
    striped: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let frame = egui::Frame::new()
        .fill(if striped {
            CARD.gamma_multiply(0.55)
        } else {
            egui::Color32::TRANSPARENT
        })
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        });
    if ui.rect_contains_pointer(frame.response.rect) {
        ui.painter().rect_stroke(
            frame.response.rect,
            egui::CornerRadius::same(8),
            egui::Stroke::new(1.0, CARD_HOVER),
            egui::StrokeKind::Inside,
        );
    }
    frame.inner
}

pub fn section_title(ui: &mut egui::Ui, title: &str, subtitle: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).size(17.0).strong().color(TEXT));
        if let Some(sub) = subtitle {
            ui.label(egui::RichText::new(sub).size(12.0).color(MUTED));
        }
    });
    ui.add_space(2.0);
}

/// Small colored label, e.g. state badges.
pub fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.55)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(11.5).color(color));
        })
        .response
}

/// Big number + caption tile used on the dashboard. Stretches to the width of
/// its parent - put it in `stat_row` (or a column) to get equal tiles.
pub fn stat_tile(ui: &mut egui::Ui, caption: &str, value: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(9))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(caption).size(11.0).color(MUTED));
                ui.label(egui::RichText::new(value).size(19.0).strong().color(color));
            });
        });
}

/// A row of equally wide stat tiles that always fills the available width.
pub fn stat_row(ui: &mut egui::Ui, tiles: &[(&str, String, egui::Color32)]) {
    if tiles.is_empty() {
        return;
    }
    let count = tiles.len();
    ui.columns(count, |columns| {
        for (column, (caption, value, color)) in columns.iter_mut().zip(tiles.iter()) {
            stat_tile(column, caption, value, *color);
        }
    });
}

/// Horizontal usage bar (used for disk fill).
pub fn usage_bar(ui: &mut egui::Ui, fraction: f32, color: egui::Color32) -> egui::Response {
    let width = ui.available_width().max(60.0);
    let height = 8.0;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::same(4), TRACK);
    let filled = egui::Rect::from_min_size(
        rect.min,
        egui::vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()),
    );
    if filled.width() > 0.5 {
        painter.rect_filled(filled, egui::CornerRadius::same(4), color);
    }
    response
}

/// Full-width progress bar with phase + detail text above it.
pub fn progress_row(ui: &mut egui::Ui, progress: &Progress) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&progress.phase)
                    .size(13.0)
                    .strong()
                    .color(TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let pct = progress.percent_text();
                if !pct.is_empty() {
                    ui.label(egui::RichText::new(pct).size(12.0).color(MUTED));
                }
            });
        });
        ui.add(
            egui::ProgressBar::new(progress.fraction())
                .desired_height(10.0)
                .fill(ACCENT)
                .animate(progress.total == 0),
        );
        if !progress.detail.is_empty() {
            ui.label(
                egui::RichText::new(util::ellipsize_middle(&progress.detail, 110))
                    .size(11.0)
                    .color(MUTED),
            );
        }
    });
}

pub fn primary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.to_owned()).color(egui::Color32::WHITE))
        .fill(ACCENT_DIM)
        .stroke(egui::Stroke::new(1.0, ACCENT))
        .corner_radius(egui::CornerRadius::same(7))
        .min_size(egui::vec2(0.0, 30.0))
}

pub fn danger_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.to_owned()).color(egui::Color32::WHITE))
        .fill(egui::Color32::from_rgb(120, 32, 30))
        .stroke(egui::Stroke::new(1.0, ERROR.gamma_multiply(0.7)))
        .corner_radius(egui::CornerRadius::same(7))
        .min_size(egui::vec2(0.0, 26.0))
}

pub fn ghost_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.to_owned()).size(12.5).color(TEXT))
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(6))
}

pub fn log_color(kind: LogKind) -> egui::Color32 {
    match kind {
        LogKind::Info => TEXT,
        LogKind::Good => GOOD,
        LogKind::Warn => WARN,
        LogKind::Error => ERROR,
    }
}

pub fn log_icon(kind: LogKind) -> &'static str {
    match kind {
        LogKind::Info => "•",
        LogKind::Good => "✔",
        LogKind::Warn => "!",
        LogKind::Error => "✖",
    }
}

/// Muted single-line hint text.
pub fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text).size(11.5).color(MUTED));
}

/// A label/value pair laid out on one row.
pub fn kv(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(key).size(11.5).color(MUTED));
        ui.label(egui::RichText::new(value).size(11.5).color(TEXT));
    });
}

/// Placeholder shown instead of an empty list. Centred inside a full-width
/// dashed-looking box so an empty tab still fills the window.
pub fn empty_state(ui: &mut egui::Ui, title: &str, hint_text: &str) {
    egui::Frame::new()
        .fill(CARD.gamma_multiply(0.35))
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(18.0);
                ui.label(egui::RichText::new(title).size(15.0).color(MUTED));
                ui.add_space(4.0);
                ui.label(egui::RichText::new(hint_text).size(12.0).color(MUTED));
                ui.add_space(18.0);
            });
        });
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Render `build` inside a headless egui context and return
    /// `(available width, width actually used)`.
    ///
    /// Several passes are needed because egui measures text on the first frame
    /// and only then knows the real heights.
    fn measure(build: impl Fn(&mut egui::Ui)) -> (f32, f32) {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mut available = 0.0;
        let mut used = 0.0;
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |root| {
                egui::CentralPanel::no_frame().show(root, |ui| {
                    available = ui.available_width();
                    // Measure the bounding box of the content only - the
                    // panel's own `min_rect` always spans the whole window.
                    let scoped = ui.scope(|ui| build(ui));
                    used = scoped.response.rect.width();
                });
            });
            // Headless: nothing uploads the font atlas, so discard the delta.
            output.textures_delta.clear();
        }
        (available, used)
    }

    #[test]
    fn measurement_detects_narrow_content() {
        // Guards the measurement itself: a bare label must not look full width.
        let (available, used) = measure(|ui| {
            ui.label("malý text");
        });
        assert!(
            used < available - 10.0,
            "label should stay narrow, used {used} of {available}"
        );
    }

    #[test]
    fn cards_fill_the_available_width() {
        let (available, used) = measure(|ui| {
            card(ui, |ui| {
                hint(ui, "text");
            });
        });
        assert!(
            (used - available).abs() < 2.0,
            "card used {used} of {available}"
        );
    }

    #[test]
    fn stat_rows_fill_the_available_width() {
        let (available, used) = measure(|ui| {
            let tiles = [
                ("a", "1 GB".to_string(), TEXT),
                ("b", "2 GB".to_string(), TEXT),
                ("c", "3 GB".to_string(), TEXT),
            ];
            stat_row(ui, &tiles);
        });
        assert!(
            (used - available).abs() < 2.0,
            "stat row used {used} of {available}"
        );
    }

    #[test]
    fn list_rows_fill_the_available_width() {
        let (available, used) = measure(|ui| {
            list_row(ui, true, |ui| {
                ui.label("řádek");
            });
        });
        assert!(
            (used - available).abs() < 2.0,
            "list row used {used} of {available}"
        );
    }
}
