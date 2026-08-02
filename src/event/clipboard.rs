//! すべてのテキスト入力オーバーレイ/モーダルが共有するクリップボード貼り付けヘルパー。

use crate::app::App;

/// クリップボードの内容を get_buffer が返す TextInput に貼り付ける。
///
/// multiline が false の場合、貼り付けたテキストから改行を取り除く。
pub(in crate::event) fn clipboard_paste<F>(app: &mut App, get_buffer: F, multiline: bool)
where
    F: FnOnce(&mut App) -> &mut crate::text_input::TextInput,
{
    use copypasta::ClipboardProvider;
    let text = app
        .clipboard
        .as_mut()
        .and_then(|ctx| ctx.get_contents().ok());
    if let Some(text) = text {
        let buf = get_buffer(app);
        if multiline {
            buf.insert_str(&text);
        } else {
            let cleaned: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
            buf.insert_str(&cleaned);
        }
    }
}
