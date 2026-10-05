//! QR codes drawn with half blocks: each cell shows two modules, one above the
//! other, and cells are about twice as tall as wide, so modules come out square.

use qrcode::{EcLevel, QrCode};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::theme::Colors;

/// Light modules drawn around the code. Scanners need a light margin to find
/// it, and the screen around it may be dark.
const QUIET: usize = 2;

/// `text` as a QR code, dark on light; `None` if it's too long to encode.
pub fn lines(text: &str, colors: &Colors) -> Option<Vec<Line<'static>>> {
    // Lowest error correction keeps the code small; a screen doesn't get
    // scratched.
    let code = QrCode::with_error_correction_level(text, EcLevel::L).ok()?;
    let width = code.width();
    let modules = code.to_colors();
    let size = width + 2 * QUIET;
    let dark = |x: usize, y: usize| {
        (QUIET..QUIET + width).contains(&x)
            && (QUIET..QUIET + width).contains(&y)
            && modules[(y - QUIET) * width + x - QUIET] == qrcode::Color::Dark
    };
    let style = Style::new().fg(colors.qr_dark).bg(colors.qr_light);
    let rows = (0..size)
        .step_by(2)
        .map(|y| {
            let row: String = (0..size)
                .map(|x| match (dark(x, y), dark(x, y + 1)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                })
                .collect();
            // On the span, not the line: a line's style would paint the
            // whole row light, not just the code.
            Line::from(Span::styled(row, style))
        })
        .collect();
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drawn_code_scans_back_to_the_link() {
        let link = "tg://login?token=AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA";
        let rows = lines(link, &Colors::default()).unwrap();

        // Back to modules: each character is two of them, top then bottom.
        let mut modules: Vec<Vec<bool>> = Vec::new();
        for row in &rows {
            let (top, bottom): (Vec<bool>, Vec<bool>) = row
                .to_string()
                .chars()
                .map(|c| (matches!(c, '█' | '▀'), matches!(c, '█' | '▄')))
                .unzip();
            modules.extend([top, bottom]);
        }

        // As pixels, framed in dark like the popup around it on a dark theme,
        // so the drawn light margin is all the scanner gets.
        const SCALE: usize = 4;
        const FRAME: usize = 4;
        let width = (modules[0].len() + 2 * FRAME) * SCALE;
        let height = (modules.len() + 2 * FRAME) * SCALE;
        let mut image = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
            let (x, y) = (
                (x / SCALE).wrapping_sub(FRAME),
                (y / SCALE).wrapping_sub(FRAME),
            );
            match modules.get(y).and_then(|row| row.get(x)) {
                Some(false) => 255,
                Some(true) | None => 0,
            }
        });
        let grids = image.detect_grids();
        assert_eq!(grids.len(), 1, "finds the code");
        let (_, text) = grids[0].decode().unwrap();
        assert_eq!(text, link);
    }
}
