// AI の応答を貯める。1 回の抽出に数分かかるので、表示を直すたびにモデルを
// 起こしていたら試行が止まる。
//
// 鍵はプロンプトだけでは足りない。作業ツリーを見ているとき、同じ行を書き換えれば
// 変更一覧は変わらないのに中身は別物になる。差分の本文と、どの AI に聞いたかも
// 鍵に含める。
//
// 鍵はファイル名にするためにハッシュへ潰すが、読むときは鍵そのものを突き合わせる。
// ハッシュの衝突で別の差分のレビューを返すのは、このツールが最も避けたい
// 「静かに間違える」挙動そのもの。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 保存する最大件数。1 件あたり差分とプロンプトを抱えるので、放っておくと
/// 際限なく太る。古いものから捨てる。
const KEEP: usize = 50;

#[derive(Serialize, Deserialize)]
struct Entry {
    version: u32,
    key: String,
    raw: String,
}

const VERSION: u32 = 1;

pub struct Cache {
    dir: PathBuf,
    enabled: bool,
}

impl Cache {
    pub fn new(dir: PathBuf, enabled: bool) -> Self {
        Self { dir, enabled }
    }

    /// 同じ鍵の応答があれば、それと在り処を返す。
    ///
    /// 壊れたファイル・古い版・鍵違いはすべて「無い」として扱う。聞き直せば
    /// 済む話で、失敗を上げても呼ぶ側にできることが無い。
    pub fn get(&self, key: &str) -> Option<(String, PathBuf)> {
        if !self.enabled {
            return None;
        }
        let path = self.path_for(key);
        let text = std::fs::read_to_string(&path).ok()?;
        let e: Entry = serde_json::from_str(&text).ok()?;
        if e.version != VERSION || e.key != key {
            return None;
        }
        Some((e.raw, path))
    }

    /// 応答を貯める。失敗しても致命的ではないので、理由だけ返す。
    pub fn put(&self, key: &str, raw: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let path = self.path_for(key);
        let e = Entry {
            version: VERSION,
            key: key.to_string(),
            raw: raw.to_string(),
        };
        let json = serde_json::to_string(&e).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
        prune(&self.dir, KEEP);
        Ok(path)
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.json", digest(key)))
    }
}

/// 答えを決めているものすべてを 1 本の文字列にする。
/// 区切りに使う \0 は、いずれの成分にも現れない。
pub fn key(ai: &str, system: &str, user: &str, diff: &str) -> String {
    format!("{ai}\0{system}\0{user}\0{diff}")
}

/// 鍵をファイル名に潰す。突き合わせは鍵そのもので行うので、ばらけさえ
/// すればよい（FNV-1a 64bit）。
fn digest(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{h:016x}")
}

fn newest_first(dir: &Path) -> Vec<(std::time::SystemTime, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<_> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let t = e.metadata().ok()?.modified().ok()?;
            Some((t, e.path()))
        })
        .collect();
    v.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    v
}

fn prune(dir: &Path, keep: usize) {
    for (_, p) in newest_first(dir).into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// テストごとに別のディレクトリ。抜けるときに消える。
    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "revidere-cache-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            Tmp(p)
        }

        fn cache(&self, enabled: bool) -> Cache {
            Cache::new(self.0.clone(), enabled)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn 貯めた答えが同じ鍵で戻ってくる() {
        let dir = Tmp::new();
        let c = dir.cache(true);
        let stored = key("ai", "sys", "user", "diff");
        assert!(c.get(&stored).is_none());
        c.put(&stored, "ANSWER").unwrap();
        assert_eq!(c.get(&stored).unwrap().0, "ANSWER");
    }

    /// 差分の本文が鍵に要るのは、作業ツリーを見ていると同じ行の書き換えでは
    /// 変更一覧が変わらないから。呼び先が要るのは、モデルが変われば答えも
    /// 変わるから。
    #[test]
    fn 鍵のどの成分が変わっても当たらない() {
        let before = "-  let a = 1;\n+  let a = 2;";
        for (name, other) in [
            ("呼び先", key("another-ai", "sys", "user", before)),
            ("システムプロンプト", key("ai", "other", "user", before)),
            ("実行ごとの指示", key("ai", "sys", "other", before)),
            (
                "台帳が同じで中身だけ違う差分",
                key("ai", "sys", "user", "-  let a = 1;\n+  let a = 3;"),
            ),
        ] {
            let dir = Tmp::new();
            let c = dir.cache(true);
            c.put(&key("ai", "sys", "user", before), "OLD").unwrap();
            assert!(c.get(&other).is_none(), "{name}が違うのに当たった");
        }
    }

    /// 読まない設定でも書く側は止めない。次の実行で使えるようにしておきたい。
    #[test]
    fn 無効なキャッシュは読まないが書く() {
        let dir = Tmp::new();
        let stored = key("ai", "s", "u", "d");
        dir.cache(true).put(&stored, "A").unwrap();
        let off = dir.cache(false);
        assert!(off.get(&stored).is_none());
        assert!(off.put(&stored, "B").is_ok());
        assert_eq!(dir.cache(true).get(&stored).unwrap().0, "B");
    }

    #[test]
    fn 壊れた項目は当たらない() {
        let dir = Tmp::new();
        let c = dir.cache(true);
        let stored = key("ai", "s", "u", "d");
        c.put(&stored, "A").unwrap();
        std::fs::write(dir.0.join(format!("{}.json", digest(&stored))), "{ broken").unwrap();
        assert!(c.get(&stored).is_none());
    }

    #[test]
    fn 古い項目は掃除される() {
        let dir = Tmp::new();
        std::fs::create_dir_all(&dir.0).unwrap();
        for i in 0..5 {
            std::fs::write(dir.0.join(format!("{i}.json")), "{}").unwrap();
        }
        prune(&dir.0, 2);
        assert_eq!(newest_first(&dir.0).len(), 2);
    }
}
