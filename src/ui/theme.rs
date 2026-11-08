//! Dark theme, font loading and the generated window icon.

use std::sync::Arc;

use eframe::egui;

// ---------------------------------------------------------------- palette --

pub const BG: egui::Color32 = egui::Color32::from_rgb(13, 17, 23);
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(21, 27, 36);
pub const CARD: egui::Color32 = egui::Color32::from_rgb(27, 35, 47);
pub const CARD_HOVER: egui::Color32 = egui::Color32::from_rgb(34, 44, 59);
pub const BORDER: egui::Color32 = egui::Color32::from_rgb(44, 57, 74);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(226, 234, 244);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(138, 152, 170);
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(88, 148, 255);
pub const ACCENT_DIM: egui::Color32 = egui::Color32::from_rgb(46, 88, 168);
pub const GOOD: egui::Color32 = egui::Color32::from_rgb(63, 185, 110);
pub const WARN: egui::Color32 = egui::Color32::from_rgb(226, 164, 61);
pub const ERROR: egui::Color32 = egui::Color32::from_rgb(248, 81, 73);
pub const TRACK: egui::Color32 = egui::Color32::from_rgb(16, 21, 28);

/// Install the LinkVault theme into an egui context.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::ThemePreference::Dark);

    let text_styles = [
        (egui::TextStyle::Heading, egui::FontId::new(21.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Body, egui::FontId::new(14.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Button, egui::FontId::new(14.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Small, egui::FontId::new(11.5, egui::FontFamily::Proportional)),
        (egui::TextStyle::Monospace, egui::FontId::new(12.5, egui::FontFamily::Monospace)),
    ];

    ctx.all_styles_mut(|style| {
        style.visuals.dark_mode = true;
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = PANEL;
        style.visuals.extreme_bg_color = egui::Color32::from_rgb(10, 14, 19);
        style.visuals.faint_bg_color = egui::Color32::from_rgb(24, 31, 41);
        style.visuals.selection.bg_fill = ACCENT_DIM;
        style.visuals.selection.stroke = egui::Stroke::new(1.0, TEXT);
        style.visuals.hyperlink_color = ACCENT;
        style.visuals.warn_fg_color = WARN;
        style.visuals.error_fg_color = ERROR;
        style.visuals.window_corner_radius = egui::CornerRadius::same(10);
        style.visuals.window_stroke = egui::Stroke::new(1.0, BORDER);
        style.visuals.menu_corner_radius = egui::CornerRadius::same(8);
        style.visuals.popup_shadow = egui::epaint::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: egui::Color32::from_black_alpha(120),
        };

        let widgets = &mut style.visuals.widgets;
        widgets.noninteractive.bg_fill = PANEL;
        widgets.noninteractive.weak_bg_fill = PANEL;
        widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT);
        widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);

        widgets.inactive.bg_fill = egui::Color32::from_rgb(32, 41, 54);
        widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(32, 41, 54);
        widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
        widgets.inactive.corner_radius = egui::CornerRadius::same(6);

        widgets.hovered.bg_fill = egui::Color32::from_rgb(45, 58, 76);
        widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(45, 58, 76);
        widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
        widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        widgets.hovered.corner_radius = egui::CornerRadius::same(6);

        widgets.active.bg_fill = ACCENT_DIM;
        widgets.active.weak_bg_fill = ACCENT_DIM;
        widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
        widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        widgets.active.corner_radius = egui::CornerRadius::same(6);

        widgets.open = widgets.hovered;

        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.indent = 16.0;
        for (kind, font) in &text_styles {
            style.text_styles.insert(kind.clone(), font.clone());
        }
    });
}

/// Use Segoe UI (and Consolas) when available, keeping egui's default glyphs as fallback.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut proportional_first = Vec::new();

    let system_fonts = [
        ("segoeui", r"C:\Windows\Fonts\segoeui.ttf"),
        ("segoeui-b", r"C:\Windows\Fonts\segoeuib.ttf"),
    ];
    for (name, path) in system_fonts {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            proportional_first.push(name.to_owned());
        }
    }

    if !proportional_first.is_empty() {
        let family = fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default();
        let mut merged = proportional_first;
        merged.append(family);
        *family = merged;
    }

    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\consola.ttf") {
        fonts
            .font_data
            .insert("consola".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
        let family = fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default();
        family.insert(0, "consola".to_owned());
    }

    ctx.set_fonts(fonts);
}

// ------------------------------------------------------------------ icon ----

/// Procedurally drawn icon: a dark rounded tile with an accent arrow moving a folder out.
pub fn make_icon() -> egui::IconData {
    const S: u32 = 64;
    let mut rgba = vec![0u8; (S * S * 4) as usize];
    let radius = 14.0f32;

    let inside = |x: f32, y: f32| -> f32 {
        // Signed distance for a rounded rectangle, returned as coverage 0..1.
        let half = (S as f32) / 2.0;
        let px = (x - half).abs() - (half - radius);
        let py = (y - half).abs() - (half - radius);
        let dist = px.max(0.0).hypot(py.max(0.0)) + px.max(py).min(0.0) - radius;
        (0.5 - dist).clamp(0.0, 1.0)
    };

    // Arrow polygon (pointing right) in icon space.
    let arrow = |x: f32, y: f32| -> bool {
        let (x0, x1) = (18.0, 46.0);
        let (y0, y1) = (26.0, 38.0);
        let shaft = x >= x0 && x <= 36.0 && y >= y0 + 3.0 && y <= y1 - 3.0;
        let head = x >= 32.0 && x <= x1 && (y - 32.0).abs() <= (x1 - x) * 0.75;
        shaft || head
    };

    for y in 0..S {
        for x in 0..S {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let cov = inside(fx, fy);
            let idx = ((y * S + x) * 4) as usize;
            if cov <= 0.0 {
                continue;
            }
            let base = if arrow(fx, fy) { ACCENT } else { PANEL };
            // Subtle vertical shading so the tile does not look flat.
            let shade = 0.85 + 0.15 * (1.0 - fy / S as f32);
            let mix = |c: egui::Color32| {
                egui::Color32::from_rgb(
                    (c.r() as f32 * shade) as u8,
                    (c.g() as f32 * shade) as u8,
                    (c.b() as f32 * shade) as u8,
                )
            };
            let color = mix(base);
            rgba[idx] = color.r();
            rgba[idx + 1] = color.g();
            rgba[idx + 2] = color.b();
            rgba[idx + 3] = (cov * 255.0) as u8;
        }
    }

    egui::IconData {
        rgba,
        width: S,
        height: S,
    }
}