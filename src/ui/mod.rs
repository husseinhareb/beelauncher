//! Software-rendered UI using a shared-memory Wayland buffer.
//!
//! Layout (top-to-bottom), with icons enabled:
//!   ┌─────────────────────────────────┐
//!   │  [input field — no icon]        │  input_height
//!   ├─────────────────────────────────┤
//!   │ [icon] App Name                 │  row_height each
//!   │ [icon] App Name (selected)      │
//!   └─────────────────────────────────┘
//!
//! All drawing is into a u32 premultiplied-ARGB pixel buffer.

mod draw;
mod font;
pub mod icons;

pub use font::FontAtlas;

use crate::{
    config::{Config, ThemeConfig},
    search,
};
use icons::IconCache;

const ICON_GAP: u32 = 6;

/// Computed layout constants derived once from config + font metrics.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub width:          u32,
    pub input_height:   u32,
    pub row_height:     u32,
    pub visible_rows:   usize,
    pub total_height:   u32,
    /// Pixel size of icons (square).  0 when icons are disabled.
    pub icon_size:      u32,
    /// Total width reserved for the icon column (icon_size + gap), 0 if disabled.
    pub icon_col_width: u32,
}

impl Layout {
    pub fn new(cfg: &Config, atlas: &FontAtlas, show_icons: bool) -> Self {
        let line_h = atlas.line_height();
        let v_pad  = cfg.theme.row_padding * 2;
        let i_pad  = cfg.theme.input_padding * 2;
        let inp_h  = line_h + i_pad;
        let rows   = cfg.window.max_entries;

        let (icon_size, icon_col_width) = if show_icons {
            // Use configured size or snap to the nearest XDG standard for the font height.
            let sz = cfg.window.icon_size.unwrap_or_else(|| nearest_icon_size(line_h));
            (sz, sz + ICON_GAP)
        } else {
            (0, 0)
        };

        // Row must fit both the text line and the icon (whichever is taller).
        let row_h = line_h.max(icon_size) + v_pad;

        Self {
            width:          cfg.window.width,
            input_height:   inp_h,
            row_height:     row_h,
            visible_rows:   rows,
            total_height:   inp_h + row_h * rows as u32,
            icon_size,
            icon_col_width,
        }
    }

    #[allow(dead_code)]
    pub fn buffer_len(&self) -> usize {
        (self.width * self.total_height) as usize
    }
}

/// Snap `px` to the nearest XDG standard icon size.
fn nearest_icon_size(px: u32) -> u32 {
    const SIZES: &[u32] = &[16, 22, 24, 32, 48, 64];
    *SIZES.iter().min_by_key(|&&s| s.abs_diff(px)).unwrap_or(&24)
}

/// Stateful renderer: manages the font glyph cache and, optionally, the icon cache.
pub struct Renderer {
    pub layout: Layout,
    atlas:      FontAtlas,
    theme:      ThemeConfig,
    icons:      Option<IconCache>,
}

impl Renderer {
    pub fn new(cfg: &Config, show_icons: bool) -> Self {
        let atlas  = FontAtlas::load(cfg);
        let layout = Layout::new(cfg, &atlas, show_icons);
        let icons  = show_icons.then(|| IconCache::new(layout.icon_size));
        Self {
            layout,
            atlas,
            theme: cfg.theme.clone(),
            icons,
        }
    }

    /// Render a complete frame into `pixels` (must be `layout.buffer_len()` long).
    pub fn render(
        &mut self,
        pixels:   &mut [u32],
        query:    &str,
        results:  &[search::Match<'_>],
        selected: usize,
        scroll:   usize,
    ) {
        let stride = self.layout.width as usize;
        let bg     = self.theme.background.to_argb_premul();
        let inp_bg = self.theme.input_bg.to_argb_premul();
        let fg     = self.theme.foreground.to_argb_premul();
        let sel_bg = self.theme.selection_bg.to_argb_premul();
        let sel_fg = self.theme.selection_fg.to_argb_premul();
        let bdr    = self.theme.border.to_argb_premul();

        // Background clear.
        draw::fill_rect(pixels, stride, 0, 0, self.layout.width, self.layout.total_height, bg);

        // Input field.
        draw::fill_rect(pixels, stride, 0, 0, self.layout.width, self.layout.input_height, inp_bg);

        let text_y   = self.layout.input_height / 2 - self.atlas.line_height() / 2;
        let text_x   = self.theme.input_padding;
        self.draw_text(pixels, stride, text_x, text_y, query, fg);

        let cursor_x = text_x + self.atlas.text_width(query);
        draw::fill_rect(pixels, stride,
            cursor_x.min(self.layout.width.saturating_sub(2)), text_y,
            1, self.atlas.line_height(), fg);

        // Result rows.
        let visible = self.layout.visible_rows.min(results.len().saturating_sub(scroll));
        for i in 0..visible {
            let idx   = scroll + i;
            let entry = results[idx].entry;
            let row_y = self.layout.input_height + i as u32 * self.layout.row_height;
            let is_sel = idx == selected;

            if is_sel {
                draw::fill_rect(pixels, stride, 0, row_y,
                    self.layout.width, self.layout.row_height, sel_bg);
            }

            let row_fg = if is_sel { sel_fg } else { fg };
            let ty = row_y + self.layout.row_height / 2 - self.atlas.line_height() / 2;

            // Icon (if enabled).
            if let Some(icons) = &mut self.icons {
                let icon_name = entry.icon.as_deref().unwrap_or(&entry.name);
                let icon_size = self.layout.icon_size;
                let icon_x = text_x;
                let icon_y = row_y + self.layout.row_height.saturating_sub(icon_size) / 2;
                icons.blit(pixels, stride, icon_x, icon_y, icon_name);
            }

            let name_x = text_x + self.layout.icon_col_width;
            self.draw_text(pixels, stride, name_x, ty, &entry.name, row_fg);
        }

        // Border.
        if self.theme.border_width > 0 {
            draw::draw_border(pixels, stride,
                self.layout.width, self.layout.total_height,
                self.theme.border_width, bdr);
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
