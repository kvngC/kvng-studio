//! Draws KVNG Studio's app icon (1024×1024 PNG): a neon-pink K with a soft glow on
//! a near-black rounded square. `cargo run --release -p kvng-studio --example make_icon -- out.png`

const SIZE: usize = 1024;
const SS: usize = 3; // supersampling per axis

type Poly = &'static [(f32, f32)];

const BAR: Poly = &[(300.0, 250.0), (412.0, 250.0), (412.0, 774.0), (300.0, 774.0)];
const UPPER: Poly = &[(398.0, 548.0), (618.0, 250.0), (752.0, 250.0), (480.0, 616.0)];
const LOWER: Poly = &[(478.0, 470.0), (752.0, 774.0), (618.0, 774.0), (398.0, 600.0)];

fn inside(poly: Poly, x: f32, y: f32) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

fn in_k(x: f32, y: f32) -> bool {
    inside(BAR, x, y) || inside(UPPER, x, y) || inside(LOWER, x, y)
}

/// Coverage of the rounded-square background at a point (0 or 1).
fn in_squircle(x: f32, y: f32) -> bool {
    let (m, r) = (40.0, 220.0);
    let (lo, hi) = (m, SIZE as f32 - m);
    if x < lo || x > hi || y < lo || y > hi {
        return false;
    }
    let cx = x.clamp(lo + r, hi - r);
    let cy = y.clamp(lo + r, hi - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

fn box_blur(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut tmp = vec![0.0; w * h];
    let mut out = vec![0.0; w * h];
    let norm = 1.0 / (2 * radius + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for k in x.saturating_sub(radius)..(x + radius + 1).min(w) {
                acc += src[y * w + k];
            }
            tmp[y * w + x] = acc * norm;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for k in y.saturating_sub(radius)..(y + radius + 1).min(h) {
                acc += tmp[k * w + x];
            }
            out[y * w + x] = acc * norm;
        }
    }
    out
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "icon.png".into());
    let mut k_cov = vec![0f32; SIZE * SIZE];
    let mut bg_cov = vec![0f32; SIZE * SIZE];
    let step = 1.0 / SS as f32;
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (mut k, mut b) = (0, 0);
            for sy in 0..SS {
                for sx in 0..SS {
                    let (x, y) = (px as f32 + (sx as f32 + 0.5) * step, py as f32 + (sy as f32 + 0.5) * step);
                    k += in_k(x, y) as u32;
                    b += in_squircle(x, y) as u32;
                }
            }
            let n = (SS * SS) as f32;
            k_cov[py * SIZE + px] = k as f32 / n;
            bg_cov[py * SIZE + px] = b as f32 / n;
        }
    }
    // Glow: the K blurred three times (≈ gaussian).
    let mut glow = k_cov.clone();
    for _ in 0..3 {
        glow = box_blur(&glow, SIZE, SIZE, 18);
    }

    let bg_top = [0x1A as f32, 0x1A as f32, 0x22 as f32];
    let bg_bottom = [0x0B as f32, 0x0B as f32, 0x0F as f32];
    let pink = [0xFF as f32, 0x2D as f32, 0x95 as f32];
    let hot = [0xFF as f32, 0x8A as f32, 0xC4 as f32];
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for py in 0..SIZE {
        let t = py as f32 / SIZE as f32;
        for px in 0..SIZE {
            let i = py * SIZE + px;
            let mut c = [0.0f32; 3];
            for ch in 0..3 {
                c[ch] = bg_top[ch] + (bg_bottom[ch] - bg_top[ch]) * t;
            }
            let g = (glow[i] * 1.6).min(1.0) * 0.55;
            for ch in 0..3 {
                c[ch] += (pink[ch] - c[ch]) * g;
            }
            // The K: hot pink at the top fading to accent pink.
            let k = k_cov[i];
            for ch in 0..3 {
                let kc = hot[ch] + (pink[ch] - hot[ch]) * t.clamp(0.2, 0.8);
                c[ch] += (kc - c[ch]) * k;
            }
            let a = bg_cov[i];
            rgba.extend_from_slice(&[c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, (a * 255.0).round() as u8]);
        }
    }

    let file = std::fs::File::create(&out).expect("create icon file");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), SIZE as u32, SIZE as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    println!("wrote {out}");
}
