//! 常駐コストの検査。
//!
//! グローバルアロケータを差し替えるので、他の検査と同じ実行単位に置かない。
//! Store が自分で申告する保持量ではなく、投入中のヒープの山を数える。
//! 申告値だと、全 Document をデコードして保持する実装に退化しても数字が動かない。

use sheaf_core::{IndexSource, Store};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Tracking;

// realloc の既定実装は alloc と dealloc に落ちるので、この 2 つだけで足りる。
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

/// ツリーの .rs を全部ハッシュして、Store::load に渡す表を作る。
/// 実運用では git から取るところ。target と .git は歩かない。
fn tree_hashes(root: &std::path::Path) -> HashMap<PathBuf, String> {
    let mut out = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != "target" && name != ".git" {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(bytes) = std::fs::read(&path)
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.insert(rel.to_path_buf(), sheaf_core::blob_hash(&bytes));
            }
        }
    }
    out
}

#[test]
#[ignore = "実索引が要る"]
fn 索引の投入でヒープが索引ファイルの2倍を超えない() {
    let index = PathBuf::from(
        std::env::var("SHEAF_TEST_INDEX").expect("SHEAF_TEST_INDEX に .scip のパスを渡すこと"),
    );
    let root = PathBuf::from(
        std::env::var("SHEAF_TEST_ROOT").expect("SHEAF_TEST_ROOT にソースツリーのルートを渡すこと"),
    );
    let file_bytes = std::fs::metadata(&index).unwrap().len() as usize;
    // 期待ハッシュの表は呼び出し側の持ち物なので、Store の常駐に数えないよう先に作る。
    let expected = tree_hashes(&root);

    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let store = Store::load(
        &[IndexSource {
            index,
            subroot: PathBuf::new(),
            expected,
        }],
        &root,
    )
    .unwrap();
    let peak = PEAK.load(Ordering::Relaxed) - before;
    let held = LIVE.load(Ordering::Relaxed) - before;

    println!(
        "索引 {} Document / ファイル {:.1}MB / 投入中の山 {:.1}MB ({:.2} 倍) / 投入後 {:.1}MB ({:.2} 倍)",
        store.len(),
        file_bytes as f64 / 1048576.0,
        peak as f64 / 1048576.0,
        peak as f64 / file_bytes as f64,
        held as f64 / 1048576.0,
        held as f64 / file_bytes as f64,
    );
    assert!(
        peak <= file_bytes * 2,
        "投入中の山 {peak} バイトが索引ファイル {file_bytes} バイトの 2 倍を超えた"
    );
    // 全 Document をデコードして保持する実装（7倍）にはほど遠い。
    //
    // 上限が言語で違うのは符号の文字列の長さによる。scip-go の符号は平均 147 バイトで
    // 相異なるものが 16,898 件あり、定義表と転置索引が同じ文字列を別々に持つので
    // 鍵だけで 3.65MB になる（rust-analyzer は 80 バイト / 7,487 件 / 0.91MB）。
    // 実測は Rust 1.20 倍、Go 1.35 倍。
    assert!(
        held * 5 <= file_bytes * 7,
        "投入後の保持 {held} バイトが索引ファイル {file_bytes} バイトの 1.40 倍を超えた"
    );
    // 自己申告が実際の保持量を上回っていたら、下限として読めていない。
    assert!(
        store.retained_bytes() <= held,
        "申告 {} が実際の保持 {held} を上回っている",
        store.retained_bytes()
    );
    drop(store);
}
