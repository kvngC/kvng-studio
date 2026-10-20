//! KVNG Studio 3D splash screen and logo asset manager.

use std::time::Instant;

use eframe::egui::{self, Align2, Color32, ColorImage, Rect, TextureHandle, Ui, Vec2, pos2, vec2};

use super::theme;

const LOGO_PNG: &[u8] = include_bytes!("../../assets/logo.png");

/// Decodes the embedded transparent PNG logo into an `egui::ColorImage`.
pub fn decode_logo_image() -> Option<ColorImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(LOGO_PNG));
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
                2 => (buf[i], buf[i], buf[i], buf[i + 1]),
                _ => (buf[i], buf[i], buf[i], 255),
            };
            pixels.push(Color32::from_rgba_unmultiplied(r, g, b, a));
        }
    }
    Some(ColorImage::new([w, h], pixels))
}

/// Fetches or caches the 3D logo texture in egui's context memory.
pub fn logo_texture(ctx: &egui::Context) -> TextureHandle {
    let id = egui::Id::new("kvng_studio_logo_texture_handle");
    let cached = ctx.data_mut(|d| d.get_temp::<TextureHandle>(id));
    if let Some(handle) = cached {
        handle
    } else {
        let img = decode_logo_image().expect("valid embedded logo.png");
        let handle = ctx.load_texture("kvng_logo", img, egui::TextureOptions::LINEAR);
        ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
        handle
    }
}

/// Size of the borderless, transparent window the app opens as while the splash plays.
pub const SPLASH_WINDOW: f32 = 440.0;
/// Size the main window opens at once the splash is over.
const MAIN_WINDOW: Vec2 = vec2(1440.0, 900.0);
const DURATION: f32 = 3.0;
/// Fade-in at the start and fade-out at the end of the splash, in seconds.
const FADE_IN: f32 = 0.35;
const FADE_OUT: f32 = 0.6;

/// Startup splash screen state.
pub struct Splash {
    pub started: Instant,
    pub dismissed: bool,
    /// Whether the window has been turned back into the normal decorated main window.
    restored: bool,
    /// Whether the first splash frame has run (sized/centered the window and started the clock).
    shown: bool,
}

impl Splash {
    /// `enabled = false` skips the splash entirely (the window is created as the main window).
    pub fn new(enabled: bool) -> Self {
        Self { started: Instant::now(), dismissed: !enabled, restored: !enabled, shown: false }
    }

    pub fn is_active(&self) -> bool {
        !self.dismissed && self.started.elapsed().as_secs_f32() < DURATION
    }

    /// The window is still the transparent splash window (clear to transparent, draw nothing else).
    pub fn is_window(&self) -> bool {
        !self.restored
    }
}

/// Renders the FL-style startup splash: just the transparent logo PNG floating on the desktop
/// (the window itself is borderless and transparent), with a smooth ease-out zoom and fade.
/// When it ends, the window becomes the normal decorated main window.
pub fn show_splash(ctx: &egui::Context, splash: &mut Splash) {
    if !splash.shown {
        splash.shown = true;
        // Time the splash from when it first appears, not from app start.
        splash.started = Instant::now();
        // An older session's saved window position may have been applied; center on the screen.
        if let Some(m) = ctx.input(|i| i.viewport().monitor_size) {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::splat(SPLASH_WINDOW)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(((m - Vec2::splat(SPLASH_WINDOW)) * 0.5).to_pos2()));
        }
    }
    if splash.is_active() {
        draw_logo(ctx, splash);
    }
    if !splash.is_active() && !splash.restored {
        splash.dismissed = true;
        splash.restored = true;
        restore_main_window(ctx);
    }
}

fn draw_logo(ctx: &egui::Context, splash: &mut Splash) {
    let elapsed = splash.started.elapsed().as_secs_f32();
    let t = (elapsed / DURATION).clamp(0.0, 1.0);

    // Cubic ease-out zoom from 0.90 to 1.06 across the whole splash.
    let zoom_ease = 1.0 - (1.0 - t).powi(3);
    let scale = 0.90 + 0.16 * zoom_ease;

    // Quadratic ease-out fade in, quadratic fade out at the end.
    let fade_in = (elapsed / FADE_IN).clamp(0.0, 1.0);
    let fade_in = 1.0 - (1.0 - fade_in) * (1.0 - fade_in);
    let fade_out = ((DURATION - elapsed) / FADE_OUT).clamp(0.0, 1.0);
    let alpha = (fade_in * fade_out * fade_out).clamp(0.0, 1.0);

    let logo = logo_texture(ctx);
    let win = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()).unwrap_or(Vec2::splat(SPLASH_WINDOW)));

    egui::Area::new(egui::Id::new("startup_splash"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(0.0, 0.0))
        .show(ctx, |ui| {
            // Invisible hit area: click (or Esc / Space) skips the splash.
            let (rect, resp) = ui.allocate_exact_size(win, egui::Sense::click());
            if resp.clicked() || ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Space)) {
                splash.dismissed = true;
                return;
            }
            // No background at all — only the logo.
            let size = (SPLASH_WINDOW - 40.0) * scale;
            let logo_rect = Rect::from_center_size(rect.center(), Vec2::splat(size));
            ui.painter().image(logo.id(), logo_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), theme::alpha(Color32::WHITE, alpha));
        });

    ctx.request_repaint();
}

/// Turns the borderless transparent splash window into the normal, decorated main window,
/// centered on the screen.
fn restore_main_window(ctx: &egui::Context) {
    use egui::ViewportCommand as Cmd;
    let monitor = ctx.input(|i| i.viewport().monitor_size);
    let size = match monitor {
        Some(m) => vec2(MAIN_WINDOW.x.min(m.x - 40.0), MAIN_WINDOW.y.min(m.y - 80.0)),
        None => MAIN_WINDOW,
    };
    ctx.send_viewport_cmd(Cmd::WindowLevel(egui::WindowLevel::Normal));
    ctx.send_viewport_cmd(Cmd::Decorations(true));
    ctx.send_viewport_cmd(Cmd::Transparent(false));
    ctx.send_viewport_cmd(Cmd::Resizable(true));
    ctx.send_viewport_cmd(Cmd::InnerSize(size));
    ctx.send_viewport_cmd(Cmd::MinInnerSize(vec2(960.0, 600.0)));
    if let Some(m) = monitor {
        ctx.send_viewport_cmd(Cmd::OuterPosition(pos2(((m.x - size.x) * 0.5).max(0.0), ((m.y - size.y) * 0.5).max(25.0))));
    }
    ctx.send_viewport_cmd(Cmd::Focus);
    ctx.request_repaint();
}

/// Renders the clean loading splash screen when opening a project.
/// No rectangular card frame or modal window — just the floating logo with smooth breathing pulse,
/// project title, status, and sleek amber progress bar on the dark studio backdrop.
pub fn draw_project_loading(ui: &mut Ui, project_name: &str, status: &str, elapsed: f32) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, theme::BG);

    let logo = logo_texture(ui.ctx());

    // Gentle floating breathing pulse
    let pulse = 1.0 + 0.02 * (elapsed * 2.5).sin();
    let logo_size = 230.0 * pulse;
    let logo_center = pos2(rect.center().x, rect.center().y - 45.0);
    let logo_rect = Rect::from_center_size(logo_center, Vec2::splat(logo_size));

    // Transparent logo emblem (no background, no box)
    painter.image(logo.id(), logo_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

    // Project Name Title
    painter.text(
        pos2(rect.center().x, logo_center.y + 145.0),
        Align2::CENTER_CENTER,
        format!("Loading \"{project_name}\""),
        theme::ui_font(18.0),
        theme::TEXT,
    );

    // Current Action / Status
    painter.text(
        pos2(rect.center().x, logo_center.y + 172.0),
        Align2::CENTER_CENTER,
        status,
        theme::ui_font(12.0),
        theme::ACCENT_HOT,
    );

    // Sleek progress bar (no card)
    let bar_rect = Rect::from_center_size(pos2(rect.center().x, logo_center.y + 200.0), vec2(320.0, 3.0));
    painter.rect_filled(bar_rect, 1.5, theme::RAISED);

    let t = (elapsed * 0.85).fract();
    let seg_w = 90.0;
    let x0 = bar_rect.min.x + (bar_rect.width() + seg_w) * t - seg_w;
    let seg = Rect::from_min_max(pos2(x0.max(bar_rect.min.x), bar_rect.min.y), pos2((x0 + seg_w).min(bar_rect.max.x), bar_rect.max.y));
    if seg.width() > 0.0 {
        painter.rect_filled(seg, 1.5, theme::ACCENT);
    }
}
