//! Colours, visuals and small reusable widgets.

use eframe::egui::{self, Color32, Response, RichText, Rounding, Sense, Stroke, Ui, Vec2};

pub const BG: Color32 = Color32::from_rgb(0x10, 0x12, 0x16);
pub const PANEL: Color32 = Color32::from_rgb(0x16, 0x19, 0x1F);
pub const CARD: Color32 = Color32::from_rgb(0x1D, 0x21, 0x29);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0x23, 0x28, 0x32);
pub const BORDER: Color32 = Color32::from_rgb(0x2A, 0x30, 0x3B);
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE9, 0xEF);
pub const MUTED: Color32 = Color32::from_rgb(0x8C, 0x95, 0xA5);
pub const ACCENT: Color32 = Color32::from_rgb(0x4F, 0x9C, 0xF9);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x1F, 0x3A, 0x5E);
pub const GOOD: Color32 = Color32::from_rgb(0x3F, 0xB9, 0x7C);
pub const WARN: Color32 = Color32::from_rgb(0xE8, 0xA3, 0x3D);
pub const BAD: Color32 = Color32::from_rgb(0xE5, 0x53, 0x4B);
pub const NSFW: Color32 = Color32::from_rgb(0xE2, 0x55, 0x7B);
pub const MWS: Color32 = Color32::from_rgb(0x4C, 0x8B, 0xF5);
pub const NEXUS: Color32 = Color32::from_rgb(0xDA, 0x8E, 0x35);

pub fn apply(ctx: &egui::Context) {
    // Always dark, regardless of the Windows light/dark setting.
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
    ctx.set_theme(egui::Theme::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(0x0C, 0x0E, 0x11);
    v.faint_bg_color = CARD;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_rounding = Rounding::same(10.0_f32);
    v.menu_rounding = Rounding::same(8.0_f32);
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.fg_stroke.color = TEXT;
    v.widgets.inactive.fg_stroke.color = TEXT;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = Rounding::same(6.0_f32);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x25, 0x2A, 0x34);
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x25, 0x2A, 0x34);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2E, 0x35, 0x42);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x2E, 0x35, 0x42);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x3D, 0x46, 0x55));
    v.widgets.active.weak_bg_fill = ACCENT_DIM;
    v.widgets.active.bg_fill = ACCENT_DIM;
    ctx.set_visuals(v);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(22.0));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(11.5));
    ctx.set_style(style);
}

pub fn rgb(c: Option<[u8; 3]>) -> Color32 {
    c.map(|[r, g, b]| Color32::from_rgb(r, g, b)).unwrap_or(Color32::from_rgb(0x5A, 0x63, 0x73))
}

/// A small coloured pill. Clickable when `interactive`.
pub fn chip(ui: &mut Ui, text: &str, color: Color32, selected: bool, interactive: bool) -> Response {
    let font = egui::FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, TEXT);
    let size = galley.size() + egui::vec2(16.0, 6.0);
    let sense = if interactive { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        let alpha = if selected { 200 } else if resp.hovered() && interactive { 110 } else { 60 };
        let fill = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
        let stroke = if selected { Stroke::new(1.0_f32, color) } else { Stroke::new(1.0_f32, color.gamma_multiply(0.6)) };
        ui.painter().rect(rect, Rounding::same(rect.height() / 2.0), fill, stroke);
        let text_color = if selected { Color32::WHITE } else { TEXT };
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, text_color);
    }
    if interactive {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// iOS-style toggle switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = egui::vec2(36.0, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool(resp.id, *on);
        let bg = lerp_color(Color32::from_rgb(0x3A, 0x41, 0x4E), GOOD, t);
        ui.painter().rect_filled(rect, Rounding::same(10.0_f32), bg);
        let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, t);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), 7.5, Color32::WHITE);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
            .fill(ACCENT)
            .rounding(Rounding::same(6.0_f32)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn colored_button(ui: &mut Ui, text: &str, fill: Color32) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE))
            .fill(fill)
            .rounding(Rounding::same(6.0_f32)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn ghost_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(RichText::new(text).color(TEXT)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0_f32, BORDER)))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(11.5).color(MUTED).strong());
}

pub fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

pub fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn fmt_size(b: u64) -> String {
    if b == 0 {
        String::new()
    } else if b >= 1_048_576 {
        format!("{:.1} MB", b as f64 / 1_048_576.0)
    } else {
        format!("{:.0} KB", (b as f64 / 1024.0).max(1.0))
    }
}
