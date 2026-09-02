//! ビルド済みバイナリを取ってきて、走っている実行ファイルと入れ替える。

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::ReleaseAsset;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// 画面に出す段階。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Downloading,
    Extracting,
    Installing,
}

impl Progress {
    pub fn message(self) -> &'static str {
        match self {
            Self::Downloading => "Downloading the pre-built binary\u{2026}",
            Self::Extracting => "Extracting\u{2026}",
            Self::Installing => "Installing\u{2026}",
        }
    }
}

/// アセットを取ってきて、いま走っている実行ファイルを置き換える。
///
/// `version` は作業ディレクトリの名前にしか使わない。
pub fn install(asset: &ReleaseAsset, version: &str, report: impl Fn(Progress)) -> Result<()> {
    let workdir = std::env::temp_dir().join(format!("conductor-update-{version}"));
    let _ = std::fs::remove_dir_all(&workdir);
    std::fs::create_dir_all(&workdir).context("could not create the temporary directory")?;
    let outcome = install_from(asset, &workdir, report);
    let _ = std::fs::remove_dir_all(&workdir);
    outcome
}

fn install_from(asset: &ReleaseAsset, workdir: &Path, report: impl Fn(Progress)) -> Result<()> {
    report(Progress::Downloading);
    let archive = workdir.join(&asset.name);
    download(&asset.download_url, &archive)?;

    report(Progress::Extracting);
    let extract = Command::new("tar")
        .arg("xzf")
        .arg(&archive)
        .arg("-C")
        .arg(workdir)
        .output()
        .context("could not run tar")?;
    if !extract.status.success() {
        bail!("could not extract the archive");
    }
    let new_binary = workdir.join("conductor");
    if !new_binary.exists() {
        bail!("the archive does not contain a conductor binary");
    }

    report(Progress::Installing);
    swap_in(&new_binary)
}

fn download(url: &str, into: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()?;
    let mut request = client.get(url).header("User-Agent", "conductor");
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.is_empty()
    {
        request = request.header("Authorization", format!("token {token}"));
    }
    let mut response = request.send().context("could not download the binary")?;
    if !response.status().is_success() {
        bail!("could not download the binary: HTTP {}", response.status());
    }
    let mut file = std::fs::File::create(into)?;
    std::io::copy(&mut response, &mut file)?;
    Ok(())
}

/// 検証した新しいバイナリを、走っている実行ファイルの位置へ原子的に置く。
fn swap_in(new_binary: &Path) -> Result<()> {
    // ~/.cargo/bin と決め打つと、Homebrew や /usr/local/bin から起動した人の
    // バイナリはそのままに、別のファイルを黙って書き換えてしまう。
    let dest = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .context("could not resolve the running executable")?;
    let dest_dir = dest
        .parent()
        .context("the executable has no parent directory")?;

    // 同じディレクトリへ置く。ファイルシステムを跨ぐ rename は EXDEV で失敗し、
    // 原子的でない copy へ黙って劣化する。
    let staged = dest_dir.join(format!(".conductor-update-{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    std::fs::copy(new_binary, &staged).context("could not stage the new binary")?;
    make_launchable(&staged);

    if let Err(e) = verify_runnable(&staged) {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }

    // dest を上書きすると macOS arm64 では走っているバイナリの署名が壊れ、以降
    // 起動のたびに SIGKILL される。rename ならパスが新しい inode を指すだけで、
    // 走っているプロセスは unlink された古い inode のまま生き続ける。
    let backup = dest_dir.join(".conductor.bak");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(&dest, &backup).context("could not back up the current binary")?;
    if let Err(e) = std::fs::rename(&staged, &dest) {
        let _ = std::fs::rename(&backup, &dest);
        let _ = std::fs::remove_file(&staged);
        return Err(e).context("could not install the new binary");
    }
    let _ = std::fs::remove_file(&backup);
    Ok(())
}

fn make_launchable(staged: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755));
    }
    // 署名は Mach-O に埋まっているので、Gatekeeper に止められる quarantine だけ落とす。
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("xattr").args(["-cr"]).arg(staged).output();
    }
}

/// 入れ替える前のスモークテスト。バージョンすら出せないバイナリで、動いているものを
/// 置き換えてはならない。
fn verify_runnable(staged: &Path) -> Result<()> {
    let status = Command::new(staged)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("the downloaded binary could not be launched")?;
    if !status.success() {
        bail!("the downloaded binary did not run");
    }
    Ok(())
}
