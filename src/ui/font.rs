//! Font rasterisation with fontdue, with a simple glyph cache.
//!
//! We rasterise glyphs on first use and cache them in a HashMap.
//! For typical Latin text this warms fully in the first keystroke.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

use crate::config::Config;

/// Rasterised glyph (8-bit coverage mask).
struct Glyph {
    width:   u32,
    height:  u32,
    x_off:   i32,
    y_off:   i32,
    advance: u32,
    bitmap:  Vec<u8>,
}

pub struct FontAtlas {
    font:        Font,
    size:        f32,
    line_height: u32,
    cache:       HashMap<char, Glyph>,
}

// Built-in fallback font: a minimal TTF included at compile time.
// We bundle Noto Sans Regular subset for Latin coverage.
const FALLBACK_FONT: &[u8] = include_bytes!("../../assets/Inter-Regular.ttf");

impl FontAtlas {
    pub fn load(cfg: &Config) -> Self {
        let font_data: Vec<u8> = if let Some(path) = &cfg.font.path {
            std::fs::read(path).unwrap_or_else(|_| FALLBACK_FONT.to_vec())
        } else {
            FALLBACK_FONT.to_vec()
        };

        let font = Font::from_bytes(font_data, FontSettings::default())
            .expect("invalid font data");

        let size = cfg.font.size;
        // Approximate line height: ascent + descent + a small leading.
        let line_height = if let Some(m) = font.horizontal_line_metrics(size) {
            (m.ascent - m.descent + 2.0).ceil() as u32
        } else {
            size.ceil() as u32 + 4
        };

        Self {
            font,
            size,
            line_height,
            cache: HashMap::with_capacity(128),
        }
    }

    pub fn line_height(&self) -> u32 {
        self.line_height
    }

    /// Returns the pixel width of a string.
    pub fn text_width(&self, text: &str) -> u32 {
        text.chars()
            .map(|ch| {
                let metrics = self.font.metrics(ch, self.size);
                metrics.advance_width.ceil() as u32
            })
            .sum()
    }

    /// Rasterise and blit a character into the pixel buffer.
    /// Returns the horizontal advance in pixels.
    pub fn blit_char(
        &mut self,
        pixels: &mut [u32],
        stride: usize,
        x: u32,
        y: u32,
        ch: char,
        color: u32,
    ) -> u32 {
        // Ensure glyph is in cache.
        if !self.cache.contains_key(&ch) {
            let (metrics, bitmap) = self.font.rasterize(ch, self.size);
            let glyph = Glyph {
                width:   metrics.width as u32,
                height:  metrics.height as u32,
                x_off:   metrics.xmin,
                y_off:   metrics.ymin,
                advance: metrics.advance_width.ceil() as u32,
                bitmap,
            };
            self.cache.insert(ch, glyph);
        }

        let glyph = self.cache.get(&ch).unwrap();
        let advance = glyph.advance;

        // Baseline: place the glyph such that the top of the capital letter
        // aligns with `y`.
        let metrics = self.font.metrics(ch, self.size);
        let ascent = self.font.horizontal_line_metrics(self.size)
            .map(|m| m.ascent)
            .unwrap_or(self.size * 0.8);
        let baseline_y = y as i32 + ascent.ceil() as i32;

        let gx = x as i32 + glyph.x_off;
        let gy = baseline_y - glyph.y_off - glyph.height as i32;

        // Blit coverage mask into pixel buffer with alpha-blending.
        let ch_r = (color >> 16) & 0xFF;
        let ch_g = (color >>  8) & 0xFF;
        let ch_b =  color        & 0xFF;

        for row in 0..glyph.height as i32 {
            for col in 0..glyph.width as i32 {
                let px = gx + col;
                let py = gy + row;
                if px < 0 || py < 0 { continue; }
                let px = px as usize;
                let py = py as usize;
                let idx = py * stride + px;
                if idx >= pixels.len() { continue; }

                let coverage = glyph.bitmap[(row as usize) * glyph.width as usize + col as usize];
                if coverage == 0 { continue; }

                let alpha = coverage as u32;
                let inv_a = 255 - alpha;
                let dst = pixels[idx];
                let r = (ch_r * alpha / 255) + (((dst >> 16) & 0xFF) * inv_a / 255);
                let g = (ch_g * alpha / 255) + (((dst >>  8) & 0xFF) * inv_a / 255);
                let b = (ch_b * alpha / 255) + (( dst         & 0xFF) * inv_a / 255);
                let a = alpha + ((dst >> 24) & 0xFF) * inv_a / 255;
                pixels[idx] = (a.min(255) << 24)
                    | (r.min(255) << 16)
                    | (g.min(255) <<  8)
                    |  b.min(255);
            }
        }

        let _ = metrics; // suppress unused warning
        advance
    }
}
