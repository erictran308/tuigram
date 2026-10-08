//! The photo viewer: the photo as big as the window allows, keeping its
//! shape, over everything but the status bar; or smaller, zoomed out.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use ratatui_image::FontSize;
use ratatui_image::sliced::{SignedPosition, SlicedImage};

use super::{Cover, popup_block};
use crate::images::Images;
use crate::messages::Preview;
use crate::theme::Colors;
use crate::viewer::{PhotoView, SIZES};

pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    view: &mut PhotoView,
    images: &mut Images,
    colors: &Colors,
) {
    let inner = Block::bordered().inner(area);
    frame.render_widget(Cover, area);
    let shot = shot(
        &view.photo,
        view.zoom.map(|i| SIZES[i]),
        inner.width,
        inner.height,
        images.font_size(),
    );
    view.natural = shot.natural;
    let (cols, rows) = (shot.cols, shot.rows);
    // The largest photo, and the bubble's until that's downloaded.
    images.want(&view.photo, cols, rows);
    let stand_in = view
        .stand_in
        .clone()
        .filter(|_| !images.downloaded(&view.photo));
    if let Some(stand_in) = &stand_in {
        images.want(stand_in, cols, rows);
    }
    let sharp = std::iter::once(&view.photo)
        .chain(&stand_in)
        .find(|p| images.sharp(p, cols, rows).is_some())
        .cloned();
    let ready = sharp
        .as_ref()
        .is_some_and(|p| p.file_id == view.photo.file_id);
    let broken =
        images.is_broken(&view.photo) && view.stand_in.as_ref().is_none_or(|s| images.is_broken(s));

    let mut title = vec![Span::from(" Photo")];
    if let Some(i) = view.zoom {
        title.push(Span::from(format!(" · {:.0}%", SIZES[i] * 100.0)));
    }
    if !ready && !broken {
        title.push(Span::from(" · "));
        title.push(Span::from("loading…").fg(colors.accent).bold());
    }
    title.push(Span::from(" "));
    let block = popup_block(
        Line::from(title),
        " `h/l` older/newer · `j/k` zoom in/out · `o` open in its app · `Esc` close ",
        colors,
    );
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let at = SignedPosition::from((shot.x as i16, shot.y as i16));
    if let Some(photo) = &sharp
        && let Some(image) = images.sharp(photo, cols, rows)
    {
        frame.render_widget(SlicedImage::new(image, at), inner);
        return;
    }
    if let Some(stand_in) = &stand_in
        && let Some(image) = images.get(stand_in, cols, rows)
    {
        frame.render_widget(SlicedImage::new(image, at), inner);
        return;
    }
    // Nothing of the size before stays while this one gets ready: the
    // words in the middle, where the eyes are, say the key was taken.
    let label = match broken {
        true => Line::from("Can't show this photo").fg(colors.subtle),
        false => Line::from("Loading…").fg(colors.accent).bold(),
    };
    let middle = Rect {
        y: inner.y + inner.height / 2,
        height: 1,
        ..inner
    };
    frame.render_widget(label.centered(), middle);
}

/// Where the photo goes in the window.
#[derive(Debug, PartialEq)]
struct Shot {
    cols: u16,
    rows: u16,
    /// From the window's top left corner.
    x: u16,
    y: u16,
    /// The share of the size that fills the window it takes unzoomed.
    natural: f64,
}

/// The photo in the middle of `cols` × `rows` cells: `share` of the size
/// that fills them with its shape kept, since cells are taller than wide;
/// unzoomed (`None`), that size, but no bigger than its own pixels, which
/// would only blur it.
fn shot(photo: &Preview, share: Option<f64>, cols: u16, rows: u16, font: FontSize) -> Shot {
    let (fw, fh) = (f64::from(font.width.max(1)), f64::from(font.height.max(1)));
    let (pw, ph) = (
        f64::from(photo.width.max(1)),
        f64::from(photo.height.max(1)),
    );
    let (cols, rows) = (cols.max(1), rows.max(1));
    let fill = (f64::from(cols) * fw / pw).min(f64::from(rows) * fh / ph);
    let natural = (1.0 / fill).min(1.0);
    let scale = fill * share.unwrap_or(natural);
    let cells =
        |pixels: f64, cell: f64, room: u16| ((pixels * scale / cell).round() as u16).clamp(1, room);
    let (shown_cols, shown_rows) = (cells(pw, fw, cols), cells(ph, fh, rows));
    Shot {
        cols: shown_cols,
        rows: shown_rows,
        x: (cols - shown_cols) / 2,
        y: (rows - shown_rows) / 2,
        natural,
    }
}

#[cfg(test)]
mod tests {
    use image::DynamicImage;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui_image::picker::Picker;
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::images::Key;
    use crate::messages::MediaFile;

    fn photo(width: u32, height: u32) -> Preview {
        Preview {
            file_id: 1,
            width,
            height,
            thumbnail: None,
            sticker: false,
        }
    }

    /// Cells twice as tall as wide, as most fonts' are.
    const FONT: FontSize = FontSize {
        width: 10,
        height: 20,
    };

    /// The cells it takes at `share` of filling 100 × 40 cells.
    fn cells(photo: &Preview, share: Option<f64>) -> (u16, u16) {
        let shot = shot(photo, share, 100, 40, FONT);
        (shot.cols, shot.rows)
    }

    #[test]
    fn a_wide_photo_takes_the_whole_width_and_rows_by_its_shape() {
        // 2000×1000 px in 100 columns: 1000×500 px of cells, 25 rows.
        assert_eq!(cells(&photo(2000, 1000), None), (100, 25));
        let shot = shot(&photo(2000, 1000), None, 100, 40, FONT);
        assert_eq!((shot.x, shot.y), (0, 7), "in the middle");
    }

    #[test]
    fn a_tall_photo_takes_the_whole_height_and_columns_by_its_shape() {
        // 1000×2000 px in 40 rows: 400×800 px of cells, 40 columns.
        assert_eq!(cells(&photo(1000, 2000), None), (40, 40));
    }

    #[test]
    fn a_small_photo_opens_no_bigger_than_its_pixels() {
        assert_eq!(cells(&photo(300, 200), None), (30, 10));
        let shot = shot(&photo(300, 200), None, 100, 40, FONT);
        assert_eq!(shot.natural, 0.3, "of filling the width");
    }

    #[test]
    fn zoomed_it_takes_its_share_of_filling_the_window_and_never_more() {
        assert_eq!(cells(&photo(2000, 1000), Some(0.5)), (50, 13));
        assert_eq!(
            cells(&photo(300, 200), Some(1.0)),
            (100, 33),
            "a small photo fills it"
        );
    }

    /// The viewer's rows, drawn 60 × 20 cells.
    fn draw_rows(view: &mut PhotoView, images: &mut Images) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|f| draw(f, f.area(), view, images, &Colors::default()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..20)
            .map(|y| (0..60).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn title(view: &mut PhotoView, images: &mut Images) -> String {
        draw_rows(view, images).remove(0)
    }

    #[test]
    fn the_title_says_loading_until_the_photo_is_ready_at_its_size() {
        let mut images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let mut view = PhotoView {
            chat_id: 1,
            message_id: 1,
            photo: photo(2000, 1000),
            stand_in: None,
            file: MediaFile {
                id: 1,
                label: "Photo".into(),
                photo: true,
            },
            cant_copy: None,
            zoom: None,
            natural: 1.0,
        };
        assert!(title(&mut view, &mut images).contains("Photo · loading…"));

        let shot = shot(&view.photo, None, 58, 18, images.font_size());
        let key = Key {
            file_id: 1,
            cols: shot.cols,
            rows: shot.rows,
            thumbnail: false,
            avatar: false,
        };
        images.insert_ready(key, DynamicImage::new_rgb8(20, 10));
        let ready = title(&mut view, &mut images);
        assert!(
            ready.contains("Photo") && !ready.contains("loading"),
            "{ready}"
        );

        view.zoom_out();
        let rows = draw_rows(&mut view, &mut images);
        assert!(rows[0].contains("Photo · 75% · loading…"), "{rows:#?}");
        assert!(
            rows[10].contains("Loading…"),
            "in place of the picture before: {rows:#?}"
        );
    }

    #[test]
    fn a_tiny_window_still_shows_a_cell_of_it() {
        let shot = shot(&photo(2000, 1000), Some(0.25), 1, 1, FONT);
        assert_eq!((shot.cols, shot.rows), (1, 1));
    }
}
