//! Meal-photo rendering as truecolor half-block cells: each text cell packs
//! two vertical pixels (upper via foreground, lower via background of `▀`).

use image::imageops::FilterType;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Decode `data`, downscale to `cols × (rows*2)` pixels, and return one
/// [`Line`] per text row using the upper-half-block technique. Returns an
/// empty vec if the image cannot be decoded.
pub fn render_image_halfblocks(data: &[u8], cols: u16, rows: u16) -> Vec<Line<'static>> {
    let cols = cols.max(1) as u32;
    let px_h = (rows.max(1) as u32) * 2;

    let Ok(img) = image::load_from_memory(data) else {
        return Vec::new();
    };
    let img = img.resize_exact(cols, px_h, FilterType::Triangle).to_rgb8();

    let mut lines = Vec::with_capacity(rows as usize);
    let mut y = 0u32;
    while y + 1 < px_h {
        let mut spans = Vec::with_capacity(cols as usize);
        for x in 0..cols {
            let up = img.get_pixel(x, y).0;
            let lo = img.get_pixel(x, y + 1).0;
            let style = Style::default()
                .fg(Color::Rgb(up[0], up[1], up[2]))
                .bg(Color::Rgb(lo[0], lo[1], lo[2]));
            spans.push(Span::styled("▀", style));
        }
        lines.push(Line::from(spans));
        y += 2;
    }
    lines
}
