//! 索引を吐く外部ツール。言語ごとに違うのはここだけにする。

use std::path::Path;
use std::time::Duration;

/// 生成を諦めるまでの時間。
///
/// 温まっていれば 12 秒だが、ビルド成果物が無いと 74 秒かかり、さらに
/// rust-analyzer は cargo のパッケージキャッシュのロックを取るので、
/// 利用者のビルドと待ち合う。短く置くと、ビルドしているだけで
/// 再生成が永久に成功しなくなる。
const TIMEOUT: Duration = Duration::from_secs(300);

/// 索引を吐く外部ツール。
///
/// 言語ごとに違うのはここだけにする。起動・寿命・タイムアウト・出自の採取は
/// 言語を問わず同じなので、この層では「何を起動するか」しか聞かない。
pub trait Producer: Send + Sync {
    /// 起動するコマンドと引数。cwd はツリーのルートに固定して呼ぶ。
    fn command(&self, out: &Path) -> Vec<String>;

    /// 生成を諦めるまでの時間。既定は [`TIMEOUT`]。
    ///
    /// 短くしてよいのは、その producer が待ち合う相手を持たないと分かっているときだけ。
    fn timeout(&self) -> Duration {
        TIMEOUT
    }
}

/// Rust 用の既定の producer。
pub struct RustAnalyzer;

impl Producer for RustAnalyzer {
    fn command(&self, out: &Path) -> Vec<String> {
        vec![
            "rust-analyzer".into(),
            "scip".into(),
            ".".into(),
            "--output".into(),
            out.to_string_lossy().into_owned(),
        ]
    }
}

/// Go 用の既定の producer。
pub struct ScipGo;

impl Producer for ScipGo {
    fn command(&self, out: &Path) -> Vec<String> {
        vec![
            "scip-go".into(),
            "index".into(),
            "--output".into(),
            out.to_string_lossy().into_owned(),
        ]
    }
}

/// TypeScript 用の既定の producer。
///
/// 版を固定する。`@latest` だと、新版が出た瞬間に黙って別の道具に切り替わる。
///
/// `--infer-tsconfig` は渡さない。無いときに `tsconfig.json` を対象のツリーへ
/// 書き込むので、索引を作るだけのつもりが相手のリポジトリを書き換える。
pub struct ScipTypescript;

impl Producer for ScipTypescript {
    fn command(&self, out: &Path) -> Vec<String> {
        vec![
            "npx".into(),
            "--yes".into(),
            "-p".into(),
            "@sourcegraph/scip-typescript@0.4.0".into(),
            // scip-typescript は typescript を `^5.6.2` で抱えており、npx が実行時に解決する。
            // 解決された版は `scip-typescript npm typescript 5.9.3 lib/...` の形で
            // シンボルの綴りに埋まるので、放っておくと新版が出た日に索引が黙って変わる。
            "-p".into(),
            "typescript@5.9.3".into(),
            "scip-typescript".into(),
            "index".into(),
            "--cwd".into(),
            ".".into(),
            "--output".into(),
            out.to_string_lossy().into_owned(),
            "--no-progress-bar".into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 既定の_producer_はツリーのパスを渡す() {
        // rust-analyzer scip は位置引数の path が必須で、無いと即座に落ちる。
        // 索引を吐くコマンドが本当に動く形かは、ここでしか見ていない。
        let argv = RustAnalyzer.command(Path::new("/tmp/out.scip"));
        assert_eq!(argv[0], "rust-analyzer");
        assert_eq!(argv[1], "scip");
        assert!(argv.contains(&".".to_string()), "argv: {argv:?}");
    }

    #[test]
    fn go_の_producer_は出力先を渡す() {
        // scip-go index は位置引数を省くと既定の ./... を対象にする。
        let argv = ScipGo.command(Path::new("/tmp/out.scip"));
        assert_eq!(argv[0], "scip-go");
        assert_eq!(argv[1], "index");
        assert!(
            argv.contains(&"/tmp/out.scip".to_string()),
            "argv: {argv:?}"
        );
    }
}
