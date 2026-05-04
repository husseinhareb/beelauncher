//! XDG icon lookup with lazy loading and an in-process pixel cache.
//!
//! Lookup order (per XDG Icon Theme Specification):
//!   1. User's current GTK icon theme (read from gtk-4.0/gtk-3.0 settings)
//!   2. "hicolor" — the mandatory universal fallback theme
//!   3. /usr/share/pixmaps — legacy flat directory
//!
//! For each theme, sizes are tried nearest-first to the target pixel size.
//! PNG files are decoded with the `png` crate; SVG files with `resvg`.
//! All icons are scaled to exactly `size × size` pixels using bilinear
//! interpolation and stored as premultiplied ARGB u32 values.
//!
//! If nothing is found, a procedurally-generated fallback icon is used:
//! a rounded square tinted with a hue derived from the app name.

use std::{
    collections::HashMap,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use resvg::{tiny_skia, usvg};

// All XDG standard icon sizes, ordered for search by `sized_search_order()`.
const ALL_ICON_SIZES: &[u32] = &[16, 22, 24, 32, 48, 64, 128, 256, 512];

/// Build a search order for icon sizes relative to `target`.
/// Sizes ≥ target come first (ascending — smallest downscale wins);
/// then sizes < target (descending — largest upscale source wins).
/// This minimises scaling artifacts: we'd rather downscale a 64px icon
/// to 48px than upscale a 22px one.
fn sized_search_order(target: u32) -> Vec<u32> {
    let mut above: Vec<u32> = ALL_ICON_SIZES.iter().copied().filter(|&s| s >= target).collect();
    let mut below: Vec<u32> = ALL_ICON_SIZES.iter().copied().filter(|&s| s < target).collect();
    above.sort_unstable();
    below.sort_unstable_by(|a, b| b.cmp(a)); // descending — closest to target first
    above.extend(below);
    above
}

pub struct IconCache {
    /// icon_name → decoded premultiplied ARGB pixels at `size × size`.
    /// `None` = lookup done, nothing found → use fallback.
    resolved: HashMap<String, Option<Vec<u32>>>,
    pub size:     u32,
    fallback: Vec<u32>,
    search_bases: Vec<PathBuf>,
    theme:        String,
}

impl IconCache {
    pub fn new(size: u32) -> Self {
        let search_bases = icon_search_bases();
        let theme        = detect_gtk_theme(&search_bases);
        Self {
            resolved: HashMap::with_capacity(256),
            size,
            fallback: make_fallback(size),
            search_bases,
            theme,
        }
    }

    /// Blit the icon for `icon_name` into the pixel buffer at (x, y).
    /// Loads and caches on first call per icon name.
    pub fn blit(
        &mut self,
        pixels: &mut [u32],
        stride: usize,
        x: u32,
        y: u32,
        icon_name: &str,
    ) {
        // Lazily resolve and cache.
        if !self.resolved.contains_key(icon_name) {
            let pixels_opt = self.resolve(icon_name);
            self.resolved.insert(icon_name.to_owned(), pixels_opt);
        }

        let src = match self.resolved.get(icon_name).and_then(|o| o.as_ref()) {
            Some(p) => p.as_slice(),
            None    => self.fallback.as_slice(),
        };

        blit_argb(pixels, stride, x, y, self.size, src);
    }

    // ── Resolution pipeline ───────────────────────────────────────────────────

    fn resolve(&self, icon_name: &str) -> Option<Vec<u32>> {
        log::debug!("icon resolve: {:?}  theme={:?}  size={}", icon_name, self.theme, self.size);

        // If the field is an absolute path, load it directly.
        let p = Path::new(icon_name);
        if p.is_absolute() {
            if p.exists() {
                log::debug!("icon resolve: absolute path exists, loading");
                return load_image_file(p, self.size);
            } else {
                log::debug!("icon resolve: absolute path does not exist");
                return None;
            }
        }

        // Only strip the extension when it is a known image format suffix.
        // Reverse-domain names like "org.xfce.thunar" must NOT be stripped —
        // file_stem() would turn them into "org.xfce" which matches nothing.
        let bare = match p.extension().and_then(|e| e.to_str()) {
            Some("png") | Some("svg") | Some("xpm") | Some("jpg") | Some("jpeg") => {
                let s = p.file_stem().and_then(|s| s.to_str()).unwrap_or(icon_name);
                log::debug!("icon resolve: stripped extension -> bare={:?}", s);
                s
            }
            _ => {
                log::debug!("icon resolve: no image extension -> bare={:?}", icon_name);
                icon_name
            }
        };

        // Theme search, then pixmaps, then flat base-dir fallback.
        for theme_name in [self.theme.as_str(), "hicolor"] {
            log::debug!("icon resolve: searching theme {:?} for {:?}", theme_name, bare);
            if let Some(pix) = self.find_in_theme(theme_name, bare) {
                log::debug!("icon resolve: FOUND in theme {:?}", theme_name);
                return Some(pix);
            }
        }
        log::debug!("icon resolve: not in themes, trying pixmaps");
        if let Some(pix) = self.find_in_pixmaps(bare) {
            return Some(pix);
        }
        // Some packages (e.g. Zed) drop a PNG directly in the icon base dir
        // (/usr/share/icons/name.png) rather than inside a theme/size tree.
        log::debug!("icon resolve: trying flat base dirs");
        if let Some(pix) = self.find_in_base_dirs(bare) {
            return Some(pix);
        }
        log::debug!("icon resolve: NOT FOUND anywhere for {:?}, using fallback", icon_name);
        None
    }

    fn find_in_theme(&self, theme: &str, name: &str) -> Option<Vec<u32>> {
        let search_sizes = sized_search_order(self.size);

        for base in &self.search_bases {
            let theme_dir = base.join(theme);
            if !theme_dir.is_dir() {
                log::debug!("  theme dir missing: {:?}", theme_dir);
                continue;
            }
            log::debug!("  searching theme dir: {:?}", theme_dir);

            // SVG scalable takes priority — it renders crisply at any size.
            for subdir in ["scalable/apps", "scalable"] {
                let dir = theme_dir.join(subdir);
                if let Some(pix) = find_icon_file(&dir, name, self.size) {
                    return Some(pix);
                }
            }

            // Sized directories, searched nearest-to-target first to minimise
            // scaling artifacts (prefer downscale over upscale).
            for &sz in &search_sizes {
                for subdir in ["apps", ""] {
                    let dir = if subdir.is_empty() {
                        theme_dir.join(format!("{sz}x{sz}"))
                    } else {
                        theme_dir.join(format!("{sz}x{sz}")).join(subdir)
                    };
                    if let Some(pix) = find_icon_file(&dir, name, self.size) {
                        return Some(pix);
                    }
                }
            }
        }
        None
    }

    fn find_in_pixmaps(&self, name: &str) -> Option<Vec<u32>> {
        let dir = PathBuf::from("/usr/share/pixmaps");
        find_icon_file(&dir, name, self.size)
    }

    /// Flat fallback: some packages place icons directly under a search base
    /// (e.g. /usr/share/icons/zed.png) rather than inside a theme/size tree.
    fn find_in_base_dirs(&self, name: &str) -> Option<Vec<u32>> {
        for base in &self.search_bases {
            if let Some(pix) = find_icon_file(base, name, self.size) {
                return Some(pix);
            }
        }
        None
    }
}

// ── File discovery ────────────────────────────────────────────────────────────

fn find_icon_file(dir: &Path, name: &str, target_size: u32) -> Option<Vec<u32>> {
    if !dir.is_dir() { return None; }

    // Prefer PNG, then SVG, then XPM (we skip XPM — obsolete format).
    for ext in ["png", "svg"] {
        let path = dir.join(format!("{name}.{ext}"));
        log::debug!("    checking: {:?}  exists={}", path, path.exists());
        if path.exists() {
            log::debug!("    loading: {:?}", path);
            let result = load_image_file(&path, target_size);
            if result.is_none() {
                log::debug!("    load_image_file returned None for {:?}", path);
            } else {
                log::debug!("    load_image_file OK for {:?}", path);
            }
            return result;
        }
    }
    None
}

fn load_image_file(path: &Path, target_size: u32) -> Option<Vec<u32>> {
    let data = fs::read(path).ok()?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => decode_png(&data, target_size),
        Some("svg") => decode_svg(&data, target_size),
        _           => None,
    }
}

// ── PNG decoding ──────────────────────────────────────────────────────────────

fn decode_png(data: &[u8], target: u32) -> Option<Vec<u32>> {
    let mut decoder = png::Decoder::new(Cursor::new(data));
    // Expand palette/grayscale to RGB/RGBA; strip 16-bit to 8-bit.
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16,
    );
    let mut reader = decoder.read_info().ok()?;

    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;

    log::debug!("    decode_png: {}x{} {:?} -> target {}", info.width, info.height, info.color_type, target);

    let w = info.width;
    let h = info.height;

    // Convert to premultiplied ARGB u32.
    let argb: Vec<u32> = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()]
            .chunks_exact(4)
            .map(|p| rgba_to_argb_premul(p[0], p[1], p[2], p[3]))
            .collect(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .chunks_exact(3)
            .map(|p| rgba_to_argb_premul(p[0], p[1], p[2], 255))
            .collect(),
        // Grayscale (after EXPAND, grayscale-alpha → RGBA, plain grayscale → RGB)
        png::ColorType::GrayscaleAlpha => buf[..info.buffer_size()]
            .chunks_exact(2)
            .map(|p| rgba_to_argb_premul(p[0], p[0], p[0], p[1]))
            .collect(),
        png::ColorType::Grayscale => buf[..info.buffer_size()]
            .iter()
            .map(|&v| rgba_to_argb_premul(v, v, v, 255))
            .collect(),
        _ => return None,
    };

    if w == target && h == target {
        Some(argb)
    } else {
        Some(scale_bilinear(&argb, w, h, target, target))
    }
}

// ── SVG decoding ──────────────────────────────────────────────────────────────

fn decode_svg(data: &[u8], target: u32) -> Option<Vec<u32>> {
    let opt  = usvg::Options::default();
    let tree = usvg::Tree::from_data(data, &opt).ok()?;

    let mut pixmap = tiny_skia::Pixmap::new(target, target)?;
    let sx = target as f32 / tree.size().width();
    let sy = target as f32 / tree.size().height();

    // resvg 0.44: render takes &usvg::Tree directly.
    resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());

    // tiny_skia stores pixels as PREMULTIPLIED RGBA — channels are already scaled
    // by alpha.  Premultiplying again would square the values and make everything
    // near-black.  We only reorder the bytes from RGBA to ARGB.
    let argb = pixmap
        .data()
        .chunks_exact(4)
        .map(|p| ((p[3] as u32) << 24) | ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | (p[2] as u32))
        .collect();
    Some(argb)
}

// ── Scaling ───────────────────────────────────────────────────────────────────

/// Bilinear downscale of premultiplied ARGB pixels.
/// Bilinear is correct for premultiplied data and significantly better than
/// nearest-neighbour for non-integer scale factors.
fn scale_bilinear(src: &[u32], sw: u32, sh: u32, tw: u32, th: u32) -> Vec<u32> {
    if sw == tw && sh == th {
        return src.to_vec();
    }

    let mut dst = vec![0u32; (tw * th) as usize];
    let x_ratio = sw as f32 / tw as f32;
    let y_ratio = sh as f32 / th as f32;

    for dy in 0..th {
        let fy = dy as f32 * y_ratio;
        let sy0 = (fy as u32).min(sh - 1);
        let sy1 = (sy0 + 1).min(sh - 1);
        let yt  = fy - sy0 as f32;

        for dx in 0..tw {
            let fx = dx as f32 * x_ratio;
            let sx0 = (fx as u32).min(sw - 1);
            let sx1 = (sx0 + 1).min(sw - 1);
            let xt  = fx - sx0 as f32;

            let p00 = src[(sy0 * sw + sx0) as usize];
            let p10 = src[(sy0 * sw + sx1) as usize];
            let p01 = src[(sy1 * sw + sx0) as usize];
            let p11 = src[(sy1 * sw + sx1) as usize];

            dst[(dy * tw + dx) as usize] = bilerp(p00, p10, p01, p11, xt, yt);
        }
    }
    dst
}

/// Bilinear interpolation of four ARGB pixels.
#[inline]
fn bilerp(p00: u32, p10: u32, p01: u32, p11: u32, tx: f32, ty: f32) -> u32 {
    let lerp_chan = |a: u32, b: u32, c: u32, d: u32| -> u32 {
        let top    = a as f32 + (b as f32 - a as f32) * tx;
        let bottom = c as f32 + (d as f32 - c as f32) * tx;
        (top + (bottom - top) * ty).round() as u32 & 0xFF
    };

    let a = lerp_chan(p00 >> 24, p10 >> 24, p01 >> 24, p11 >> 24);
    let r = lerp_chan((p00 >> 16) & 0xFF, (p10 >> 16) & 0xFF, (p01 >> 16) & 0xFF, (p11 >> 16) & 0xFF);
    let g = lerp_chan((p00 >>  8) & 0xFF, (p10 >>  8) & 0xFF, (p01 >>  8) & 0xFF, (p11 >>  8) & 0xFF);
    let b = lerp_chan(p00 & 0xFF, p10 & 0xFF, p01 & 0xFF, p11 & 0xFF);

    (a << 24) | (r << 16) | (g << 8) | b
}

// ── Blit ─────────────────────────────────────────────────────────────────────

/// Alpha-blit a size×size premultiplied ARGB icon onto the destination buffer.
fn blit_argb(dst: &mut [u32], stride: usize, x: u32, y: u32, size: u32, src: &[u32]) {
    for row in 0..size {
        let dst_base = (y + row) as usize * stride + x as usize;
        let src_base = (row * size) as usize;
        let dst_end  = (dst_base + size as usize).min(dst.len());
        let copy_w   = dst_end.saturating_sub(dst_base);
        if copy_w == 0 { break; }

        let src_row = &src[src_base..src_base + copy_w];
        let dst_row = &mut dst[dst_base..dst_end];
        for (d, &s) in dst_row.iter_mut().zip(src_row.iter()) {
            *d = blend(*d, s);
        }
    }
}

// ── Colour helpers ────────────────────────────────────────────────────────────

#[inline]
fn rgba_to_argb_premul(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let a32 = a as u32;
    let r32 = (r as u32 * a32 / 255) & 0xFF;
    let g32 = (g as u32 * a32 / 255) & 0xFF;
    let b32 = (b as u32 * a32 / 255) & 0xFF;
    (a32 << 24) | (r32 << 16) | (g32 << 8) | b32
}

/// Porter-Duff src-over on premultiplied ARGB.
#[inline]
fn blend(dst: u32, src: u32) -> u32 {
    let sa = (src >> 24) & 0xFF;
    if sa == 255 { return src; }
    if sa == 0   { return dst; }
    let inv = 255 - sa;
    let r = ((src >> 16) & 0xFF) + (((dst >> 16) & 0xFF) * inv / 255);
    let g = ((src >>  8) & 0xFF) + (((dst >>  8) & 0xFF) * inv / 255);
    let b = ( src        & 0xFF) + (( dst         & 0xFF) * inv / 255);
    let a = sa + ((dst >> 24) & 0xFF) * inv / 255;
    (a.min(255) << 24) | (r.min(255) << 16) | (g.min(255) << 8) | b.min(255)
}

// ── Fallback icon (procedural) ────────────────────────────────────────────────

/// Generate a solid rounded-square icon whose colour is derived from the icon
/// name so that each app gets a consistent, distinct colour without any file I/O.
fn make_fallback(size: u32) -> Vec<u32> {
    // A neutral dark square — used when icon_name is unknown at construction time.
    // Individual calls get per-name tinted versions via `make_tinted_fallback`.
    vec![rgba_to_argb_premul(0x45, 0x47, 0x5A, 0xFF); (size * size) as usize]
}

/// Per-name tinted fallback (called from IconCache when lookup returns None).
/// Exposed for future use in blit; currently IconCache uses the single fallback.
#[allow(dead_code)]
pub fn make_tinted_fallback(name: &str, size: u32) -> Vec<u32> {
    // Hash the name to a hue in [0, 360).
    let hash: u32 = name.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let hue = (hash % 360) as f32;
    let (r, g, b) = hsl_to_rgb(hue, 0.45, 0.35);
    let color = rgba_to_argb_premul(r, g, b, 0xFF);
    let radius = (size / 5) as i32;
    let s = size as i32;
    (0..size * size)
        .map(|i| {
            let px = (i % size) as i32;
            let py = (i / size) as i32;
            if in_rounded_rect(px, py, s, s, radius) { color } else { 0 }
        })
        .collect()
}

fn in_rounded_rect(px: i32, py: i32, w: i32, h: i32, r: i32) -> bool {
    let cx = px.clamp(r, w - r - 1);
    let cy = py.clamp(r, h - r - 1);
    let dx = px - cx;
    let dy = py - cy;
    dx * dx + dy * dy <= r * r
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if h < 60.0 { (c, x, 0.0) }
        else if h < 120.0 { (x, c, 0.0) }
        else if h < 180.0 { (0.0, c, x) }
        else if h < 240.0 { (0.0, x, c) }
        else if h < 300.0 { (x, 0.0, c) }
        else              { (c, 0.0, x) };
    (((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8)
}

// ── XDG environment helpers ───────────────────────────────────────────────────

fn icon_search_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Some(d) = dirs::data_local_dir() {
        bases.push(d.join("icons"));
    }
    bases.push(PathBuf::from("/usr/share/icons"));
    // Some distributions also put icons under /usr/local/share.
    bases.push(PathBuf::from("/usr/local/share/icons"));
    bases
}

/// Detect the user's GTK icon theme name by parsing their GTK settings file.
/// Falls back to "hicolor" (always present) if the file is missing or unparseable.
fn detect_gtk_theme(bases: &[PathBuf]) -> String {
    if let Some(theme) = read_gtk_theme() {
        // Verify the theme actually exists on disk before trusting it.
        let found = bases.iter().any(|b| b.join(&theme).is_dir());
        if found { return theme; }
    }
    "hicolor".to_owned()
}

fn read_gtk_theme() -> Option<String> {
    let config = dirs::config_dir()?;
    for file in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
        let path = config.join(file);
        if let Ok(content) = fs::read_to_string(&path) {
            for line in content.lines() {
                // Handle both `gtk-icon-theme-name=X` and `gtk-icon-theme-name = X`.
                if let Some(rest) = line.trim().strip_prefix("gtk-icon-theme-name") {
                    let val = rest.trim_start_matches([' ', '=']).trim();
                    if !val.is_empty() {
                        return Some(val.to_owned());
                    }
                }
            }
        }
    }
    None
}
