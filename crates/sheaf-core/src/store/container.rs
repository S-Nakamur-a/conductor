//! 符号から「それを囲んでいるもの」の綴りを組み立てる。
//!
//! 所属が分かると、ホバーが説明しているのがどの型のフィールドなのかが名前だけで
//! 判る。組み立てられない形では None にする — 当てにいくと別物の名前を出す。

/// `app/types/App#theme_sel.` から `app::types::App` を作る。
///
/// `enclosing` はローカル束縛のように符号そのものが綴りを持たないときに使う。
/// そちらは「囲んでいるもの」がそのまま答えなので、末尾を落とさない。
pub(crate) fn of(symbol: &str, enclosing: Option<&str>, path: &std::path::Path) -> Option<String> {
    let sep = separator(path);
    match parts(symbol) {
        Some(mut parts) if parts.len() > 1 => {
            parts.pop();
            Some(parts.join(sep))
        }
        // 綴りを持つのに囲むものが無い (トップレベル) なら、囲みも無い。
        Some(_) => None,
        None => Some(parts(enclosing?)?.join(sep)),
    }
}

/// Rust だけが `::` で、ほかは `.`。索引を作ったツールではなくソースの綴りで決める。
fn separator(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "::",
        _ => ".",
    }
}

/// 符号の descriptor を 1 つずつに分ける。
///
/// 逆クォートで括られた名前空間は落とす。TypeScript はファイルを
/// ``src/`greet.tsx`/`` という名前空間として符号に含めるが、位置はホバーが別に
/// 出しているので綴りには要らない。名前空間以外の逆クォート (ジェネリックな
/// 型の綴り) は落とすと別物になるので、そこでは組み立てを諦める。
fn parts(symbol: &str) -> Option<Vec<String>> {
    // シンボルは `<scheme> <manager> <package> <version> <descriptors>`。
    let descriptors = symbol.split(' ').nth(4)?;
    let mut parts = Vec::new();
    let mut name = String::new();
    let mut chars = descriptors.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // 逆クォートで括られた名前。中の区切りは字面なので、閉じまで読み切る。
            '`' => {
                let escaped: String = std::iter::from_fn(|| chars.next())
                    .take_while(|&c| c != '`')
                    .collect();
                // 名前空間なら落とす。ほかの位置に来たものは落とすと別物になる。
                if chars.peek() != Some(&'/') {
                    return None;
                }
                chars.next();
                let _ = escaped;
            }
            // 名前空間・型・項の区切り。ここまでが 1 つの descriptor。
            '/' | '#' | '.' => {
                if name.is_empty() {
                    return None;
                }
                parts.push(std::mem::take(&mut name));
            }
            // 名前が確定していれば、メソッドの曖昧さ回避 `name(1).`。読み飛ばす。
            // 確定していなければ引数の descriptor `(name)` で、括弧の中が名前。
            '(' => {
                let inner: String = std::iter::from_fn(|| chars.next())
                    .take_while(|&c| c != ')')
                    .collect();
                if name.is_empty() {
                    parts.push(inner);
                }
            }
            // 型引数、マクロ、メタ。綴りを組み立てる規則を持っていない。
            '[' | ']' | '!' | ':' => return None,
            _ => name.push(c),
        }
    }
    (!parts.is_empty()).then_some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn 囲んでいるものの綴りを組み立てる() {
        for (symbol, enclosing, path, want) in [
            (
                "rust-analyzer cargo conductor 0.1.0 app/types/App#theme_sel.",
                None,
                "a.rs",
                Some("app::types::App"),
            ),
            (
                "rust-analyzer cargo conductor 0.1.0 app/types/App#",
                None,
                "a.rs",
                Some("app::types"),
            ),
            (
                "rust-analyzer cargo conductor 0.1.0 app/App#set_focus().",
                None,
                "a.rs",
                Some("app::App"),
            ),
            // 綴りを持たない符号は、囲んでいる符号がそのまま答えになる。
            (
                "local 3",
                Some("rust-analyzer cargo conductor 0.1.0 app/types/App#theme_sel()."),
                "a.rs",
                Some("app::types::App::theme_sel"),
            ),
            ("local 3", None, "a.rs", None),
            // 区切りは索引を作ったツールではなくソースの拡張子で決まる。
            (
                "scip-go gomod demo . demo/Loud#Greet().",
                None,
                "greet.go",
                Some("demo.Loud"),
            ),
            (
                "scip-typescript npm tsdemo 1.0.0 src/`greet.tsx`/run().(g)",
                None,
                "src/greet.tsx",
                Some("src.run"),
            ),
            // ファイルの名前空間は綴りに入れない。
            (
                "scip-typescript npm tsdemo 1.0.0 src/`greet.tsx`/Loud#greet().",
                None,
                "src/greet.tsx",
                Some("src.Loud"),
            ),
        ] {
            assert_eq!(
                of(symbol, enclosing, Path::new(path)),
                want.map(str::to_string),
                "{symbol}"
            );
        }
    }

    #[test]
    fn 組み立てられない綴りでは黙る() {
        for symbol in [
            "rust-analyzer cargo conductor 0.1.0 app/focus/impl#[Focus][Eq]eq().",
            "rust-analyzer cargo conductor 0.1.0 ui/`PanelChrome<'a>`#draw().",
            "rust-analyzer cargo conductor 0.1.0 macros/log!",
            "local 3",
        ] {
            assert_eq!(of(symbol, None, Path::new("a.rs")), None, "{symbol}");
        }
    }
}
