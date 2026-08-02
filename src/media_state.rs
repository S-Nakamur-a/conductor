//! Viewer パネルのメディア描画の状態。
//!
//! image クレートで画像を読み込み、次の 2 つの経路のどちらかで Viewer パネル
//! 向けに描画する:
//!
//! - ハーフブロック (既定): aa_media::Renderer の ANSI 文字列を ratatui の
//!   Line へ変換する。truecolor の端末ならどれでも動く。
//! - ピクセル (rich モードの Tier B): ratatui_image のグラフィックスプロトコル
//!   (kitty / iTerm2 / sixel) のペイロードを、(ファイル, パネルサイズ) の組ごとに
//!   1 度だけバックグラウンドスレッドで作る。エンコード済みのエスケープ列は
//!   ratatui の通常のセル差分に乗るので、変化していない画像が再送されることはない。

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

use aa_media::renderer::{Mode, Renderer};

/// 画像として扱う拡張子。
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// 動画として扱う拡張子。
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "webm", "avi", "mov", "mkv"];

/// ファイルパスがメディア (画像または動画) の拡張子を持つか。
pub fn is_media_file(path: &str) -> bool {
    is_image_file(path) || is_video_file(path)
}

/// ファイルパスが画像の拡張子を持つか。
pub fn is_image_file(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// ファイルパスが動画の拡張子を持つか。
pub fn is_video_file(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    VIDEO_EXTENSIONS.contains(&ext.as_str())
}

/// メディア描画の結果のキャッシュ。
#[derive(Clone)]
pub enum MediaContent {
    /// 描画中。
    Loading,
    /// 描画に成功した行。
    Rendered {
        lines: Vec<Line<'static>>,
        /// 画像の寸法 (幅 x 高さ、ピクセル)。
        dimensions: (u32, u32),
        /// ファイルサイズ (バイト)。
        file_size: u64,
    },
    /// ピクセル品質のグラフィックスプロトコルのペイロード (rich モードの Tier B)。
    /// Arc にしてあるのは [Protocol] が Clone でなく、描画経路が毎フレーム
    /// mutex から中身を clone して取り出すため。
    Pixel {
        protocol: Arc<Protocol>,
        /// 画像の寸法 (幅 x 高さ、ピクセル)。
        dimensions: (u32, u32),
        /// ファイルサイズ (バイト)。
        file_size: u64,
    },
    /// 描画に失敗した。このエラーメッセージを表示する。
    Error(String),
}

/// Viewer でのメディア表示の状態。
pub struct MediaState {
    /// 描画したファイルのパス (変化を検知するため)。
    pub rendered_path: Option<String>,
    /// 直近の描画に使った端末サイズ (桁数, 行数)。
    pub rendered_size: (u16, u16),
    /// 直近の描画がピクセル (グラフィックスプロトコル) 経路だったか。実行中に
    /// rich モードを切り替えるとキャッシュが無効になるようにするため。
    pub rendered_pixel: bool,
    /// 共有の描画結果。バックグラウンドスレッドが更新する。
    pub content: Arc<Mutex<MediaContent>>,
}

impl Default for MediaState {
    fn default() -> Self {
        Self {
            rendered_path: None,
            rendered_size: (0, 0),
            rendered_pixel: false,
            content: Arc::new(Mutex::new(MediaContent::Loading)),
        }
    }
}

impl MediaState {
    /// メディアファイルの描画をバックグラウンドスレッドで開始する。
    ///
    /// picker が Some (rich モードの Tier B) なら画像をグラフィックス
    /// プロトコルのペイロードとしてエンコードし、そうでなければハーフブロックの
    /// ANSI にフォールバックする。ファイル・サイズ・描画経路のいずれも変わって
    /// いなければ何もしない (キャッシュを再利用する)。
    pub fn render_if_needed(
        &mut self,
        full_path: &Path,
        rel_path: &str,
        cols: u16,
        rows: u16,
        picker: Option<Picker>,
    ) {
        let size = (cols, rows);

        // ファイル・サイズ・描画経路が変わっていなければキャッシュを使う。
        if self.rendered_path.as_deref() == Some(rel_path)
            && self.rendered_size == size
            && self.rendered_pixel == picker.is_some()
        {
            return;
        }

        self.rendered_path = Some(rel_path.to_string());
        self.rendered_size = size;
        self.rendered_pixel = picker.is_some();
        // poison からの復帰: デコードスレッドがロックを持ったまま panic した場合でも、
        // Viewer を生かしたまま poison された値を上書きする。
        *self.content.lock().unwrap_or_else(|e| e.into_inner()) = MediaContent::Loading;

        let path = full_path.to_path_buf();
        let content = Arc::clone(&self.content);

        thread::spawn(move || {
            let result = match picker {
                Some(mut picker) => render_image_to_pixels(&path, &mut picker, cols, rows),
                None => render_image_to_lines(&path, cols, rows),
            };
            *content.lock().unwrap_or_else(|e| e.into_inner()) = result;
        });
    }

    /// キャッシュを無効にする (メディア以外のファイルへ切り替えたときなど)。
    pub fn clear(&mut self) {
        self.rendered_path = None;
        self.rendered_size = (0, 0);
        self.rendered_pixel = false;
    }
}

/// 画像ファイルをグラフィックスプロトコルのペイロードへ描画する
/// (rich モードの Tier B)。
///
/// プロトコルのデータは Viewer のメディア領域、すなわちパネルサイズから枠と
/// 情報行を引いた大きさに合わせる。ハーフブロック経路のレイアウトと揃えてある。
fn render_image_to_pixels(path: &Path, picker: &mut Picker, cols: u16, rows: u16) -> MediaContent {
    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let img = match image::open(path) {
        Ok(img) => img,
        Err(e) => return MediaContent::Error(format!("Failed to load image: {e}")),
    };
    let dimensions = (img.width(), img.height());

    let avail_cols = cols.saturating_sub(2);
    let avail_rows = rows.saturating_sub(3); // 枠と情報行のぶん
    let area = Rect::new(0, 0, avail_cols, avail_rows);

    match picker.new_protocol(
        img,
        area,
        Resize::Fit(Some(image::imageops::FilterType::Triangle)),
    ) {
        Ok(protocol) => MediaContent::Pixel {
            protocol: Arc::new(protocol),
            dimensions,
            file_size,
        },
        Err(e) => MediaContent::Error(format!("Graphics render error: {e}")),
    }
}

/// aa-media の Tile レンダラを使って画像ファイルを ratatui の Line へ描画する。
fn render_image_to_lines(path: &Path, cols: u16, rows: u16) -> MediaContent {
    // ファイルサイズを読む。
    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    // 画像を読み込む。
    let img = match image::open(path) {
        Ok(img) => img.into_rgb8(),
        Err(e) => return MediaContent::Error(format!("Failed to load image: {e}")),
    };

    let src_w = img.width();
    let src_h = img.height();

    // 色付きで最も品質が良い Tile モード (ハーフブロック) を使う。
    let mut renderer = Renderer::new(Mode::Tile, " .:-=+*#%@", true);

    // パネルに収まる目標ピクセルサイズを計算する (枠のぶんを空ける)。
    let avail_cols = cols.saturating_sub(2);
    let avail_rows = rows.saturating_sub(3); // 枠と情報行のぶん
    let (tw, th) = renderer.target_size_fit(avail_cols, avail_rows, src_w, src_h);

    // 画像をリサイズする。
    let resized = image::imageops::resize(
        &img,
        tw as u32,
        th as u32,
        image::imageops::FilterType::Triangle,
    );

    // ANSI 文字列へ描画する。
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 16);
    if let Err(e) = renderer.render_frame(&mut buf, &resized, avail_cols) {
        return MediaContent::Error(format!("Render error: {e}"));
    }

    // ANSI 出力を vt100 に通し、スタイル付きのセルを取り出す。
    let lines = ansi_to_ratatui_lines(&buf, avail_cols, avail_rows);

    MediaContent::Rendered {
        lines,
        dimensions: (src_w, src_h),
        file_size,
    }
}

/// ANSI エスケープを含むバイト列を vt100 に通してパースし、ratatui の Line へ変換する。
fn ansi_to_ratatui_lines(ansi_bytes: &[u8], cols: u16, rows: u16) -> Vec<Line<'static>> {
    let parser = vt100::Parser::new(rows, cols, 0);
    let mut parser = parser;
    parser.process(ansi_bytes);

    let screen = parser.screen();
    let mut lines = Vec::new();

    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();
        let mut first = true;

        for col in 0..cols {
            let cell = screen.cell(row, col);
            let cell = match cell {
                Some(c) => c,
                None => continue,
            };

            let style = vt100_cell_to_style(cell);
            let ch = cell.contents();

            if first {
                current_style = style;
                first = false;
            }

            if style != current_style {
                if !current_text.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut current_text),
                        current_style,
                    ));
                }
                current_style = style;
            }

            if ch.is_empty() {
                current_text.push(' ');
            } else {
                current_text.push_str(&ch);
            }
        }

        if !current_text.is_empty() {
            // 末尾の空白を落とす。
            let trimmed = current_text.trim_end();
            if !trimmed.is_empty() {
                spans.push(Span::styled(trimmed.to_string(), current_style));
            }
        }

        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
    }

    // 末尾の空行を取り除く。
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }

    lines
}

/// vt100 のセルの色を ratatui の Style へ変換する。
fn vt100_cell_to_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    style = match cell.fgcolor() {
        vt100::Color::Default => style,
        vt100::Color::Idx(i) => style.fg(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => style.fg(Color::Rgb(r, g, b)),
    };

    style = match cell.bgcolor() {
        vt100::Color::Default => style,
        vt100::Color::Idx(i) => style.bg(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => style.bg(Color::Rgb(r, g, b)),
    };

    if cell.bold() {
        style = style.add_modifier(ratatui::style::Modifier::BOLD);
    }

    style
}
