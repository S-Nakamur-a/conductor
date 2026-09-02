//! stdout は JSON-RPC が占有する。ここに 1 バイトでも紛れ込むと、クライアントは
//! プロトコルエラーでセッションを落とすが、原因はどのツールの応答にも現れない。
//!
//! 実行時に fd 1 を覗く形では捕まえられない: libtest はテストスレッドの print!
//! をスレッドローカルに横取りするので、ハンドラの println! が fd 1 に出ない。
//! 印字の入り込む経路そのものをソースから締め出す。

use std::path::Path;

#[test]
fn クレートはstdoutへ印字しない() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let banned = ["println!", "print!", "dbg!", "io::stdout", "stdout()"];

    let mut offenders = Vec::new();
    let mut scanned = 0;
    visit(&src, &mut |path| {
        scanned += 1;
        let text = std::fs::read_to_string(path).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for needle in banned {
                if code.contains(needle) {
                    offenders.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                }
            }
        }
    });

    assert!(scanned > 0, "no sources scanned under {}", src.display());
    assert!(
        offenders.is_empty(),
        "stdout writes found:\n{}",
        offenders.join("\n")
    );
}

fn visit(dir: &Path, f: &mut impl FnMut(&Path)) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            visit(&path, f);
        } else if path.extension().is_some_and(|e| e == "rs") {
            f(&path);
        }
    }
}
