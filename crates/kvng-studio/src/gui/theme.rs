//! KVNG Studio's visual language: colour tokens, type, spacing and egui styling.
//!
//! Near-black surfaces with an energetic citrus-amber accent inspired by FL Studio.
//! Amber marks what is active, selected or playing; everything else stays quiet.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};

// Surfaces, darkest to lightest (authentic brushed gunmetal steel finish).
pub const BG: Color32 = Color32::from_rgb(0x13, 0x15, 0x1B);
pub const PANEL: Color32 = Color32::from_rgb(0x1D, 0x20, 0x29);
pub const PANEL_2: Color32 = Color32::from_rgb(0x25, 0x29, 0x35);
pub const RAISED: Color32 = Color32::from_rgb(0x2F, 0x35, 0x44);
pub const RAISED_HI: Color32 = Color32::from_rgb(0x3D, 0x45, 0x56);

// Metallic finish accents & specular highlights.
pub const METAL_FACE: Color32 = Color32::from_rgb(0x2D, 0x32, 0x40);
pub const METAL_FACE_HI: Color32 = Color32::from_rgb(0x42, 0x4A, 0x5C);
pub const METAL_SHINE: Color32 = Color32::from_rgb(0x65, 0x71, 0x8C);
pub const METAL_RIM: Color32 = Color32::from_rgb(0x4E, 0x57, 0x6E);
pub const METAL_RIM_DARK: Color32 = Color32::from_rgb(0x16, 0x18, 0x20);
pub const METAL_SHADOW: Color32 = Color32::from_rgb(0x0A, 0x0C, 0x10);

// Lines & milled bevel edges.
pub const LINE: Color32 = Color32::from_rgb(0x34, 0x3B, 0x4B);
pub const LINE_SOFT: Color32 = Color32::from_rgb(0x26, 0x2A, 0x36);
pub const LINE_STRONG: Color32 = Color32::from_rgb(0x53, 0x5D, 0x75);

// Text.
pub const TEXT: Color32 = Color32::from_rgb(0xEC, 0xEE, 0xF5);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xAF, 0xB3, 0xC4);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x82, 0x86, 0x99);

// Accent: FL Studio signature citrus-amber palette.
pub const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x76, 0x19);
pub const ACCENT_HOT: Color32 = Color32::from_rgb(0xFF, 0xA2, 0x4D);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x8A, 0x36, 0x0A);
pub const ACCENT_SUBTLE: Color32 = Color32::from_rgb(0x2E, 0x18, 0x0C);
pub const ACCENT_BORDER: Color32 = Color32::from_rgb(0xD9, 0x56, 0x10);
pub const ACCENT_GLOW: Color32 = Color32::from_rgba_premultiplied(0x55, 0x22, 0x06, 0x40);

// Hardware status indicators.
pub const STATUS_PLAY: Color32 = Color32::from_rgb(0x22, 0xC5, 0x5E);
pub const STATUS_PLAY_GLOW: Color32 = Color32::from_rgba_premultiplied(0x12, 0x45, 0x22, 0x40);
pub const STATUS_REC: Color32 = Color32::from_rgb(0xEF, 0x44, 0x44);
pub const STATUS_REC_GLOW: Color32 = Color32::from_rgba_premultiplied(0x55, 0x12, 0x18, 0x40);

// Containers and modular pills with milled bezel.
pub const PILL_BG: Color32 = Color32::from_rgb(0x19, 0x1C, 0x24);
pub const PILL_LINE: Color32 = Color32::from_rgb(0x33, 0x3A, 0x4A);

// Status.
pub const WARN: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x20);
pub const ERROR: Color32 = Color32::from_rgb(0xFF, 0x4D, 0x5E);

/// Gain reduction above this is "over-compressed" and shows red.
pub const GR_HEAVY_DB: f32 = 12.0;

/// Colour for an amount of gain reduction: amber (gentle) → orange → red (over-compressed).
pub fn gr_color(db: f32) -> Color32 {
    if db >= GR_HEAVY_DB {
        ERROR
    } else if db >= 6.0 {
        Color32::from_rgb(0xFF, 0x7A, 0x2A)
    } else {
        WARN
    }
}

/// Clip colours: vibrant studio hues pulled back so the accent still stands out.
pub const CLIP_PALETTE: [Color32; 8] = [
    Color32::from_rgb(0xF9, 0x73, 0x16), // tangerine orange
    Color32::from_rgb(0x8B, 0x5C, 0xF6), // violet
    Color32::from_rgb(0x22, 0xB0, 0xC8), // cyan
    Color32::from_rgb(0xE8, 0x9A, 0x1E), // amber
    Color32::from_rgb(0x6B, 0xC2, 0x2E), // lime
    Color32::from_rgb(0xF0, 0x6A, 0x5E), // coral
    Color32::from_rgb(0x4C, 0x8D, 0xF6), // blue
    Color32::from_rgb(0x1E, 0xB8, 0xA0), // teal
];

/// A stable colour for anything with a name (patterns, channels).
pub fn clip_color(key: &str) -> Color32 {
    let hash = key.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    CLIP_PALETTE[(hash as usize) % CLIP_PALETTE.len()]
}

/// `c` mixed toward `target` by `t` (0..=1).
pub fn mix(c: Color32, target: Color32, t: f32) -> Color32 {
    let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(l(c.r(), target.r()), l(c.g(), target.g()), l(c.b(), target.b()))
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

// Spacing and shape.
pub const RADIUS: CornerRadius = CornerRadius::same(4);
pub const RADIUS_SM: CornerRadius = CornerRadius::same(2);
pub const RADIUS_MD: CornerRadius = CornerRadius::same(6);
#[allow(dead_code)]
pub const RADIUS_LG: CornerRadius = CornerRadius::same(8);
pub const RADIUS_ZERO: CornerRadius = CornerRadius::ZERO;

const BRUSHED_METAL_PNG: &[u8] = include_bytes!("../../assets/brushed_dark_metal.png");

/// Decodes the embedded dark brushed metal texture into an `egui::ColorImage`.
pub fn decode_metal_texture() -> Option<egui::ColorImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(BRUSHED_METAL_PNG));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = info.color_type.samples();
    let mut pixels = Vec::with_capacity(w * h);

    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * channels;
            let (r, g, b, a) = match channels {
                4 => (buf[i], buf[i + 1], buf[i + 2], buf[i + 3]),
                3 => (buf[i], buf[i + 1], buf[i + 2], 255),
                _ => (buf[i], buf[i], buf[i], 255),
            };
            pixels.push(Color32::from_rgba_unmultiplied(r, g, b, a));
        }
    }
    Some(egui::ColorImage::new([w, h], pixels))
}

/// Fetches or caches the brushed dark metal texture in egui's context memory.
pub fn metal_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("kvng_studio_brushed_dark_metal_texture");
    let cached = ctx.data_mut(|d| d.get_temp::<egui::TextureHandle>(id));
    if let Some(handle) = cached {
        handle
    } else {
        let img = decode_metal_texture().expect("valid embedded brushed_dark_metal.png");
        let opts = egui::TextureOptions {
            wrap_mode: egui::TextureWrapMode::Repeat,
            ..egui::TextureOptions::LINEAR
        };
        let handle = ctx.load_texture("kvng_brushed_metal", img, opts);
        ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
        handle
    }
}

/// Draws an authentic brushed dark metal finish across a panel or window.
/// Uses the realistic hairline brushed metal texture, crisp dual-tone milled chamfers, and drop shadow.
pub fn draw_gunmetal_surface(painter: &egui::Painter, rect: egui::Rect, radius: CornerRadius, base_color: Color32) {
    // 1. Dark drop shadow
    painter.rect_filled(rect.translate(egui::vec2(0.0, 1.2)), radius, METAL_SHADOW);
    // 2. Base gunmetal sheet
    painter.rect_filled(rect, radius, base_color);

    // 3. Photo-realistic dark brushed metal texture with fine hairline grain
    let texture = metal_texture(painter.ctx());
    // For tall/narrow vertical strips (like mixer channels), sample continuous horizontal grain at 1:1 scale
    let uv = if rect.width() < 140.0 && rect.height() > 180.0 {
        let u0 = (rect.min.x / 740.0).fract().abs();
        let u1 = u0 + (rect.width() / 740.0);
        egui::Rect::from_min_max(egui::pos2(u0, 0.0), egui::pos2(u1, 1.0))
    } else {
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
    };
    painter.image(texture.id(), rect, uv, Color32::WHITE);

    // 4. Precision milled chamfer rim
    painter.rect_stroke(rect, radius, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);

    // 5. Crisp top specular edge (light catching the upper beveled lip)
    let top_highlight = egui::Rect::from_min_max(
        egui::pos2(rect.min.x + 2.0, rect.min.y + 0.8),
        egui::pos2(rect.max.x - 2.0, rect.min.y + 2.0),
    );
    painter.rect_filled(top_highlight, 1.0, alpha(Color32::WHITE, 0.14));

    // 6. Bottom deep bevel shadow
    let bottom_shadow = egui::Rect::from_min_max(
        egui::pos2(rect.min.x + 2.0, rect.max.y - 1.8),
        egui::pos2(rect.max.x - 2.0, rect.max.y - 0.6),
    );
    painter.rect_filled(bottom_shadow, 1.0, alpha(METAL_SHADOW, 0.35));
}

/// Draws an authentic machined hardware machine screw (slotted) into a gunmetal chassis.
pub fn draw_screw(painter: &egui::Painter, center: egui::Pos2, radius: f32, angle: f32) {
    // Recessed socket
    painter.circle_filled(center, radius + 1.2, METAL_SHADOW);
    painter.circle_stroke(center, radius + 1.2, Stroke::new(0.8, LINE_SOFT));
    // Screw head body
    painter.circle_filled(center, radius, METAL_FACE);
    painter.circle_stroke(center, radius, Stroke::new(0.8, METAL_RIM));
    // Top-left specular crescent
    let spec_c = center - egui::vec2(radius * 0.25, radius * 0.25);
    painter.circle_filled(spec_c, radius * 0.45, alpha(Color32::WHITE, 0.16));
    // Screw drive slot
    let dir = egui::vec2(angle.cos(), angle.sin());
    let slot_start = center - dir * (radius * 0.7);
    let slot_end = center + dir * (radius * 0.7);
    painter.line_segment([slot_start + egui::vec2(0.5, 0.5), slot_end + egui::vec2(0.5, 0.5)], Stroke::new(1.2, METAL_SHADOW));
    painter.line_segment([slot_start, slot_end], Stroke::new(1.2, METAL_RIM_DARK));
}

/// Draws an authentic hardware unit faceplate with dark brushed gunmetal texture and optional corner screws.
pub fn draw_hardware_faceplate(painter: &egui::Painter, rect: egui::Rect, radius: CornerRadius, base_color: Color32, screws: bool) {
    draw_gunmetal_surface(painter, rect, radius, base_color);
    if screws && rect.width() > 120.0 && rect.height() > 80.0 {
        let inset = 9.0;
        let r = 4.0;
        draw_screw(painter, egui::pos2(rect.min.x + inset, rect.min.y + inset), r, 0.4);
        draw_screw(painter, egui::pos2(rect.max.x - inset, rect.min.y + inset), r, 2.1);
        draw_screw(painter, egui::pos2(rect.min.x + inset, rect.max.y - inset), r, 1.3);
        draw_screw(painter, egui::pos2(rect.max.x - inset, rect.max.y - inset), r, 2.8);
    }
}

/// Draws a tactile metallic panel with top specular highlight and subtle shadow bevel.
#[allow(dead_code)]
pub fn draw_metal_panel(painter: &egui::Painter, rect: egui::Rect, radius: CornerRadius, base_color: Color32) {
    draw_gunmetal_surface(painter, rect, radius, base_color);
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn ui_font(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Shortens `text` with a trailing "…" so it fits in `max_width` at `font`.
pub fn elide(painter: &egui::Painter, text: &str, font: FontId, max_width: f32) -> String {
    let fits = |t: &str| painter.layout_no_wrap(t.to_string(), font.clone(), TEXT).size().x <= max_width;
    if fits(text) {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.pop().is_some() {
        let t: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
        if fits(&t) {
            return t;
        }
    }
    "…".into()
}

/// Installs fonts and the dark theme.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (TextStyle::Heading, ui_font(15.0)),
            (TextStyle::Body, ui_font(13.0)),
            (TextStyle::Button, ui_font(13.0)),
            (TextStyle::Small, ui_font(11.5)),
            (TextStyle::Monospace, mono(12.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
        style.spacing.interact_size.y = 22.0;
        style.spacing.window_margin = Margin::same(10);

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = PANEL;
        v.window_fill = PANEL_2;
        v.window_stroke = Stroke::new(1.0, LINE);
        v.window_corner_radius = CornerRadius::same(6);
        v.menu_corner_radius = CornerRadius::same(6);
        v.extreme_bg_color = BG;
        v.faint_bg_color = PANEL_2;
        v.code_bg_color = RAISED;
        v.override_text_color = None;
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = WARN;
        v.error_fg_color = ERROR;
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0, ACCENT);

        let w = &mut v.widgets;
        for (state, fill, stroke, text) in [
            (&mut w.noninteractive, PANEL, LINE_SOFT, TEXT_DIM),
            (&mut w.inactive, METAL_FACE, LINE, TEXT),
            (&mut w.hovered, METAL_FACE_HI, LINE_STRONG, TEXT),
            (&mut w.active, ACCENT_DIM, ACCENT, TEXT),
            (&mut w.open, METAL_FACE_HI, LINE_STRONG, TEXT),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.0, text);
            state.corner_radius = RADIUS;
            state.expansion = 0.0;
        }
    });
}

/// Uses the macOS system font (SF) when present, falling back to egui's defaults.
/// System fonts are read at runtime, never bundled.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, path, family) in [
        ("sf", "/System/Library/Fonts/SFNS.ttf", FontFamily::Proportional),
        ("sf-mono", "/System/Library/Fonts/SFNSMono.ttf", FontFamily::Monospace),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    }
    ctx.set_fonts(fonts);
}
