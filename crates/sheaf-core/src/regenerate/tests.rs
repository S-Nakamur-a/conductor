//! 再生成の結合検査。Harness が実際に子プロセスを立てて、状態機械を端から駆動する。

use super::job::Lock;
use super::*;
use crate::blob_hash;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 本文どおりに振る舞う producer。出力先が引数の最後に付く。
struct Script(Vec<String>);

impl Producer for Script {
    fn command(&self, out: &Path) -> Vec<String> {
        let mut argv = self.0.clone();
        argv.push(out.to_string_lossy().into_owned());
        argv
    }
}

struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    /// ソースが変わったことを伝える。
    fn touch(&self, regen: &mut Regenerator) {
        regen.note_change(&self.dir.path().join("src/lib.rs"), self.dir.path());
    }
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn a() {}\n").unwrap();
        Harness { dir }
    }

    fn script(&self, body: &str) -> Arc<dyn Producer> {
        use std::os::unix::fs::PermissionsExt;
        let path = self.dir.path().join("producer.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        Arc::new(Script(vec![path.to_string_lossy().into_owned()]))
    }

    fn target(&self) -> Target {
        Target {
            root: self.dir.path().to_path_buf(),
            index: self.dir.path().join("index.scip"),
            hashes: self.dir.path().join("index.hashes"),
            log: self.dir.path().join("index.log"),
            lock: self.dir.path().join("index.lock"),
        }
    }
}

/// 条件が成り立つまで待つ。
///
/// 固定の sleep で待つと、ほかのテストと並列に走ったときに producer の起動が
/// 間に合わず、実装ではなく負荷で落ちる。
fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !cond() {
        assert!(Instant::now() < deadline, "{what} にならない");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 静穏時間を待たずに始められるよう、変更時刻を過去に倒す。
///
/// 変更を伝えないので、「編集が来なくてもやり直す」の検査に使える。
fn backdate(regen: &mut Regenerator) {
    if regen.is_pending() {
        regen.state = State::Pending {
            last_change: Instant::now() - QUIESCENCE - Duration::from_millis(1),
        };
    }
}

fn make_due(regen: &mut Regenerator, h: &Harness) {
    h.touch(regen);
    backdate(regen);
}

/// 走り終わるまで tick を回す。
fn drive(regen: &mut Regenerator, target: &Target) -> Outcome {
    loop {
        if let Some(outcome) = regen.tick(target) {
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// タイムアウトだけ差し替える包み。上限は 300 秒なので、そのままでは検査が書けない。
struct Impatient(Arc<dyn Producer>, Duration);

impl Producer for Impatient {
    fn command(&self, out: &Path) -> Vec<String> {
        self.0.command(out)
    }
    fn timeout(&self) -> Duration {
        self.1
    }
}

/// 索引の置き場所に残っている、生成中の一時ファイル。
///
/// 失敗の経路ごとに手で消していると、経路が増えたときに消し忘れる。残った一時ファイルは
/// 誰も消さないので、置き場所に積もる。
fn leftover_temp_files(h: &Harness) -> Vec<String> {
    let dir = h.target().index.parent().unwrap().to_path_buf();
    let mut out: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    out.sort();
    out
}

/// `src/lib.rs` の `a` に定義 occurrence を1つ持つ索引を書く。
fn write_scip_defining_a(path: &Path) {
    use protobuf::{EnumOrUnknown, Message, MessageField};
    use scip::types::{Document, Index, Metadata, Occurrence, TextEncoding};

    let index = Index {
        metadata: MessageField::some(Metadata {
            text_document_encoding: EnumOrUnknown::new(TextEncoding::UTF8),
            ..Default::default()
        }),
        documents: vec![Document {
            relative_path: "src/lib.rs".to_string(),
            // "pub fn a() {}" の a。
            occurrences: vec![Occurrence {
                range: vec![0, 7, 8],
                symbol: "scip-test cargo demo 0.1.0 a().".to_string(),
                symbol_roles: 1,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    std::fs::write(path, index.write_to_bytes().unwrap()).unwrap();
}

#[test]
fn 変更したファイルも再生成が終われば_exact_に戻る() {
    // 「変更されたから恒久的に構文レベル」にならないことの検査。
    // 鮮度の検査だけがあると、いちど編集したファイルが二度と索引に戻らない
    // 実装でも緑のままになる。
    let h = Harness::new();
    let fixture = h.dir.path().join("fixture.scip");
    write_scip_defining_a(&fixture);
    let mut regen = Regenerator::new(h.script(&format!("cp \"{}\" \"$1\"", fixture.display())));
    let target = h.target();
    let rel = Path::new("src/lib.rs");
    let span = crate::Span {
        start_line: 0,
        start_col: 7,
        end_line: 0,
        end_col: 8,
    };

    make_due(&mut regen, &h);
    let Outcome::Ready { store, .. } = drive(&mut regen, &target) else {
        panic!("最初の生成が Ready にならなかった");
    };
    assert!(
        store.definitions_in(rel, span).is_some(),
        "生成直後なのに索引が答えない"
    );

    // 索引はそのままに、ソースだけを進める。
    std::fs::write(
        h.dir.path().join("src/lib.rs"),
        "pub fn a() {}\n// 足した\n",
    )
    .unwrap();
    assert!(
        store.definitions_in(rel, span).is_none(),
        "書き換えたのに古い索引で答えた"
    );
    drop(store);

    make_due(&mut regen, &h);
    let Outcome::Ready { store, .. } = drive(&mut regen, &target) else {
        panic!("作り直しが Ready にならなかった");
    };
    assert!(
        store.definitions_in(rel, span).is_some(),
        "作り直したのに構文層のままになっている"
    );
}

#[test]
fn ロックを取れなかったら待機に戻って編集を待たずにやり直す() {
    // 索引ルートが複数あると、ブランチ切替のように同時に変わったとき 1 本しか
    // ロックを取れない。負けたほうが待機に戻らないと、次にそのツリーへ編集が
    // 入るまで索引されないままになる。
    let h = Harness::new();
    let counter = h.dir.path().join("runs");
    let mut regen = Regenerator::new(h.script(&format!("echo x >> {}", counter.display())));
    let target = h.target();

    let held = Lock::acquire(&target.lock)
        .expect("ロックを置けない")
        .expect("検査側がロックを取れない");

    make_due(&mut regen, &h);
    let outcome = drive(&mut regen, &target);
    assert!(
        matches!(outcome, Outcome::Busy),
        "ロックを取れなかったのに Busy 以外を返した"
    );
    assert!(
        !counter.exists(),
        "ロックを取れていないのに producer が走った"
    );
    assert!(
        regen.is_pending(),
        "ロックを取れなかったあと待機に戻っていない"
    );

    // ここが本題。h.touch を呼ばない。
    drop(held);
    backdate(&mut regen);
    let _ = drive(&mut regen, &target);
    assert!(
        counter.exists(),
        "ロックが空いても、編集が来るまで producer を起動しなかった"
    );
}

#[test]
fn 上限の時間で終わらない_producer_は孫ごと止める() {
    let h = Harness::new();
    let beat = h.dir.path().join("beat");
    let slow = h.script(&format!(
        "( touch {beat}; while true; do sleep 0.05; touch {beat}; done ) &\nsleep 30",
        beat = beat.display()
    ));
    // 上限そのものが「孫が動いているのを見られる窓」になる。孫は上限で殺されるので、
    // ここが短いと、負荷で sh の起動が間に合わないだけで「動かなかった」と誤判定する。
    let limit = Duration::from_secs(6);
    let mut regen = Regenerator::new(Arc::new(Impatient(slow, limit)));
    let target = h.target();

    make_due(&mut regen, &h);
    let started = Instant::now();
    let _ = regen.tick(&target);

    // 上限が来る前に孫が動いていることを見ておく。動いていないと、
    // このあとの「止まった」が「そもそも動かなかった」と区別できない。
    let beaten_at = || {
        std::fs::metadata(&beat)
            .ok()
            .and_then(|m| m.modified().ok())
    };
    wait_until("孫が動き出した状態", || beaten_at().is_some());
    let first = beaten_at();
    wait_until("孫が触り続けている状態", || beaten_at() != first);

    let outcome = drive(&mut regen, &target);
    let took = started.elapsed();

    let Outcome::Failed(why) = outcome else {
        panic!("上限を過ぎても Failed にならなかった");
    };
    assert!(why.contains("終わらない"), "別の理由で失敗した: {why}");
    // producer 自身は 30 秒走る。上限で切っていなければここに来ない。
    assert!(
        took < Duration::from_secs(10),
        "上限を待たなかった: {took:?}"
    );

    std::thread::sleep(Duration::from_millis(300));
    let stopped_at = beaten_at();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        beaten_at(),
        stopped_at,
        "上限で止めたのに孫が触り続けている"
    );

    assert!(!target.index.exists());
    assert!(!target.hashes.exists());
}

#[test]
fn producer_を起動できなければ以後試みない() {
    let h = Harness::new();
    let missing = h.dir.path().join("no-such-producer");
    let mut regen = Regenerator::new(Arc::new(Script(vec![
        missing.to_string_lossy().into_owned(),
    ])));
    make_due(&mut regen, &h);
    let outcome = loop {
        if let Some(o) = regen.tick(&h.target()) {
            break o;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(outcome, Outcome::Unavailable(_)));

    // 同じパスに動く producer を置いても、もう起動しない。
    let counter = h.dir.path().join("runs");
    std::fs::write(
        &missing,
        format!("#!/bin/sh\ntouch {}\n", counter.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&missing).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&missing, perms).unwrap();

    for _ in 0..30 {
        make_due(&mut regen, &h);
        let _ = regen.tick(&h.target());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!counter.exists(), "諦めたはずの producer を起動した");
}

#[test]
fn 編集が続いている間は生成を始めない() {
    let h = Harness::new();
    let counter = h.dir.path().join("runs");
    let mut regen = Regenerator::new(h.script(&format!("echo x >> {}", counter.display())));
    let target = h.target();
    make_due(&mut regen, &h);
    for _ in 0..20 {
        h.touch(&mut regen);
        let _ = regen.tick(&target);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!regen.is_running());
    assert!(!counter.exists(), "編集が続いているのに生成が始まった");
}

#[test]
fn 索引に載らないファイルの変更は引き金にしない() {
    // ビルド成果物は数秒おきに書き換わる。数えると静穏時間が永久に来ない。
    let h = Harness::new();
    std::fs::write(h.dir.path().join(".gitignore"), "/target\n").unwrap();
    let mut regen = Regenerator::new(h.script("true"));
    let root = h.dir.path();
    for rel in ["target/debug/x", ".git/index", ".sheaf/index.scip"] {
        regen.note_change(&root.join(rel), root);
        assert!(!regen.is_pending(), "{rel} が引き金になった");
    }
    regen.note_change(&root.join("src/lib.rs"), root);
    assert!(regen.is_pending(), "ソースの変更が引き金にならない");
}

#[test]
fn 生成中に来た変更は走り終えてから作り直す() {
    let h = Harness::new();
    let mut regen = Regenerator::new(h.script("true"));
    let target = h.target();
    make_due(&mut regen, &h);
    let _ = regen.tick(&target);
    assert!(regen.is_running());
    // この世代の索引には入らないので、覚えておいて次を回す必要がある。
    h.touch(&mut regen);
    wait_until("生成が終わった状態", || {
        regen.tick(&target).is_some()
    });
    assert!(regen.is_pending(), "生成中に来た変更が捨てられた");
}

#[test]
fn 索引の置き場所が違っても生成は同時に走らない() {
    // 索引を言語やツリーごとに分けても本数の上限が残ることの検査。ロックを
    // 索引のパスから導いていると、分けた瞬間にこれが通らなくなる。
    let h = Harness::new();
    let mut first = Regenerator::new(h.script("sleep 1"));
    let mut second = Regenerator::new(h.script("sleep 1"));
    let a = h.target();
    let b = Target {
        index: h.dir.path().join("other.scip"),
        hashes: h.dir.path().join("other.hashes"),
        ..a.clone()
    };

    make_due(&mut first, &h);
    let _ = first.tick(&a);
    wait_until("1 本目がロックを取った状態", || {
        a.lock.is_file()
    });

    make_due(&mut second, &h);
    let _ = second.tick(&b);
    let outcome = loop {
        if let Some(o) = second.tick(&b) {
            break o;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // producer が索引を書かないので、ロック以外の理由でも生成は失敗する。
    // Busy はロックを取れなかったときにしか返らないので、これで区別できる。
    assert!(
        matches!(outcome, Outcome::Busy),
        "置き場所が違うだけで 2 本目が走った: {}",
        match outcome {
            Outcome::Failed(why) | Outcome::Unavailable(why) => why,
            _ => "Ready".into(),
        }
    );
}

#[test]
fn 引き金を連打しても子プロセスは一本しか立たない() {
    let h = Harness::new();
    let counter = h.dir.path().join("runs");
    let mut regen =
        Regenerator::new(h.script(&format!("echo x >> {}\nsleep 1", counter.display())));
    let target = h.target();
    make_due(&mut regen, &h);
    let _ = regen.tick(&target);
    let runs = || std::fs::read_to_string(&counter).unwrap_or_default();
    wait_until("producer が起動した状態", || {
        runs().lines().count() == 1
    });

    for _ in 0..50 {
        h.touch(&mut regen);
        let _ = regen.tick(&target);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(regen.is_running());
    assert_eq!(runs().lines().count(), 1, "起動回数: {:?}", runs());
    regen.abort();
}

#[test]
fn 中止すると孫プロセスまで止まる() {
    let h = Harness::new();
    let beat = h.dir.path().join("beat");
    let mut regen = Regenerator::new(h.script(&format!(
        "( while true; do touch {}; sleep 0.05; done ) &\nsleep 30",
        beat.display()
    )));
    let target = h.target();
    make_due(&mut regen, &h);
    let _ = regen.tick(&target);
    let beaten_at = || {
        std::fs::metadata(&beat)
            .ok()
            .and_then(|m| m.modified().ok())
    };
    wait_until("孫が動き出した状態", || beaten_at().is_some());
    // 存在だけでは「1 回 touch して死んだ」と区別できず、そのあとの停止の検査が
    // 無意味になるので、更新時刻が進むところまで見る。
    let first = beaten_at();
    wait_until("孫が触り続けている状態", || beaten_at() != first);

    regen.abort();
    std::thread::sleep(Duration::from_millis(300));
    let stopped_at = beaten_at();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(beaten_at(), stopped_at, "中止したのに孫が触り続けている");
}

/// `Store::load` が受け付ける最小の SCIP を書く。Document を 1 つも持たない。
fn write_empty_scip(path: &Path) {
    use protobuf::{EnumOrUnknown, Message, MessageField};
    use scip::types::{Index, Metadata, TextEncoding};

    let mut index = Index::new();
    let mut metadata = Metadata::new();
    metadata.text_document_encoding = EnumOrUnknown::new(TextEncoding::UTF8);
    index.metadata = MessageField::some(metadata);
    std::fs::write(path, index.write_to_bytes().unwrap()).unwrap();
}

#[test]
fn producer_が_document_0件の索引を書いても_ready_にしない() {
    // go.mod が見つからないなど、対象を認識できない producer は終了コード 0 で
    // 空の索引を書くことがある。exit status しか見ないと、これが正常な索引として
    // 古い索引を上書きしてしまう。
    let h = Harness::new();
    let fake_index = h.dir.path().join("empty.scip");
    write_empty_scip(&fake_index);

    let mut regen = Regenerator::new(h.script(&format!("cp \"{}\" \"$1\"", fake_index.display())));
    make_due(&mut regen, &h);
    let outcome = loop {
        if let Some(o) = regen.tick(&h.target()) {
            break o;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(outcome, Outcome::Failed(_)));
    assert!(!h.target().hashes.exists());
    assert!(!h.target().index.exists());
    assert_eq!(leftover_temp_files(&h), Vec::<String>::new());
}

#[test]
fn producer_が失敗しても索引も出自も置かない() {
    let h = Harness::new();
    let mut regen = Regenerator::new(h.script("exit 3"));
    make_due(&mut regen, &h);
    let outcome = loop {
        if let Some(o) = regen.tick(&h.target()) {
            break o;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(outcome, Outcome::Failed(_)));
    assert!(!h.target().hashes.exists());
    assert!(!h.target().index.exists());
    assert_eq!(leftover_temp_files(&h), Vec::<String>::new());
}

#[test]
fn ツリーの完全に外の変更は引き金にしない() {
    // 「索引に載らないファイルの変更は引き金にしない」は root の中の
    // gitignore 対象しか見ていない。root そのものが違う場合の
    // starts_with(root) の側は、ここでしか検査していない。
    let h = Harness::new();
    let mut regen = Regenerator::new(h.script("true"));
    let elsewhere = tempfile::tempdir().unwrap();
    regen.note_change(&elsewhere.path().join("src/lib.rs"), h.dir.path());
    assert!(!regen.is_pending(), "見ていないツリーの変更で時計が進んだ");
}

/// `Store::load` が受け付ける最小の SCIP を書く。中身は空で `relative_path` だけ持つ。
fn write_minimal_scip(path: &Path, rel: &str) {
    use protobuf::{EnumOrUnknown, Message, MessageField};
    use scip::types::{Document, Index, Metadata, TextEncoding};

    let mut index = Index::new();
    let mut metadata = Metadata::new();
    metadata.text_document_encoding = EnumOrUnknown::new(TextEncoding::UTF8);
    index.metadata = MessageField::some(metadata);
    let mut doc = Document::new();
    doc.relative_path = rel.to_string();
    doc.language = "rust".to_string();
    index.documents.push(doc);
    std::fs::write(path, index.write_to_bytes().unwrap()).unwrap();
}

#[test]
fn producer_が成功すると索引を置いてから出自を書いて_ready_が返る() {
    let h = Harness::new();
    let fake_index = h.dir.path().join("fake.scip");
    write_minimal_scip(&fake_index, "src/lib.rs");

    let target = h.target();
    // index.hashes を FIFO にして、読み手が来るまで write_provenance を必ず
    // 止めておく。rename の直後にしか index.scip は現れないので、「読み手を
    // 開く前に index.scip が既にある」が rename が先であることの決定的な
    // 証拠になる(sleep でタイミングを当てにいかない)。
    let status = Command::new("mkfifo").arg(&target.hashes).status().unwrap();
    assert!(status.success(), "mkfifo に失敗した");

    let mut regen = Regenerator::new(h.script(&format!("cp \"{}\" \"$1\"", fake_index.display())));
    make_due(&mut regen, &h);
    let _ = regen.tick(&target);

    wait_until("索引が rename で置かれた状態", || {
        target.index.is_file()
    });
    assert!(
        regen.is_running(),
        "索引を置いた時点でまだ出自を書いている途中のはず"
    );

    let hashes_path = target.hashes.clone();
    let read_hashes = std::thread::spawn(move || std::fs::read_to_string(&hashes_path));

    let outcome = loop {
        if let Some(o) = regen.tick(&target) {
            break o;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(outcome, Outcome::Ready { .. }));

    // 出自の表は root 以下で生成をまたいで動かなかった全ファイルが対象になる
    // (producer のスクリプト自身なども含む)。ここで確かめたいのは
    // src/lib.rs の行が正しいハッシュで載っていること。
    let hashes_body = read_hashes.join().unwrap().unwrap();
    let expected_hash = blob_hash(b"pub fn a() {}\n");
    assert!(hashes_body.contains(&format!("{expected_hash} src/lib.rs\n")));
}

#[test]
fn 生成中に書き換えたファイルは出自から外れる() {
    // unchanged() 自体の単体検査だけでは、producer が実際に走っている最中の
    // 書き換えを本当に拾えているかまでは検査できない。producer 自身に
    // ソースを書き換えさせて、実プロセスを通した経路で確かめる。
    let h = Harness::new();
    let fake_index = h.dir.path().join("fake.scip");
    write_minimal_scip(&fake_index, "src/lib.rs");
    let src = h.dir.path().join("src/lib.rs");

    let mut regen = Regenerator::new(h.script(&format!(
        "echo changed > {}\ncp \"{}\" \"$1\"",
        src.display(),
        fake_index.display()
    )));
    let target = h.target();
    make_due(&mut regen, &h);
    let outcome = loop {
        if let Some(o) = regen.tick(&target) {
            break o;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(outcome, Outcome::Ready { .. }));

    let body = std::fs::read_to_string(&target.hashes).unwrap();
    assert!(
        !body.contains("src/lib.rs"),
        "生成中に書き換えたファイルが出自に残っている: {body}"
    );
}

#[test]
#[ignore = "本物の rust-analyzer を使うので、明示的に指定したときだけ走らせる"]
fn 本物の_rust_analyzer_で_ready_に到達する() {
    let repo_root = std::env::var("CONDUCTOR_TEST_REPO")
        .expect("CONDUCTOR_TEST_REPO に rust-analyzer で索引できるリポジトリのパスを渡すこと");
    let out_dir = tempfile::tempdir().unwrap();
    let target = Target {
        root: PathBuf::from(repo_root),
        index: out_dir.path().join("index.scip"),
        hashes: out_dir.path().join("index.hashes"),
        log: out_dir.path().join("index.log"),
        lock: out_dir.path().join("index.lock"),
    };

    let mut regen = Regenerator::default();
    regen.note_change(&target.root.join("src"), &target.root);
    if regen.is_pending() {
        regen.state = State::Pending {
            last_change: Instant::now() - QUIESCENCE - Duration::from_millis(1),
        };
    }
    let outcome = loop {
        if let Some(o) = regen.tick(&target) {
            break o;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(
        matches!(outcome, Outcome::Ready { .. }),
        "rust-analyzer での生成が Ready に到達しなかった"
    );
    assert!(target.index.is_file());
    assert!(target.hashes.is_file());
}

#[test]
fn 置き場所がまだ無いだけならビジーにしない() {
    // 索引を一度も作っていないリポジトリは `.conductor/` を持たない。ここを
    // ロックの取得失敗と同じ扱いにすると、初回の生成が必ず「ほかのプロセスが
    // 索引を作っている」で終わる。
    let h = Harness::new();
    let counter = h.dir.path().join("runs");
    let mut regen = Regenerator::new(h.script(&format!("echo x >> {}", counter.display())));
    let mut target = h.target();
    let fresh = h.dir.path().join("never-made/.conductor");
    target.lock = fresh.join("generate.lock");
    target.index = fresh.join("index.scip");
    target.hashes = fresh.join("index.hashes");
    target.log = fresh.join("index.log");

    make_due(&mut regen, &h);
    let outcome = drive(&mut regen, &target);
    assert!(
        !matches!(outcome, Outcome::Busy),
        "置き場所が無いだけなのに Busy を返した"
    );
    assert!(counter.exists(), "producer が走っていない");
}
