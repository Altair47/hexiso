// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! Dark "terminal" look: near-black panels, phosphor green, cyan highlights, monospace everywhere.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

pub const BG: Color32 = Color32::from_rgb(4, 7, 9);
pub const PANEL: Color32 = Color32::from_rgb(8, 13, 16);
pub const PANEL2: Color32 = Color32::from_rgb(12, 20, 24);
pub const ROW_ALT: Color32 = Color32::from_rgb(10, 16, 19);
pub const LINE: Color32 = Color32::from_rgb(20, 52, 44);
pub const GREEN: Color32 = Color32::from_rgb(0, 255, 156);
pub const GREEN_DIM: Color32 = Color32::from_rgb(0, 150, 96);
pub const CYAN: Color32 = Color32::from_rgb(0, 217, 255);
pub const AMBER: Color32 = Color32::from_rgb(255, 184, 0);
pub const RED: Color32 = Color32::from_rgb(255, 64, 96);
pub const TEXT: Color32 = Color32::from_rgb(190, 240, 220);
pub const DIM: Color32 = Color32::from_rgb(92, 128, 116);
pub const SEL: Color32 = Color32::from_rgb(0, 70, 48);

pub fn apply(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // everything monospace
    let mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    fonts.families.insert(FontFamily::Proportional, mono);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::monospace(10.5)),
            (TextStyle::Body, FontId::monospace(13.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
            (TextStyle::Button, FontId::monospace(13.0)),
            (TextStyle::Heading, FontId::monospace(17.0)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
        let v = &mut s.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = Some(TEXT);
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = BG;
        v.faint_bg_color = ROW_ALT;
        v.code_bg_color = BG;
        v.window_stroke = Stroke::new(1.0, GREEN_DIM);
        v.window_corner_radius = CornerRadius::same(2);
        v.menu_corner_radius = CornerRadius::same(2);
        v.hyperlink_color = CYAN;
        v.selection.bg_fill = SEL;
        v.selection.stroke = Stroke::new(1.0, GREEN);
        v.warn_fg_color = AMBER;
        v.error_fg_color = RED;
        let r = CornerRadius::same(1);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = r;
        w.inactive.bg_fill = PANEL2;
        w.inactive.weak_bg_fill = PANEL2;
        w.inactive.bg_stroke = Stroke::new(1.0, LINE);
        w.inactive.fg_stroke = Stroke::new(1.0, GREEN);
        w.inactive.corner_radius = r;
        w.hovered.bg_fill = Color32::from_rgb(10, 40, 32);
        w.hovered.weak_bg_fill = Color32::from_rgb(10, 40, 32);
        w.hovered.bg_stroke = Stroke::new(1.0, GREEN);
        w.hovered.fg_stroke = Stroke::new(1.5, GREEN);
        w.hovered.corner_radius = r;
        w.active.bg_fill = Color32::from_rgb(0, 90, 60);
        w.active.weak_bg_fill = Color32::from_rgb(0, 90, 60);
        w.active.bg_stroke = Stroke::new(1.0, CYAN);
        w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.active.corner_radius = r;
        w.open.bg_fill = Color32::from_rgb(6, 30, 26);
        w.open.weak_bg_fill = Color32::from_rgb(6, 30, 26);
        w.open.bg_stroke = Stroke::new(1.0, CYAN);
        w.open.fg_stroke = Stroke::new(1.0, CYAN);
        w.open.corner_radius = r;
    });
}
