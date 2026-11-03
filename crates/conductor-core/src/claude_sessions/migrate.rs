//! 会話のログを、別の working directory (grab/ungrab) や別のアカウントの config dir から
//! claude --resume <id> で見つけられるようにする。どちらもシンボリックリンクで、ログの実ファイルは
//! 1 本のまま。

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::discovery::now_ms;
use super::{ClaudeHome, log_in_root};

impl ClaudeHome {
    /// from_root の会話のログを to_root からも resume できるようにする。to_root の同じ名前を
    /// from_root のログの実ファイルへのリンクにする。両方に別のファイルがあれば長い方を残し、
    /// どちらも相手の先頭でなければ to_root の方を退ける。退けたファイルを返す。
    pub fn share_session_log(
        from_root: &Path,
        to_root: &Path,
        working_dir: &Path,
        session_id: &str,
    ) -> Result<Option<PathBuf>> {
        let source = log_in_root(from_root, working_dir, session_id)
            .or_else(|| log_anywhere_in(from_root, session_id))
            .with_context(|| format!("no log of {session_id} in {}", from_root.display()))?;
        let real = std::fs::canonicalize(&source)?;
        let relative = source.strip_prefix(from_root)?;
        join(&to_root.join(relative), &real)
    }

    /// <session_id>.jsonl と、あればサブエージェントの <session_id>/ を dest 側へリンクし、
    /// dest の resume 一覧に出るよう history.jsonl にも追記する。どちらも会話を走らせた
    /// config_dir の中で済ませる。そこにログが無ければ何もせず Ok(false)。
    pub fn migrate_session(
        config_dir: &Path,
        session_id: &str,
        source_working_dir: &Path,
        dest_working_dir: &Path,
        display_hint: &str,
    ) -> Result<bool> {
        let log_name = format!("{session_id}.jsonl");
        if !Self::projects_dir_in(config_dir, source_working_dir)
            .join(&log_name)
            .exists()
        {
            log::warn!(
                "migrate_session: no {log_name} for {}",
                source_working_dir.display()
            );
            return Ok(false);
        }
        let src_dir = Self::projects_dir_in(config_dir, source_working_dir);
        let dst_dir = Self::projects_dir_in(config_dir, dest_working_dir);
        std::fs::create_dir_all(&dst_dir)?;
        link(&src_dir.join(&log_name), &dst_dir.join(&log_name))?;
        if src_dir.join(session_id).is_dir() {
            link(&src_dir.join(session_id), &dst_dir.join(session_id))?;
        }
        append_history_entry(config_dir, session_id, dest_working_dir, display_hint)?;
        Ok(true)
    }

    /// migrate_session が作ったリンクを外す。
    ///
    /// 実体に置き換わっていたら (CC が書き換えたとき)、最新の会話は dest にしか無いので source へ
    /// コピーして戻してから消す。別のアカウントのログへのリンク (share_session_log) は、外す前に source から辿れる
    /// ようにする。root ごとに続け、最初の失敗を返す。
    pub fn unmigrate_session(
        &self,
        session_id: &str,
        source_working_dir: &Path,
        dest_working_dir: &Path,
    ) -> Result<()> {
        let log_name = format!("{session_id}.jsonl");
        let mut first_error = None;
        for root in &self.roots {
            let src_dir = Self::projects_dir_in(root, source_working_dir);
            let dst_dir = Self::projects_dir_in(root, dest_working_dir);
            for name in [log_name.as_str(), session_id] {
                if let Err(e) = take_back(&dst_dir.join(name), &src_dir.join(name)) {
                    first_error.get_or_insert(e);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

/// CC も --resume <id> では config dir の全プロジェクトから id で探す。
fn log_anywhere_in(root: &Path, session_id: &str) -> Option<PathBuf> {
    let name = format!("{session_id}.jsonl");
    std::fs::read_dir(root.join("projects"))
        .ok()?
        .flatten()
        .map(|project| project.path().join(&name))
        .find(|path| path.is_file())
}

/// path を実ファイル real と同じ 1 本に解決させる。path の側にだけある内容を失わせないため、
/// 長い方を残し、どちらも相手の先頭でなければ path の側を退ける。退けたファイルを返す。
fn join(path: &Path, real: &Path) -> Result<Option<PathBuf>> {
    if same_file(path, real) {
        return Ok(None);
    }
    let Ok(other) = std::fs::canonicalize(path) else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        place_link(path, real)?;
        return Ok(None);
    };
    if is_prefix(&other, real)? {
        place_link(&other, real)?;
        return Ok(None);
    }
    if is_prefix(real, &other)? {
        place_link(real, &other)?;
        return Ok(None);
    }
    let kept = set_aside(&other)?;
    place_link(&other, real)?;
    Ok(Some(kept))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
}

/// short の中身が long の先頭と同じか (同じ内容も含む)。
fn is_prefix(short: &Path, long: &Path) -> Result<bool> {
    let len = std::fs::metadata(short)?.len();
    if len > std::fs::metadata(long)?.len() {
        return Ok(false);
    }
    let mut a = std::fs::File::open(short)?.take(len);
    let mut b = std::fs::File::open(long)?.take(len);
    let (mut x, mut y) = (vec![0; 1 << 16], vec![0; 1 << 16]);
    loop {
        let n = a.read(&mut x)?;
        if n == 0 {
            return Ok(true);
        }
        b.read_exact(&mut y[..n])?;
        if x[..n] != y[..n] {
            return Ok(false);
        }
    }
}

/// hard link で別名を足す。rename と違って既にある名前を潰さず、元の名前が消える瞬間も無い。
fn set_aside(path: &Path) -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for n in 0.. {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{n}")
        };
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".replaced-{stamp}{suffix}"));
        let aside = PathBuf::from(name);
        match std::fs::hard_link(path, &aside) {
            Ok(()) => return Ok(aside),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("set aside {}", path.display())),
        }
    }
    unreachable!()
}

/// path を target へのリンクにする。途中で落ちても path は前のまま残るよう、隣に作って rename する。
fn place_link(path: &Path, target: &Path) -> Result<()> {
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().context("log path has no file name")?);
    name.push(".conductor-link");
    let temp = path.with_file_name(name);
    let _ = std::fs::remove_file(&temp);
    std::os::unix::fs::symlink(target, &temp)
        .with_context(|| format!("symlink {} -> {}", temp.display(), target.display()))?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })?;
    Ok(())
}

fn append_history_entry(
    root: &Path,
    session_id: &str,
    project_path: &Path,
    display: &str,
) -> Result<()> {
    use std::io::Write;
    let entry = serde_json::json!({
        "display": display,
        "sessionId": session_id,
        "timestamp": now_ms(),
        "project": project_path.to_string_lossy(),
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("history.jsonl"))?;
    writeln!(file, "{entry}")?;
    Ok(())
}

fn link(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        return Ok(());
    }
    std::os::unix::fs::symlink(src, dst)
        .with_context(|| format!("symlink {} -> {}", dst.display(), src.display()))?;
    log::info!("migrate_session: symlinked {}", dst.display());
    Ok(())
}

/// dst がリンクなら外す。辿った先がログのファイルなら、外す前に src を同じ 1 本にする。
/// 実体なら src へコピーしてから消す。無ければ何もしない。
fn take_back(dst: &Path, src: &Path) -> Result<()> {
    let Ok(meta) = dst.symlink_metadata() else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        if let Ok(real) = std::fs::canonicalize(dst)
            && real.is_file()
        {
            join(src, &real)?;
        }
        std::fs::remove_file(dst)?;
    } else if meta.is_dir() {
        copy_dir_recursive(dst, src)
            .with_context(|| format!("copy back {} -> {}", dst.display(), src.display()))?;
        std::fs::remove_dir_all(dst)?;
    } else {
        if let Some(parent) = src.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(dst, src)
            .with_context(|| format!("copy back {} -> {}", dst.display(), src.display()))?;
        std::fs::remove_file(dst)?;
    }
    log::info!("unmigrate_session: removed {}", dst.display());
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}
