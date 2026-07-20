//! Clipboard-paste helper shared by every text-entry overlay/modal.

use crate::app::App;

/// Paste clipboard contents into the `TextInput` returned by `get_buffer`.
///
/// If `multiline` is false, newlines are stripped from the pasted text.
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
