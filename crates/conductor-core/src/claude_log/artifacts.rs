//! 会話ログ (.jsonl) から、そのセッションが publish した Artifact を拾う。
//!
//! publish は会話のどこにでもあり、ログは百 MB を超えることがあるので、読んだ所までの
//! バイト位置を持ち、次は伸びた分だけを読む。

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use serde::Deserialize;

const ARTIFACT_URL: &str = "https://claude.ai/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub url: String,
    pub title: String,
}

/// list は最後に publish した順で、新しいものが先頭。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub offset: u64,
    pub list: Vec<Artifact>,
}

/// 書きかけの最終行は読まずに残し、次の呼び出しで読む。
pub fn scan(path: &Path, from: Scan) -> io::Result<Scan> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let Scan {
        mut offset,
        mut list,
    } = if len < from.offset {
        Scan::default()
    } else {
        from
    };
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = reader.read_until(b'\n', &mut buf)?;
        if read == 0 || buf.last() != Some(&b'\n') {
            return Ok(Scan { offset, list });
        }
        offset += read as u64;
        let line = String::from_utf8_lossy(&buf);
        if let Some(artifact) = published(&line) {
            list.retain(|a| a.url != artifact.url);
            list.insert(0, artifact);
        }
    }
}

/// publish の結果だけがトップレベルに url / title / version を揃えて持つ。read や open の結果は揃わない。
fn published(line: &str) -> Option<Artifact> {
    if !line.contains("\"toolUseResult\"") {
        return None;
    }
    let result = serde_json::from_str::<Line>(line).ok()?.tool_use_result?;
    result.version?;
    let url = result.url.filter(|url| url.starts_with(ARTIFACT_URL))?;
    Some(Artifact {
        url,
        title: result.title?,
    })
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<ToolUseResult>,
}

#[derive(Deserialize)]
struct ToolUseResult {
    url: Option<String>,
    title: Option<String>,
    version: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const PUBLISHED: &str = r#"{"type":"user","toolUseResult":{"url":"https://claude.ai/artifact/ShKgcpL2tjS6QnG2rBL1xw","path":"/private/tmp/claude-501/-Users-shunnaka-ghq-github-com-S-Nakamur-a-conductor-worktrees-fleet-shell-toggle-git-changes/282b1860-8288-4541-8fb4-f90f6c51e0eb/scratchpad/fleet-shell-plan.html","artifact_id":"d015bb2c-5090-4574-b422-eed716f966a4","title":"一覧のシェル切り替え計画","updated":false,"owned":true,"icon":"terminal","audience":"owner","seq":1,"version":"1793046974-9282","contract":"0.0.0","liveSubscription":"arming"}}"#;
    const OPENED: &str = r#"{"type":"user","toolUseResult":{"opened":true,"url":"https://claude.ai/artifact/HFdQSYX1Xz116qb3jd7ob2","artifact_id":"839d10f1-e8af-4dc4-a7ce-9bc61742371d","title":"計画: claim の定期実行と Claude 起動","owned":true}}"#;
    const READ: &str = r#"{"type":"user","toolUseResult":{"read":{"bytes":35335,"code":200,"codeText":"OK","result":"","durationMs":855,"url":"https://claude.ai/artifact/2XBBeiB6PsHiKbu5HCey8q","title":"どこでもひつじ仙人"}}}"#;

    fn publish(url: &str, title: &str) -> String {
        format!(
            r#"{{"type":"user","toolUseResult":{{"url":"{url}","title":"{title}","version":"1"}}}}"#
        )
    }

    fn log_of(text: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(text.as_bytes()).unwrap();
        file.flush().unwrap();
        file
    }

    fn append(file: &mut tempfile::NamedTempFile, text: &str) {
        file.write_all(text.as_bytes()).unwrap();
        file.flush().unwrap();
    }

    fn titles(scan: &Scan) -> Vec<&str> {
        scan.list.iter().map(|a| a.title.as_str()).collect()
    }

    #[test]
    fn 実物の_publish_の結果を拾う() {
        let file = log_of(&format!("{PUBLISHED}\n"));

        let scan = scan(file.path(), Scan::default()).unwrap();

        assert_eq!(
            scan.list,
            vec![Artifact {
                url: "https://claude.ai/artifact/ShKgcpL2tjS6QnG2rBL1xw".into(),
                title: "一覧のシェル切り替え計画".into(),
            }]
        );
    }

    #[test]
    fn open_や_read_の結果は拾わない() {
        let file = log_of(&format!("{OPENED}\n{READ}\n"));

        assert!(scan(file.path(), Scan::default()).unwrap().list.is_empty());
    }

    #[test]
    fn claude_ai_でない_url_は拾わない() {
        let file = log_of(&format!("{}\n", publish("file:///etc/passwd", "x")));

        assert!(scan(file.path(), Scan::default()).unwrap().list.is_empty());
    }

    #[test]
    fn 同じ_url_は1件に畳み_題は最新で先頭に来る() {
        let a = "https://claude.ai/artifact/a";
        let b = "https://claude.ai/artifact/b";
        let file = log_of(&format!(
            "{}\n{}\n{}\n",
            publish(a, "old"),
            publish(b, "b"),
            publish(a, "new")
        ));

        assert_eq!(
            titles(&scan(file.path(), Scan::default()).unwrap()),
            ["new", "b"]
        );
    }

    #[test]
    fn 壊れた行は飛ばす() {
        let file = log_of(&format!(
            "{{broken\n{}\n",
            publish("https://claude.ai/artifact/a", "a")
        ));

        assert_eq!(titles(&scan(file.path(), Scan::default()).unwrap()), ["a"]);
    }

    #[test]
    fn 書きかけの最終行は読まず_書き終わってから拾う() {
        let line = publish("https://claude.ai/artifact/a", "a");
        let (head, tail) = line.split_at(20);
        let mut file = log_of(&format!("{OPENED}\n{head}"));

        let first = scan(file.path(), Scan::default()).unwrap();
        assert!(first.list.is_empty());
        assert_eq!(first.offset, OPENED.len() as u64 + 1);

        append(&mut file, &format!("{tail}\n"));
        assert_eq!(titles(&scan(file.path(), first).unwrap()), ["a"]);
    }

    #[test]
    fn 伸びた分だけ読み_前の分と合わせる() {
        let mut file = log_of(&format!(
            "{}\n",
            publish("https://claude.ai/artifact/a", "a")
        ));
        let first = scan(file.path(), Scan::default()).unwrap();

        append(
            &mut file,
            &format!("{}\n", publish("https://claude.ai/artifact/b", "b")),
        );
        let second = scan(file.path(), first.clone()).unwrap();

        assert_eq!(titles(&second), ["b", "a"]);
        assert!(second.offset > first.offset);
    }

    #[test]
    fn 前より短くなったら頭から読み直す() {
        let file = log_of(&format!(
            "{}\n",
            publish("https://claude.ai/artifact/b", "b")
        ));
        let stale = Scan {
            offset: 1 << 20,
            list: vec![Artifact {
                url: "https://claude.ai/artifact/gone".into(),
                title: "gone".into(),
            }],
        };

        assert_eq!(titles(&scan(file.path(), stale).unwrap()), ["b"]);
    }

    #[test]
    fn ログが無ければエラー() {
        assert!(scan(Path::new("/nonexistent/x.jsonl"), Scan::default()).is_err());
    }
}
