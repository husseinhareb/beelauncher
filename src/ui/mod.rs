//! Software-rendered UI using a shared-memory Wayland buffer.
//!
//! Layout (top-to-bottom):
//!   ┌─────────────────────────────┐
//!   │  [input field]              │  input_height
//!   ├─────────────────────────────┤
//!   │  result row 0               │  row_height each
//!   │  result row 1 (selected)    │
//!   │  …                          │
//!   └─────────────────────────────┘
//!
//! All drawing is into a u32 ARGB pixel buffer.  Text is rasterised with
//! fontdue.  No GPU, no external compositor tricks.

mod font;
mod draw;

pub use font::FontAtlas;

use crate::{
    config::{Config, ThemeConfig},
    search,
};

/// Computed layout constants (pixel-space, logical).
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub width:        u32,
    pub input_height: u32,
    pub row_height:   u32,
    pub visible_rows: usize,
    pub total_height: u32,
}

impl Layout {
    pub fn new(cfg: &Config, atlas: &FontAtlas) -> Self {
        let line_h = atlas.line_height();
        let v_pad  = cfg.theme.row_padding * 2;
        let i_pad  = cfg.theme.input_padding * 2;
        let row_h  = line_h + v_pad;
        let inp_h  = line_h + i_pad;
        let rows   = cfg.window.max_entries;
        Self {
            width:        cfg.window.width,
            input_height: inp_h,
            row_height:   row_h,
            visible_rows: rows,
            total_height: inp_h + row_h * rows as u32,
        }
    }

    #[allow(dead_code)]
    pub fn buffer_len(&self) -> usize {
        (self.width * self.total_height) as usize
    }
}

/// Stateless renderer: writes a complete frame into `pixels`.
///
/// Called whenever state changes; the caller controls when to flush.
pub struct Renderer {
    pub layout: Layout,
    atlas:      FontAtlas,
    theme:      ThemeConfig,
}

impl Renderer {
    pub fn new(cfg: &Config) -> Self {
        let atlas = FontAtlas::load(cfg);
        let layout = Layout::new(cfg, &atlas);
        Self {
            layout,
            atlas,
            theme: cfg.theme.clone(),
        }
    }

    /// Full frame render.  `pixels` must be exactly `layout.buffer_len()` in length.
    pub fn render(
        &mut self,
        pixels:   &mut [u32],
        query:    &str,
        results:  &[search::Match<'_>],
        selected: usize,
        scroll:   usize,
    ) {
        let w = self.layout.width as usize;

        // Clear to background.
        draw::fill_rect(pixels, w, 0, 0, self.layout.width, self.layout.total_height,
            self.theme.background.to_argb_premul());

        // Input field background.
        draw::fill_rect(pixels, w, 0, 0, self.layout.width, self.layout.input_height,
            self.theme.input_bg.to_argb_premul());

        // Input text.
        let text_y = self.layout.input_height / 2 - self.atlas.line_height() / 2;
        let text_x = self.theme.input_padding;
        self.draw_text(pixels, w, text_x, text_y, query, self.theme.foreground.to_argb_premul());

        // Cursor line (1px wide, at end of text).
        let cursor_x = text_x + self.atlas.text_width(query);
        draw::fill_rect(pixels, w,
            cursor_x.min(self.layout.width.saturating_sub(2)), text_y,
            1, self.atlas.line_height(),
            self.theme.foreground.to_argb_premul());

        // Result rows.
        let visible = self.layout.visible_rows.min(results.len().saturating_sub(scroll));
        for i in 0..visible {
            let idx = scroll + i;
            let entry = results[idx].entry;
            let row_y = self.layout.input_height + i as u32 * self.layout.row_height;
            let is_sel = idx == selected;

            if is_sel {
                draw::fill_rect(pixels, w, 0, row_y, self.layout.width, self.layout.row_height,
                    self.theme.selection_bg.to_argb_premul());
            }

            let fg = if is_sel {
                self.theme.selection_fg.to_argb_premul()
            } else {
                self.theme.foreground.to_argb_premul()
            };

            let ty = row_y + self.layout.row_height / 2 - self.atlas.line_height() / 2;
            self.draw_text(pixels, w, text_x, ty, &entry.name, fg);
        }

        // Border.
        if self.theme.border_width > 0 {
            draw::draw_border(pixels, w, self.layout.width, self.layout.total_height,
                self.theme.border_width, self.theme.border.to_argb_premul());
        }
    }

    fn draw_text(&mut self, pixels: &mut [u32], stride: usize, x: u32, y: u32, text: &str, color: u32) {
        let mut cx = x;
        for ch in text.chars() {
            if cx >= self.layout.width { break; }
            let adv = self.atlas.blit_char(pixels, stride, cx, y, ch, color);
            cx += adv;
        }
    }
}
