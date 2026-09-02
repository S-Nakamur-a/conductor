//! ツール出力の行から、端末の描画をずらす文字を取り除く。

/// ANSI エスケープを捨て、タブを空白 4 つに展開し、他の制御文字を落とす。
///
/// 端末はタブをタブストップまで進め色エスケープを幅ゼロで扱うが、ratatui はどちらも
/// 文字数どおりのセルと数える。残すと行の残りがずれて画面が崩れる。
pub(super) fn sanitize_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape_sequence(&mut chars),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// ESC の直後から、CSI は終端バイト (0x40..=0x7E) まで、OSC は BEL か ST (ESC \) まで、
/// それ以外は 1 文字だけ読み飛ばす。
fn skip_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        Some(']') => {
            while let Some(c) = chars.next() {
                if c == '\u{07}' {
                    break;
                }
                if c == '\u{1b}' {
                    chars.next_if_eq(&'\\');
                    break;
                }
            }
        }
        _ => {}
    }
}
