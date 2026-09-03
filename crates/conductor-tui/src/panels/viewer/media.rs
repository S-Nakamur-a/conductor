//! 画像のプレビュー。ハーフブロックのカラー ASCII アートとして描く。
//!
//! [render] はワーカーが呼ぶ。デコードもリサイズも UI スレッドでは走らせない。

use std::path::Path;

use aa_media::renderer::{Mode, Renderer};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub fn is_image_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| IMAGE_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// 描いた絵と、その下の情報行に出す元画像の素性。
#[derive(Debug, Clone)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    /// 元画像の幅と高さ (ピクセル)。
    pub dimensions: (u32, u32),
    pub bytes: u64,
}

/// 描画を頼んだ相手。同じ組なら描き直さない。
pub type Key = (String, u16, u16);

/// 1 枚ぶんのプレビューの状態。
#[derive(Debug, Clone)]
pub enum Preview {
    Loading,
    Ready(Box<Rendered>),
    Failed(String),
}

/// path を cols x rows の升目に描く。
pub fn render(path: &Path, cols: u16, rows: u16) -> Result<Rendered, String> {
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let image = image::open(path).map_err(|e| e.to_string())?.into_rgb8();
    let dimensions = (image.width(), image.height());

    // Tile はハーフブロックで 1 セルに 2 画素を載せる。色付きで一番密度が高い。
    let mut renderer = Renderer::new(Mode::Tile, " .:-=+*#%@", true);
    let (target_w, target_h) = renderer.target_size_fit(cols, rows, dimensions.0, dimensions.1);
    let resized = image::imageops::resize(
        &image,
        target_w as u32,
        target_h as u32,
        image::imageops::FilterType::Triangle,
    );

    let mut ansi: Vec<u8> = Vec::with_capacity(1 << 16);
    renderer
        .render_frame(&mut ansi, &resized, cols)
        .map_err(|e| e.to_string())?;
    Ok(Rendered {
        lines: ansi_lines(&ansi, cols, rows),
        dimensions,
        bytes,
    })
}

/// ANSI を vt100 に通してセルの色を読む。renderer が吐くのはエスケープ列だけで、
/// ratatui に渡せる形にするには一度画面を組み立てるしかない。
fn ansi_lines(ansi: &[u8], cols: u16, rows: u16) -> Vec<Line<'static>> {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(ansi);
    let screen = parser.screen();

    let mut lines = Vec::new();
    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut text = String::new();
        let mut style: Option<Style> = None;
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let cell_style = cell_style(cell);
            if style != Some(cell_style)
                && let Some(previous) = style.replace(cell_style)
                && !text.is_empty()
            {
                spans.push(Span::styled(std::mem::take(&mut text), previous));
            }
            let glyph = cell.contents();
            if glyph.is_empty() {
                text.push(' ');
            } else {
                text.push_str(&glyph);
            }
        }
        if let Some(style) = style
            && !text.trim_end().is_empty()
        {
            spans.push(Span::styled(text.trim_end().to_string(), style));
        }
        lines.push(Line::from(spans));
    }
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }
    lines
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let color = |c: vt100::Color| match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    };
    let mut style = Style::default();
    if let Some(fg) = color(cell.fgcolor()) {
        style = style.fg(fg);
    }
    if let Some(bg) = color(cell.bgcolor()) {
        style = style.bg(bg);
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

/// 情報行の文言。
pub fn caption(rendered: &Rendered) -> String {
    let (w, h) = rendered.dimensions;
    format!(" {w}x{h} | {} ", human_bytes(rendered.bytes))
}

fn human_bytes(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / (1u64 << 10) as f64),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 画像の判定は大小を無視し画像以外を弾く() {
        let cases = [
            ("a.png", true),
            ("a.PNG", true),
            ("dir/b.jpeg", true),
            ("c.webp", true),
            ("d.mp4", false),
            ("e.rs", false),
            ("README", false),
        ];
        for (path, expected) in cases {
            assert_eq!(is_image_path(path), expected, "{path}");
        }
    }

    #[test]
    fn 読めない画像は理由になる() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("broken.png");
        std::fs::write(&path, "not really a png").unwrap();
        assert!(render(&path, 40, 20).is_err());
    }

    /// 実際に aa-media を通す。ピクセルが行になって初めてプレビューが成立する。
    #[test]
    fn 画像は升目に収まる行になる() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("solid.png");
        let image = image::RgbImage::from_fn(16, 16, |x, _| image::Rgb([(x * 16) as u8, 0, 0]));
        image.save(&path).unwrap();

        let rendered = render(&path, 20, 10).unwrap();
        assert_eq!(rendered.dimensions, (16, 16));
        assert!(!rendered.lines.is_empty());
        assert!(rendered.lines.len() <= 10);
        for line in &rendered.lines {
            assert!(line.width() <= 20, "{line}");
        }
        assert!(caption(&rendered).contains("16x16"));
    }

    #[test]
    fn ファイルサイズは単位付きで出す() {
        let cases = [(512u64, "512 B"), (2048, "2.0 KB"), (3 << 20, "3.0 MB")];
        for (bytes, expected) in cases {
            assert_eq!(human_bytes(bytes), expected);
        }
    }
}
