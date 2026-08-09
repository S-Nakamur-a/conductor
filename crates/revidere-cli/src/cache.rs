// AI の応答を貯める。
//
// 1 回の抽出に数分かかる。表示を直す・説明もれ検査をかけ直す・ホストを変えて眺める、
// といった作業は成果物さえあれば済むので、そこでモデルを起こしていたら試行が止まる。
//
// 鍵は「答えを決めているものすべて」。プロンプト（＝ base/head と変更一覧）だけでは
// 足りない。作業ツリーを見ているとき、同じ行を書き換えれば変更一覧は変わらないのに
// 中身は別物になる。だから差分の本文と、起動するコマンドも鍵に含める。
//
// 鍵はファイル名にするためにハッシュへ潰すが、読むときは鍵そのものを突き合わせる。
// ハッシュの衝突で別の差分のレビューを返すのは、このツールが最も避けたい
// 「静かに間違える」挙動そのもの。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 保存する最大件数。1 件あたり差分とプロンプトを抱えるので、放っておくと
/// 際限なく太る。古いものから捨てる。
const KEEP: usize = 50;

/// 貯めたものの形。鍵をそのまま持つのは、読むときに突き合わせるため。
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

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 貯めてある件数。無ければ 0。
    pub fn len(&self) -> usize {
        entries(&self.dir).len()
    }

    /// 同じ鍵の応答があれば、それと在り処を返す。
    ///
    /// 壊れたファイル・古い版・鍵違いはすべて「無い」として扱う。捨てて
    /// 聞き直せば済む話で、ここで失敗を上げても呼ぶ側にできることが無い。
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
///
/// 起動するコマンド（モデルが変われば答えも変わる）、システムプロンプト、
/// 実行ごとの指示、そして差分の本文。区切りに使う \0 は、いずれにも現れない。
pub fn key(argv: &[String], system: &str, user: &str, diff: &str) -> String {
    format!("{}\0{system}\0{user}\0{diff}", argv.join("\u{1}"))
}

/// 鍵をファイル名に潰す。突き合わせは鍵そのもので行うので、ここは
/// ばらけさえすればよい（FNV-1a 64bit）。
fn digest(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{h:016x}")
}

/// 貯めてあるものを新しい順に。
fn entries(dir: &Path) -> Vec<(std::time::SystemTime, PathBuf)> {
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

/// 新しい keep 件だけ残す。
fn prune(dir: &Path, keep: usize) {
    for (_, p) in entries(dir).into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// テストごとに別のディレクトリ。消してから返す。
    fn tmp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "revidere-cache-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn argv() -> Vec<String> {
        vec!["ai".to_string()]
    }

    #[test]
    fn a_stored_answer_comes_back() {
        let c = Cache::new(tmp(), true);
        let k = key(&argv(), "sys", "user", "diff");
        assert!(c.get(&k).is_none());
        c.put(&k, "ANSWER").unwrap();
        assert_eq!(c.get(&k).unwrap().0, "ANSWER");
        let _ = std::fs::remove_dir_all(c.dir());
    }

    /// 変更一覧が同じでも中身が違えば別物。作業ツリーを見ているときに、同じ行を
    /// 書き換えるとこうなる。ここで当たると、前の内容のレビューを黙って返す。
    #[test]
    fn the_same_ledger_with_different_content_is_a_miss() {
        let c = Cache::new(tmp(), true);
        let before = key(&argv(), "sys", "user", "-  let a = 1;\n+  let a = 2;");
        let after = key(&argv(), "sys", "user", "-  let a = 1;\n+  let a = 3;");
        c.put(&before, "OLD").unwrap();
        assert!(c.get(&after).is_none(), "内容が違うのに当たった");
        let _ = std::fs::remove_dir_all(c.dir());
    }

    /// コマンドが変われば、答えを出すモデルも変わる。
    #[test]
    fn a_different_command_is_a_miss() {
        let c = Cache::new(tmp(), true);
        c.put(&key(&argv(), "s", "u", "d"), "A").unwrap();
        let other = vec!["another-ai".to_string()];
        assert!(c.get(&key(&other, "s", "u", "d")).is_none());
        let _ = std::fs::remove_dir_all(c.dir());
    }

    #[test]
    fn a_disabled_cache_never_reads() {
        let dir = tmp();
        Cache::new(dir.clone(), true)
            .put(&key(&argv(), "s", "u", "d"), "A")
            .unwrap();
        let off = Cache::new(dir.clone(), false);
        assert!(off.get(&key(&argv(), "s", "u", "d")).is_none());
        // 書く側は止めない。次の実行で使えるように上書きしておきたい。
        assert!(off.put(&key(&argv(), "s", "u", "d"), "B").is_ok());
        assert_eq!(
            Cache::new(dir.clone(), true)
                .get(&key(&argv(), "s", "u", "d"))
                .unwrap()
                .0,
            "B"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 壊れたファイルは「無い」として扱う。聞き直せば済む。
    #[test]
    fn a_corrupt_entry_is_a_miss() {
        let c = Cache::new(tmp(), true);
        let k = key(&argv(), "s", "u", "d");
        c.put(&k, "A").unwrap();
        std::fs::write(c.dir().join(format!("{}.json", digest(&k))), "{ broken").unwrap();
        assert!(c.get(&k).is_none());
        let _ = std::fs::remove_dir_all(c.dir());
    }

    #[test]
    fn old_entries_are_pruned() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            std::fs::write(dir.join(format!("{i}.json")), "{}").unwrap();
        }
        prune(&dir, 2);
        assert_eq!(entries(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
