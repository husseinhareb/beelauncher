//! Primitive pixel drawing operations on a flat ARGB u32 buffer.

/// Fill a rectangle with a solid color.
#[inline]
pub fn fill_rect(
    pixels: &mut [u32],
    stride: usize,
    x: u32, y: u32,
    w: u32, h: u32,
    color: u32,
) {
    let x = x as usize;
    let y = y as usize;
    let w = w as usize;
    let h = h as usize;
    for row in y..(y + h) {
        let base = row * stride + x;
        let end  = (base + w).min(pixels.len());
        if base >= pixels.len() { break; }
        pixels[base..end].fill(color);
    }
}

/// Draw a 1-pixel or n-pixel rectangular border.
pub fn draw_border(
    pixels: &mut [u32],
    stride: usize,
    width: u32,
    height: u32,
    border_w: u32,
    color: u32,
) {
    let bw = border_w;
    // Top
    fill_rect(pixels, stride, 0, 0, width, bw, color);
    // Bottom
    fill_rect(pixels, stride, 0, height.saturating_sub(bw), width, bw, color);
    // Left
    fill_rect(pixels, stride, 0, 0, bw, height, color);
    // Right
    fill_rect(pixels, stride, width.saturating_sub(bw), 0, bw, height, color);
}

#[allow(dead_code)]
/// Alpha-blend a single premultiplied ARGB pixel over the destination.
#[inline]
pub fn blend(dst: u32, src: u32) -> u32 {
    let sa = (src >> 24) & 0xFF;
    if sa == 255 { return src; }
    if sa == 0   { return dst; }
    let inv_a = 255 - sa;
    let r = ((src >> 16) & 0xFF) + (((dst >> 16) & 0xFF) * inv_a / 255);
    let g = ((src >>  8) & 0xFF) + (((dst >>  8) & 0xFF) * inv_a / 255);
    let b = ( src        & 0xFF) + (( dst         & 0xFF) * inv_a / 255);
    let a = sa + ((dst >> 24) & 0xFF) * inv_a / 255;
    (a << 24) | (r.min(255) << 16) | (g.min(255) << 8) | b.min(255)
}
