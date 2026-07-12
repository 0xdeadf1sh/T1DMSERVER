//! Login QR rendering as Unicode half-blocks (two module rows per text row).
//! The Sessions pane feeds this the JSON `{type,token,addr,port}` payload.

use qrcode::{Color, QrCode};

/// Render `data` as a QR code in half-block glyphs. Returns one string per
/// text row (each row packs two module rows). A one-module quiet border is
/// included. On encode failure, returns an empty vec.
pub fn render_qr(data: &str) -> Vec<String> {
    let Ok(code) = QrCode::new(data.as_bytes()) else {
        return Vec::new();
    };
    let width = code.width();
    let modules = code.to_colors();
    let dark = |x: usize, y: usize| -> bool {
        if x >= width || y >= width {
            return false; // quiet zone = light
        }
        modules[y * width + x] == Color::Dark
    };

    let quiet = 1usize;
    let full = width + quiet * 2;
    let mut lines = Vec::with_capacity(full.div_ceil(2));
    let mut y = 0usize;
    while y < full {
        let mut line = String::with_capacity(full);
        for x in 0..full {
            // Coordinates offset by the quiet border.
            let top = dark(x.wrapping_sub(quiet), y.wrapping_sub(quiet));
            let bot = if y + 1 < full {
                dark(x.wrapping_sub(quiet), (y + 1).wrapping_sub(quiet))
            } else {
                false
            };
            line.push(match (top, bot) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        lines.push(line);
        y += 2;
    }
    lines
}
