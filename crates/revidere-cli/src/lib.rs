//! revidere の CLI 本体。接続点は git diff だけで、特定のホストを前提にしない。
//!
//! AI の呼び出し・プロンプト・応答の解釈・応答の貯め置きはここに閉じている。
//! 成果物を読むだけのホストに、解析側の依存を背負わせないため。
//!
//! 実体をライブラリに置いてあるのは、`revidere` バイナリと
//! `conductor revidere ...` が同じコードを通るようにするため。

mod ai;
mod args;
mod cache;
mod config;
mod error;
mod parse;
mod prompt;

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

use error::CliError;
use revidere::{coverage, diff, git, review::Review};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const USAGE: &str = "\
revidere — git diff を入口にした 3 段階のレビュー支援

  revidere analyze [options]   差分を解析して成果物 JSON を書く
  revidere verify  [options]   変更一覧が git と一致するかだけ確かめる
  revidere ledger  [options]   変更箇所の一覧を表示する
  revidere prompt  [options]   AI に渡すプロンプトを表示する（起動はしない）
  revidere config  [options]   AI の設定の読み先と、無ければ雛形を表示する
  revidere check <file>        既存の成果物の説明もれを調べ直す

options:
  --repo <path>   対象リポジトリ（既定: カレントディレクトリ）
  --base <ref>    比較のベース（既定: origin/HEAD → main → master の順に推定）
  --head <ref>    比較の先端（既定: HEAD）
                  worktree を渡すと、コミット間ではなく作業ツリーを見る
                  （HEAD vs 作業ツリー。未追跡ファイルも含む）
  --out <path>    成果物の書き出し先（既定: <repo>/.revidere/review.json）
  --config <path> 設定ファイル（既定: <repo>/.revidere/config.toml →
                  ~/.config/revidere/config.toml の順に探す）
  --ai <cmd>      AI コマンドを空白区切りで上書きする（既定: 設定ファイルの
                  [ai] command。revidere は AI CLI を同梱しない）
  --timeout <s>   AI の実時間上限を上書きする（既定: 設定ファイル / 900）
  --no-repair     説明なしが残っても差し戻さない
  --no-cache      貯めた応答を使わず、AI に聞き直す（結果は貯め直す）
";

/// サブコマンド名以降の引数を受け取って実行し、終了コードを返す。
/// 0 成功 / 1 失敗 / 2 処理は通ったが説明もれが残った。
pub fn run(argv: impl Iterator<Item = String>) -> u8 {
    let mut argv = argv;
    let Some(cmd) = argv.next() else {
        eprint!("{USAGE}");
        return 1;
    };
    if matches!(cmd.as_str(), "-h" | "--help" | "help") {
        print!("{USAGE}");
        return 0;
    }
    let rest: Vec<String> = argv.collect();
    error::exit_code(dispatch(&cmd, &rest))
}

/// サブコマンド名から引数の型を選び、実行する。
fn dispatch(cmd: &str, rest: &[String]) -> Result<bool, CliError> {
    match cmd {
        "analyze" => cmd_analyze(&args::AnalyzeArgs::parse(rest)?),
        "verify" => cmd_verify(&args::DiffArgs::parse(rest)?),
        "ledger" => cmd_ledger(&args::DiffArgs::parse(rest)?),
        "prompt" => cmd_prompt(&args::DiffArgs::parse(rest)?),
        "config" => cmd_config(&args::ConfigArgs::parse(rest)?),
        "check" => cmd_check(&args::CheckArgs::parse(rest)?),
        other => Err(CliError::Usage(format!(
            "知らないサブコマンド: {other}\n\n{USAGE}"
        ))),
    }
}

/// リポジトリのルートと、解決済みの base/head を得る。
fn resolve(
    repo: &Path,
    base: Option<&str>,
    head: &str,
) -> Result<(PathBuf, String, String), CliError> {
    let root = git::root(repo)?;
    let base = match (base, head) {
        (Some(b), _) => b.to_string(),
        // 作業ツリーを見るときのベースは HEAD しかあり得ない。
        (None, git::WORKTREE) => "HEAD".to_string(),
        (None, _) => git::guess_base(&root)?,
    };
    Ok((root, base, head.to_string()))
}

/// 何を起動するかと、どれだけ待つか。どこから読んだ設定かも返す
/// （analyze・config の両方がそれを表示するため）。
///
/// revidere は AI CLI を持たないので、ここで決まらなければ先へ進めない。
/// 優先順は --ai / --timeout（その場限りの上書き）、次に設定ファイル。
fn resolve_ai(
    config_path: Option<&Path>,
    ai_override: Option<&[String]>,
    timeout_override: Option<Duration>,
    root: &Path,
) -> Result<(Vec<String>, Duration, Option<PathBuf>), CliError> {
    let (cfg, from) = config::load(config_path, root)?;
    let cmd = ai_override
        .map(<[String]>::to_vec)
        .unwrap_or_else(|| cfg.ai.command.clone());
    if cmd.is_empty() {
        return Err(CliError::Config(config::ConfigError(
            config::missing_command_error(root, from.as_deref()),
        )));
    }
    Ok((
        cmd,
        timeout_override.unwrap_or_else(|| cfg.ai.timeout()),
        from,
    ))
}

/// 設定がどこから読まれたかの 1 行。config と analyze の両方で出す。
fn config_source_line(repo: &Path, from: Option<&Path>) -> String {
    match from {
        Some(p) => format!("設定: {}", p.display()),
        None => format!(
            "設定: 見つからない（探した先: {}）",
            config::candidates(repo)
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// 貯めた応答の置き場。成果物と同じ .revidere の下に置く。
fn cache_dir(root: &Path) -> PathBuf {
    root.join(revidere::review::DIR).join("cache")
}

/// 設定がどこから読まれ、何が起動されるのか。無ければ雛形を出す。
fn cmd_config(o: &args::ConfigArgs) -> Result<bool, CliError> {
    // git リポジトリの外でも答えられるようにする。設定の話は差分と関係ない。
    let root = git::root(&o.repo).unwrap_or_else(|_| o.repo.clone());
    let (cfg, from) = config::load(o.config.as_deref(), &root)?;
    println!("{}", config_source_line(&root, from.as_deref()));
    let cmd = o.ai.clone().unwrap_or_else(|| cfg.ai.command.clone());
    if cmd.is_empty() {
        println!(
            "\n{}\n\n--- 雛形 ---\n{}",
            config::missing_command_error(&root, from.as_deref()),
            config::TEMPLATE
        );
        return Ok(false);
    }
    println!("AI コマンド: {}", cmd.join(" "));
    println!(
        "実時間上限: {} 秒",
        o.timeout.unwrap_or_else(|| cfg.ai.timeout()).as_secs()
    );
    let store = cache::Cache::new(cache_dir(&root), o.cache);
    println!("貯めた応答: {} 件 / {}", store.len(), store.dir().display());
    Ok(true)
}

fn load_diff(
    repo: &Path,
    base: Option<&str>,
    head: &str,
) -> Result<(PathBuf, String, String, diff::Diff), CliError> {
    let (root, base, head) = resolve(repo, base, head)?;
    let text = git::diff(&root, &base, &head)?;
    if text.trim().is_empty() {
        return Err(CliError::Message(format!("{base}...{head} に差分が無い")));
    }
    Ok((root, base, head, diff::parse(&text)))
}

fn cmd_ledger(o: &args::DiffArgs) -> Result<bool, CliError> {
    let (_, _, _, d) = load_diff(&o.repo, o.base.as_deref(), &o.head)?;
    print!("{}", d.ledger_summary());
    println!("---\n変更箇所 {} 件", d.positions().len());
    Ok(true)
}

/// 変更一覧が git 自身の集計と一致するか。外部オラクルとの突き合わせ。
fn cmd_verify(o: &args::DiffArgs) -> Result<bool, CliError> {
    let (root, base, head, d) = load_diff(&o.repo, o.base.as_deref(), &o.head)?;
    let stat = git::numstat(&root, &base, &head)?;
    // 未追跡ファイルは numstat に出ないが変更一覧には入る。差ではなく想定内の増分。
    let untracked = if head == git::WORKTREE {
        git::untracked(&root)?
    } else {
        Vec::new()
    };
    let mut ok = true;
    if stat.len() + untracked.len() != d.files.len() {
        println!(
            "ファイル数が違う: numstat {} + 未追跡 {} / 変更一覧 {}",
            stat.len(),
            untracked.len(),
            d.files.len()
        );
        ok = false;
    }
    if !untracked.is_empty() {
        println!("未追跡 {} ファイルを変更一覧に含めた", untracked.len());
    }
    for (path, added, deleted) in &stat {
        let Some(f) = d.file(path) else {
            println!("{path}: numstat にあるが変更一覧に無い");
            ok = false;
            continue;
        };
        // バイナリは numstat が "-" を返すので行数の比較対象にならない。
        let (Some(a), Some(dl)) = (added, deleted) else {
            continue;
        };
        if f.added() != *a || f.deleted() != *dl {
            println!(
                "{path}: numstat +{a} -{dl} / 変更一覧 +{} -{}",
                f.added(),
                f.deleted()
            );
            ok = false;
        }
    }
    println!(
        "{}: {} ファイル / 変更箇所 {} 件",
        if ok { "一致" } else { "不一致" },
        d.files.len(),
        d.positions().len()
    );
    Ok(ok)
}

fn cmd_prompt(o: &args::DiffArgs) -> Result<bool, CliError> {
    let (root, base, head, d) = load_diff(&o.repo, o.base.as_deref(), &o.head)?;
    // analyze と同じく解決済みのコミット ID で組む。呼び名のまま出すと、
    // ここで見えるものと実際に送られるものが違ってしまう。
    let base_oid = git::short_oid(&root, &base).unwrap_or_else(|_| base.clone());
    let head_oid = git::short_oid(&root, &head).unwrap_or_else(|_| head.clone());
    println!("{}", prompt::SYSTEM);
    println!("\n{}", "=".repeat(60));
    println!(
        "{}",
        prompt::user(&base_oid, &head_oid, &d.ledger_summary())
    );
    Ok(true)
}

fn cmd_check(o: &args::CheckArgs) -> Result<bool, CliError> {
    let text = std::fs::read_to_string(&o.file)
        .map_err(|e| CliError::Io(format!("{}: {e}", o.file.display())))?;
    let mut r = Review::from_json(&text)?;
    let root = git::root(&o.repo)?;
    let text = git::diff(&root, &r.base, &r.head)?;
    let d = diff::parse(&text);
    r.coverage = coverage::check(&d.positions(), &r.sections);
    report(&r);
    Ok(r.coverage.is_complete())
}

fn cmd_analyze(o: &args::AnalyzeArgs) -> Result<bool, CliError> {
    let (root, base, head) = resolve(&o.repo, o.base.as_deref(), &o.head)?;
    // 起動するものが決まらないなら、差分を読む前に言う。設定が無いことは
    // 差分を全部数え上げてから知らせる類の話ではない。
    let (cmd, timeout, config_from) =
        resolve_ai(o.config.as_deref(), o.ai.as_deref(), o.timeout, &root)?;
    eprintln!("{}", config_source_line(&root, config_from.as_deref()));
    let base_oid = git::short_oid(&root, &base).unwrap_or_else(|_| base.clone());
    let head_oid = git::short_oid(&root, &head).unwrap_or_else(|_| head.clone());

    if head != git::WORKTREE && git::is_dirty(&root).unwrap_or(false) {
        eprintln!(
            "注意: {} に未コミットの変更がある。レビュー対象は {base}...{head} なので、\n\
             作業ツリーで見えているものと食い違う可能性がある。",
            root.display()
        );
    }

    let text = git::diff(&root, &base, &head)?;
    if text.trim().is_empty() {
        return Err(CliError::Message(format!("{base}...{head} に差分が無い")));
    }
    let d = diff::parse(&text);
    let ledger = d.positions();
    eprintln!(
        "{}: {base}...{head} / {} ファイル / 変更箇所 {} 件",
        root.display(),
        d.files.len(),
        ledger.len()
    );

    // 1 回の抽出に数分かかる。同じ差分を同じコマンドに聞き直す理由は無い。
    let store = cache::Cache::new(cache_dir(&root), o.cache);
    let ask = |user: &str| -> Result<String, CliError> {
        let key = cache::key(&cmd, prompt::SYSTEM, user, &text);
        if let Some((raw, at)) = store.get(&key) {
            // AI が動いていないことは必ず言う。黙って前の答えを返すのが
            // 一番たちが悪い。
            eprintln!("貯めてある応答を使う（AI は起動しない）: {}", at.display());
            return Ok(raw);
        }
        eprintln!("AI を起動: {}", cmd.join(" "));
        let raw = ai::run(&cmd, &root, prompt::SYSTEM, user, timeout)?;
        if let Err(e) = store.put(&key, &raw) {
            eprintln!("応答を貯められなかった（結果は使う）: {e}");
        }
        Ok(raw)
    };

    // プロンプトには解決済みの ID を入れる。同じ範囲を HEAD~2 と呼んでも
    // コミット ID で呼んでも、同じ問いになって貯めた応答に当たる。
    let raw = ask(&prompt::user(&base_oid, &head_oid, &d.ledger_summary()))?;

    let mut r = parse::review(&raw, &base_oid, &head_oid)?;
    r.coverage = coverage::check(&ledger, &r.sections);

    // 説明の無い変更が残ったら、残りだけを渡して差し戻す。全部やり直させると
    // 正しく分類できていた部分まで揺れる。
    if o.repair && !r.coverage.unclassified.is_empty() {
        eprintln!(
            "説明の無い変更が {} 件残ったので差し戻す",
            r.coverage.unclassified.len()
        );
        let gaps = coverage::gap_summary(&r.coverage.unclassified);
        let previous = serde_json::to_string(&r)?;
        match ask(&prompt::repair(&previous, &gaps)) {
            Ok(raw2) => match parse::review(&raw2, &base_oid, &head_oid) {
                Ok(mut r2) => {
                    r2.coverage = coverage::check(&ledger, &r2.sections);
                    // 悪化したら採らない。差し戻しで壊れることは起きる。
                    if r2.coverage.unclassified.len() < r.coverage.unclassified.len() {
                        r = r2;
                    } else {
                        eprintln!("差し戻しで改善しなかったので最初の結果を使う");
                    }
                }
                Err(e) => eprintln!("差し戻しの応答が読めなかったので最初の結果を使う: {e}"),
            },
            Err(e) => eprintln!("差し戻しに失敗したので最初の結果を使う: {e}"),
        }
    }

    let out = o
        .out
        .clone()
        .unwrap_or_else(|| revidere::review::artifact_path(&root));
    write_artifact(&out, &r)?;
    eprintln!("書き出した: {}", out.display());
    report(&r);
    Ok(r.coverage.is_complete())
}

fn write_artifact(path: &Path, r: &Review) -> Result<(), CliError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| CliError::Io(format!("{}: {e}", dir.display())))?;
    }
    let json = r.to_json()?;
    std::fs::write(path, json).map_err(|e| CliError::Io(format!("{}: {e}", path.display())))
}

/// 検査結果を標準出力へ。破れは黙って通さない。
fn report(r: &Review) {
    let c = &r.coverage;
    println!("変更箇所 {} 件 / 説明あり {} 件", c.total, c.classified);
    for (label, items) in [
        ("どの項目でも説明されていない", &c.unclassified),
        ("複数の項目が取り合っている", &c.conflicts),
        ("変更されていない位置を指している", &c.unknown),
    ] {
        if items.is_empty() {
            continue;
        }
        println!("\n{label}: {} 件", items.len());
        for p in items.iter().take(20) {
            println!("  {p}");
        }
        if items.len() > 20 {
            println!("  ... 他 {} 件", items.len() - 20);
        }
    }
    if c.is_complete() {
        println!("\n説明もれ: なし");
    } else {
        println!("\n説明もれ: あり");
    }
    let mut counts = [0usize; 4];
    for ctx in &r.sections {
        counts[ctx.importance as usize] += 1;
    }
    let labels: Vec<String> = revidere::Importance::ORDER
        .iter()
        .map(|i| format!("{} {}", i.label_ja(), counts[*i as usize]))
        .collect();
    println!("項目 {} 件（{}）", r.sections.len(), labels.join(" / "));
    println!("機能への影響 {} 件", r.impacts.len());
}
