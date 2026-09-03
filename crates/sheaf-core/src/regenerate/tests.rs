//! 再生成の結合検査。Harness が実際に子プロセスを立てて、状態機械を端から駆動する。

use super::job::Lock;
use super::*;
use crate::blob_hash;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
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

/// 検査が進める時計。
#[derive(Clone)]
struct Hand(Arc<Mutex<Instant>>);

impl Hand {
    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }

    fn clock(&self) -> Clock {
        let at = Arc::clone(&self.0);
        Clock::reading(move || *at.lock().unwrap())
    }
}

struct Harness {
    dir: tempfile::TempDir,
    clock: Hand,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn a() {}\n").unwrap();
        Harness {
            dir,
            clock: Hand(Arc::new(Mutex::new(Instant::now()))),
        }
    }

    fn regenerator(&self, producer: Arc<dyn Producer>) -> Regenerator {
        Regenerator::new(producer).with_clock(self.clock.clock())
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

    /// ソースが変わったことを伝える。
    fn touch(&self, regen: &mut Regenerator) {
        regen.note_change(&self.dir.path().join("src/lib.rs"), self.dir.path());
    }

    fn make_due(&self, regen: &mut Regenerator) {
        self.touch(regen);
        self.quiesce();
    }

    fn quiesce(&self) {
        self.clock.advance(QUIESCENCE);
    }
}

/// 条件が成り立つまで待つ。子プロセスの起動だけは実時間でしか待てない。
fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !cond() {
        assert!(Instant::now() < deadline, "{what} にならない");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 走り終わるまで tick を回す。
fn drive(regen: &mut Regenerator, target: &Target) -> Regenerated {
    loop {
        if let Some(outcome) = regen.tick(target) {
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 置かれた索引を読み直す。
///
/// [`Regenerator::tick`] は索引そのものを返さないので、投入された中身を見る検査は
/// 利用側と同じくディスクから読む。
fn reload(target: &Target, producer: &dyn Producer) -> Store {
    let expected = crate::read_provenance(&target.hashes, producer).expect("出自を読めない");
    let source = crate::IndexSource {
        index: target.index.clone(),
        subroot: PathBuf::new(),
        expected,
    };
    Store::load(std::slice::from_ref(&source), &target.root).expect("置いた索引を読めない")
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

/// 孫プロセスが刻んでいる印。
struct Beat(PathBuf);

impl Beat {
    fn at(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(&self.0)
            .ok()
            .and_then(|m| m.modified().ok())
    }

    /// 孫が動き出し、さらに刻み続けているところまで見る。存在だけでは
    /// 「1 回 touch して死んだ」と区別できず、このあとの停止の検査が無意味になる。
    fn wait_until_running(&self) {
        wait_until("孫が動き出した状態", || self.at().is_some());
        let first = self.at();
        wait_until("孫が触り続けている状態", || self.at() != first);
    }

    fn assert_stopped(&self) {
        std::thread::sleep(Duration::from_millis(300));
        let stopped_at = self.at();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(self.at(), stopped_at, "止めたのに孫が触り続けている");
    }
}

/// 検査用の SCIP を組み立てる。
#[derive(Default)]
struct Scip(Vec<scip::types::Document>);

impl Scip {
    /// 中身の無い Document。パスだけを主張する。
    fn document(mut self, rel: &str) -> Self {
        self.0.push(scip::types::Document {
            relative_path: rel.to_string(),
            language: "rust".to_string(),
            ..Default::default()
        });
        self
    }

    /// `pub fn a() {}` の a に定義 occurrence を持つ Document。
    fn defining_a(mut self, rel: &str) -> Self {
        self.0.push(scip::types::Document {
            relative_path: rel.to_string(),
            occurrences: vec![scip::types::Occurrence {
                range: vec![0, 7, 8],
                symbol: "scip-test cargo demo 0.1.0 a().".to_string(),
                symbol_roles: 1,
                ..Default::default()
            }],
            ..Default::default()
        });
        self
    }

    fn write(self, path: &Path) {
        use protobuf::{EnumOrUnknown, Message, MessageField};
        use scip::types::{Index, Metadata, TextEncoding};

        let index = Index {
            metadata: MessageField::some(Metadata {
                text_document_encoding: EnumOrUnknown::new(TextEncoding::UTF8),
                ..Default::default()
            }),
            documents: self.0,
            ..Default::default()
        };
        std::fs::write(path, index.write_to_bytes().unwrap()).unwrap();
    }
}

#[test]
fn 変更したファイルも再生成が終われば_exact_に戻る() {
    // 鮮度の検査だけがあると、いちど編集したファイルが二度と索引に戻らない実装でも
    // 緑のままになる。
    let h = Harness::new();
    let fixture = h.dir.path().join("fixture.scip");
    Scip::default().defining_a("src/lib.rs").write(&fixture);
    let producer = h.script(&format!("cp \"{}\" \"$1\"", fixture.display()));
    let mut regen = h.regenerator(Arc::clone(&producer));
    let target = h.target();
    let rel = Path::new("src/lib.rs");
    let span = crate::Span {
        start_line: 0,
        start_col: 7,
        end_line: 0,
        end_col: 8,
    };

    h.make_due(&mut regen);
    let Regenerated::Ready { .. } = drive(&mut regen, &target) else {
        panic!("最初の生成が Ready にならなかった");
    };
    let store = reload(&target, &*producer);
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

    h.make_due(&mut regen);
    let Regenerated::Ready { .. } = drive(&mut regen, &target) else {
        panic!("作り直しが Ready にならなかった");
    };
    let store = reload(&target, &*producer);
    assert!(
        store.definitions_in(rel, span).is_some(),
        "作り直したのに構文層のままになっている"
    );
}

#[test]
fn 手で頼まれたぶんだけ静穏時間を待たない() {
    // request の遅れは起動と生成 (実測 2.3GiB) が重なるのを避けるためのもので、
    // 押した本人が待っている場面には当たらない。待つと 3 秒何も起きない。
    let h = Harness::new();
    let counter = h.dir.path().join("runs");
    let producer = h.script(&format!("echo x >> {}", counter.display()));
    let target = h.target();

    let mut waits = h.regenerator(Arc::clone(&producer));
    waits.request();
    waits.tick(&target);
    assert!(!waits.is_running(), "静穏時間を待たずに始めた");

    let mut now = h.regenerator(producer);
    now.request_now();
    now.tick(&target);
    assert!(now.is_running(), "手で頼んだのに次の周で始まらなかった");
    let _ = drive(&mut now, &target);
}

#[test]
fn ロックを取れなかったら待機に戻って編集を待たずにやり直す() {
    // 索引ルートが複数あると、ブランチ切替のように同時に変わったとき 1 本しか
    // ロックを取れない。負けたほうが待機に戻らないと、次にそのツリーへ編集が
    // 入るまで索引されないままになる。
    let h = Harness::new();
    let counter = h.dir.path().join("runs");
    let mut regen = h.regenerator(h.script(&format!("echo x >> {}", counter.display())));
    let target = h.target();

    let held = Lock::acquire(&target.lock)
        .expect("ロックを置けない")
        .expect("検査側がロックを取れない");

    h.make_due(&mut regen);
    let outcome = drive(&mut regen, &target);
    assert!(
        matches!(outcome, Regenerated::Busy),
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

    // ここが本題。変更を伝えない。
    drop(held);
    h.quiesce();
    let _ = drive(&mut regen, &target);
    assert!(
        counter.exists(),
        "ロックが空いても、編集が来るまで producer を起動しなかった"
    );
}

#[test]
fn 上限の時間で終わらない_producer_は孫ごと止める() {
    let h = Harness::new();
    let beat = Beat(h.dir.path().join("beat"));
    let slow = h.script(&format!(
        "( touch {beat}; while true; do sleep 0.05; touch {beat}; done ) &\nsleep 30",
        beat = beat.0.display()
    ));
    let limit = Duration::from_secs(60);
    let mut regen = h.regenerator(Arc::new(Impatient(slow, limit)));
    let target = h.target();

    h.make_due(&mut regen);
    let _ = regen.tick(&target);
    beat.wait_until_running();

    h.clock.advance(limit);
    let Regenerated::Failed(why) = drive(&mut regen, &target) else {
        panic!("上限を過ぎても Failed にならなかった");
    };
    assert!(why.contains("終わらない"), "別の理由で失敗した: {why}");
    beat.assert_stopped();

    assert!(!target.index.exists());
    assert!(!target.hashes.exists());
}

#[test]
fn 中止すると孫プロセスまで止まる() {
    let h = Harness::new();
    let beat = Beat(h.dir.path().join("beat"));
    let mut regen = h.regenerator(h.script(&format!(
        "( while true; do touch {}; sleep 0.05; done ) &\nsleep 30",
        beat.0.display()
    )));
    let target = h.target();
    h.make_due(&mut regen);
    let _ = regen.tick(&target);
    beat.wait_until_running();

    regen.abort();
    beat.assert_stopped();
}

#[test]
fn producer_を起動できなければ以後試みない() {
    let h = Harness::new();
    let missing = h.dir.path().join("no-such-producer");
    let mut regen = Regenerator::new(Arc::new(Script(vec![
        missing.to_string_lossy().into_owned(),
    ])))
    .with_clock(h.clock.clock());
    h.make_due(&mut regen);
    assert!(matches!(
        drive(&mut regen, &h.target()),
        Regenerated::Unavailable(_)
    ));

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
        h.make_due(&mut regen);
        let _ = regen.tick(&h.target());
    }
    assert!(!counter.exists(), "諦めたはずの producer を起動した");
}

#[test]
fn 編集が続いている間は生成を始めない() {
    let h = Harness::new();
    let counter = h.dir.path().join("runs");
    let mut regen = h.regenerator(h.script(&format!("echo x >> {}", counter.display())));
    let target = h.target();
    for _ in 0..20 {
        h.touch(&mut regen);
        h.clock.advance(QUIESCENCE / 2);
        let _ = regen.tick(&target);
    }
    assert!(!regen.is_running());
    assert!(!counter.exists(), "編集が続いているのに生成が始まった");
}

#[test]
fn 索引に載らないファイルの変更は引き金にしない() {
    // ビルド成果物は数秒おきに書き換わる。数えると静穏時間が永久に来ない。
    let h = Harness::new();
    std::fs::write(h.dir.path().join(".gitignore"), "/target\n").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let root = h.dir.path();
    let mut regen = h.regenerator(h.script("true"));

    for changed in [
        root.join("target/debug/x"),
        root.join(".git/index"),
        root.join(".sheaf/index.scip"),
        // gitignore ではなく starts_with(root) が弾く唯一の形。
        elsewhere.path().join("src/lib.rs"),
    ] {
        regen.note_change(&changed, root);
        assert!(
            !regen.is_pending(),
            "{} が引き金になった",
            changed.display()
        );
    }

    regen.note_change(&root.join("src/lib.rs"), root);
    assert!(regen.is_pending(), "ソースの変更が引き金にならない");
}

#[test]
fn 生成中に来た変更は走り終えてから作り直す() {
    let h = Harness::new();
    let mut regen = h.regenerator(h.script("true"));
    let target = h.target();
    h.make_due(&mut regen);
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
    let mut first = h.regenerator(h.script("sleep 1"));
    let mut second = h.regenerator(h.script("sleep 1"));
    let a = h.target();
    let b = Target {
        index: h.dir.path().join("other.scip"),
        hashes: h.dir.path().join("other.hashes"),
        ..a.clone()
    };

    h.make_due(&mut first);
    let _ = first.tick(&a);
    wait_until("1 本目がロックを取った状態", || {
        a.lock.is_file()
    });

    h.make_due(&mut second);
    let outcome = drive(&mut second, &b);
    // producer が索引を書かないので、ロック以外の理由でも生成は失敗する。
    // Busy はロックを取れなかったときにしか返らないので、これで区別できる。
    assert!(
        matches!(outcome, Regenerated::Busy),
        "置き場所が違うだけで 2 本目が走った: {}",
        match outcome {
            Regenerated::Failed(why) | Regenerated::Unavailable(why) => why,
            _ => "Ready".into(),
        }
    );
}

#[test]
fn 引き金を連打しても子プロセスは一本しか立たない() {
    let h = Harness::new();
    let counter = h.dir.path().join("runs");
    let mut regen = h.regenerator(h.script(&format!("echo x >> {}\nsleep 1", counter.display())));
    let target = h.target();
    h.make_due(&mut regen);
    let _ = regen.tick(&target);
    let runs = || std::fs::read_to_string(&counter).unwrap_or_default();
    wait_until("producer が起動した状態", || {
        runs().lines().count() == 1
    });

    for _ in 0..50 {
        h.touch(&mut regen);
        let _ = regen.tick(&target);
    }
    assert!(regen.is_running());
    assert_eq!(runs().lines().count(), 1, "起動回数: {:?}", runs());
    regen.abort();
}

#[test]
fn 生成に失敗したら索引も出自も一時ファイルも残さない() {
    let h = Harness::new();
    let empty = h.dir.path().join("empty.scip");
    Scip::default().write(&empty);

    for (how, body) in [
        ("異常終了する", "exit 3".to_string()),
        // go.mod が見つからないなど、対象を認識できない producer は終了コード 0 で
        // Document 0 件の索引を書く。exit status しか見ないと、これが正常な索引として
        // 古い索引を上書きしてしまう。
        (
            "Document 0 件の索引を書く",
            format!("cp \"{}\" \"$1\"", empty.display()),
        ),
    ] {
        let mut regen = h.regenerator(h.script(&body));
        let target = h.target();
        h.make_due(&mut regen);
        assert!(
            matches!(drive(&mut regen, &target), Regenerated::Failed(_)),
            "{how} producer が Failed 以外を返した"
        );
        assert!(!target.hashes.exists(), "{how} のに出自を置いた");
        assert!(!target.index.exists(), "{how} のに索引を置いた");
        assert_eq!(
            leftover_temp_files(&h),
            Vec::<String>::new(),
            "{how} と一時ファイルが残った"
        );
    }
}

#[test]
fn producer_が成功すると索引を置いてから出自を書いて_ready_が返る() {
    let h = Harness::new();
    let fake_index = h.dir.path().join("fake.scip");
    Scip::default().document("src/lib.rs").write(&fake_index);

    let target = h.target();
    // index.hashes を FIFO にして、読み手が来るまで write_provenance を必ず
    // 止めておく。rename の直後にしか index.scip は現れないので、「読み手を
    // 開く前に index.scip が既にある」が rename が先であることの決定的な
    // 証拠になる(sleep でタイミングを当てにいかない)。
    let status = Command::new("mkfifo").arg(&target.hashes).status().unwrap();
    assert!(status.success(), "mkfifo に失敗した");

    let mut regen = h.regenerator(h.script(&format!("cp \"{}\" \"$1\"", fake_index.display())));
    h.make_due(&mut regen);
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

    assert!(matches!(
        drive(&mut regen, &target),
        Regenerated::Ready { .. }
    ));

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
    Scip::default().document("src/lib.rs").write(&fake_index);
    let src = h.dir.path().join("src/lib.rs");

    let mut regen = h.regenerator(h.script(&format!(
        "echo changed > {}\ncp \"{}\" \"$1\"",
        src.display(),
        fake_index.display()
    )));
    let target = h.target();
    h.make_due(&mut regen);
    assert!(matches!(
        drive(&mut regen, &target),
        Regenerated::Ready { .. }
    ));

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

    let clock = Hand(Arc::new(Mutex::new(Instant::now())));
    let mut regen = Regenerator::default().with_clock(clock.clock());
    regen.note_change(&target.root.join("src"), &target.root);
    clock.advance(QUIESCENCE);
    let outcome = loop {
        if let Some(o) = regen.tick(&target) {
            break o;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(
        matches!(outcome, Regenerated::Ready { .. }),
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
    let mut regen = h.regenerator(h.script(&format!("echo x >> {}", counter.display())));
    let mut target = h.target();
    let fresh = h.dir.path().join("never-made/.conductor");
    target.lock = fresh.join("generate.lock");
    target.index = fresh.join("index.scip");
    target.hashes = fresh.join("index.hashes");
    target.log = fresh.join("index.log");

    h.make_due(&mut regen);
    assert!(
        !matches!(drive(&mut regen, &target), Regenerated::Busy),
        "置き場所が無いだけなのに Busy を返した"
    );
    assert!(counter.exists(), "producer が走っていない");
}
