//! 索引ファイルを投入して、問い合わせに答えられる形にする。
//!
//! 1 リポジトリに索引ルートが複数ある形が実在するので、複数の索引を 1 つの Store に
//! まとめる。まとめるにあたって難しいのは、索引の中の相対パスをリポジトリルート相対へ
//! 接ぎ木することと、同じパスを 2 本の索引が主張したときにどちらを採るかである。

use super::column::{Lines, location_of, usable_range};
use super::scip_split::{self, Split};
use super::{DocEntry, Implements, Store, is_definition, is_local, parse_document};
use crate::Result;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// 索引 1 本ぶんの投入元。
///
/// 1 リポジトリに索引ルートが複数ある形が実在する（Go の go.mod が 8 個、
/// TypeScript の tsconfig.json が 9 個、といった具合）。どれを投入するかは
/// 呼び出し側が並べて渡す。sheaf は索引ルートを探しに行かない。
pub struct IndexSource {
    /// 索引ファイルの場所。
    pub index: PathBuf,
    /// リポジトリルートから見た索引ルートの相対パス。索引ルート自身がリポジトリルートなら空。
    pub subroot: PathBuf,
    /// この索引での出自の表。鍵は**索引ルートからの相対パス**（索引の中の綴りに合わせる）。
    pub expected: HashMap<PathBuf, String>,
}

impl Store {
    /// 索引ファイルを投入する。`root` は照合先のソースツリーのルート。
    ///
    /// `expected` は「索引を生成したツリーでの内容ハッシュ」の表（相対パス -> [`blob_hash`]）。
    /// 索引はソース本文を持たない（rust-analyzer は SCIP の `Document.text` を埋めない）ので、
    /// 出自は外から渡すしかない。ここを `root` から読んで済ませると、別のツリーに向けたときに
    /// 編集済みのファイルまで「索引のまま」と判定して、ずれた行を `Exact` で返す。
    ///
    /// 表に無いファイルは `Exact` の対象から外れる。`root` が索引のツリーそのものであっても、
    /// 生成からここまでの間に編集が入っていれば同じ問題が起きるので、例外は設けない。
    pub fn load(sources: &[IndexSource], root: &Path) -> Result<Self> {
        let mut bytes: Vec<Vec<u8>> = Vec::with_capacity(sources.len());
        for source in sources {
            let raw = std::fs::read(&source.index)?;
            let Split { metadata, .. } = scip_split::split(&raw)?;
            let meta_encoding = match &metadata {
                Some(r) => scip_split::metadata_encoding(&raw[r.clone()])?,
                None => 0,
            };
            // text_document_encoding は列の数え方とは無関係（scip_split::check_text_encoding
            // を参照）。ここではファイルをバイト列として安全に読めるかどうかだけを見る。
            scip_split::check_text_encoding(meta_encoding)?;
            bytes.push(raw);
        }

        // 先に「どのパスをどの索引が持つか」だけ決める。占有が決まってから中身を読むので、
        // 衝突に負けた Document の occurrence を読む手間が要らない。
        let mut owners: HashMap<PathBuf, Owner> = HashMap::new();
        let mut outside_root = 0;
        let mut path_conflicts = 0;
        for (index, source) in sources.iter().enumerate() {
            let raw = &bytes[index];
            let Split { documents, .. } = scip_split::split(raw)?;
            let depth = source.subroot.components().count();
            for span in documents {
                let (rel, doc_encoding) = scip_split::document_header(&raw[span.clone()])?;
                let column_encoding = scip_split::resolve_column_encoding(doc_encoding)?;
                let Some(grafted) = graft(&source.subroot, &rel) else {
                    outside_root += 1;
                    continue;
                };
                let candidate = Owner {
                    index,
                    depth,
                    span,
                    rel: rel.to_string(),
                    column_encoding,
                };
                match owners.get(&grafted) {
                    // 同じパスを 2 本の索引が主張したら、そのパスを含むいちばん深い
                    // 索引ルートの索引を採る。所有者の索引が、そのファイルについて
                    // いちばん多くを知っている。索引ごと弾くと、呼び出し側が握り潰した
                    // ときに全索引が黙って消える。
                    Some(held) if held.depth >= candidate.depth => path_conflicts += 1,
                    Some(_) => {
                        path_conflicts += 1;
                        owners.insert(grafted, candidate);
                    }
                    None => {
                        owners.insert(grafted, candidate);
                    }
                }
            }
        }

        let mut docs: HashMap<PathBuf, DocEntry> = HashMap::with_capacity(owners.len());
        let mut definitions = vec![HashMap::new(); sources.len()];
        let mut references = vec![HashMap::new(); sources.len()];
        let mut implements: Vec<Implements> = vec![HashMap::new(); sources.len()];
        let mut implementers: Vec<HashMap<Box<str>, HashSet<Box<str>>>> =
            vec![HashMap::new(); sources.len()];
        let mut doc_paths: Vec<PathBuf> = Vec::with_capacity(owners.len());
        let mut missing_provenance = 0;
        for (path, owner) in owners {
            let Owner {
                index,
                span,
                rel,
                column_encoding,
                ..
            } = owner;
            // 出自の表の鍵は索引の中の綴り（索引ルート相対）のままなので、接ぎ木前で引く。
            let source_hash = sources[index].expected.get(Path::new(&rel)).cloned();
            if source_hash.is_none() {
                missing_provenance += 1;
            }

            let doc = parse_document(&bytes[index][span.clone()])?;
            // バイトオフセットが確定している Document はソースを読まない
            // （常駐・速度を変えないため）。それ以外だけ変換・判定の根拠として読む。
            let content = (column_encoding != scip_split::ColumnEncoding::Utf8)
                .then(|| std::fs::read(root.join(&path)).ok())
                .flatten();
            let lines = content.as_deref().map(Lines::of);

            let doc_id = doc_paths.len() as u32;
            // 同じシンボルへの参照が1つの Document に何度乗っても doc_id は1回だけ持たせる。
            // そうしないと references_of がその Document を参照の数だけ読み直すことになる。
            let mut referenced_in_doc: HashSet<&str> = HashSet::new();
            for occ in &doc.occurrences {
                if is_local(&occ.symbol) {
                    continue;
                }
                if is_definition(occ.symbol_roles) {
                    let Some(range) = usable_range(&occ.range, column_encoding, lines.as_ref())
                    else {
                        continue;
                    };
                    let Some(loc) = location_of(&range, &path) else {
                        continue;
                    };
                    definitions[index]
                        .entry(occ.symbol.as_str().into())
                        .or_insert_with(Vec::new)
                        .push(loc);
                } else if referenced_in_doc.insert(occ.symbol.as_str()) {
                    references[index]
                        .entry(occ.symbol.as_str().into())
                        .or_insert_with(Vec::new)
                        .push(doc_id);
                }
            }

            for info in &doc.symbols {
                let up: Vec<Box<str>> = info
                    .relationships
                    .iter()
                    .filter(|r| r.is_implementation)
                    // scip-go がインタフェース埋め込みで出す自己ループ（実索引に 3 件）。
                    // 残すと直接参照と同じ位置がインタフェース経由にも並ぶ。
                    .filter(|r| r.symbol != info.symbol)
                    .map(|r| r.symbol.as_str().into())
                    .collect();
                if up.is_empty() {
                    continue;
                }
                for interface in &up {
                    implementers[index]
                        .entry(interface.clone())
                        .or_default()
                        .insert(info.symbol.as_str().into());
                }
                implements[index].insert(info.symbol.as_str().into(), up.into());
            }
            doc_paths.push(path.clone());

            docs.insert(
                path,
                DocEntry {
                    index,
                    span,
                    source_hash,
                    column_encoding,
                },
            );
        }

        Ok(Store {
            bytes,
            docs,
            definitions,
            references,
            implements,
            // 実装の集合はここまでで確定するので、数だけ残して符号は捨てる。
            implementers: implementers
                .into_iter()
                .map(|m| m.into_iter().map(|(k, v)| (k, v.len() as u32)).collect())
                .collect(),
            doc_paths,
            outside_root,
            missing_provenance,
            path_conflicts,
            root: root.to_path_buf(),
        })
    }
}

/// 索引の中で 1 つの Document がどのパスを占めるかを決めるまでの持ち物。
struct Owner {
    index: usize,
    /// 索引ルートの深さ。同じパスを主張されたとき、深いほうがそのファイルの所有者。
    depth: usize,
    span: Range<usize>,
    /// 索引の中の綴り（索引ルート相対）。出自の表を引くのに要る。
    rel: String,
    column_encoding: scip_split::ColumnEncoding,
}

/// 索引ルート相対のパスを、リポジトリルート相対に接ぎ木する。
///
/// 索引の中の `../` は索引ルートから見た上位を指す。リポジトリの中に収まるなら
/// 正当な位置なので受け入れる。登りすぎてリポジトリの外に出るものと、絶対パスは None。
fn graft(subroot: &Path, rel: &str) -> Option<PathBuf> {
    use std::path::Component;
    let mut out: Vec<&std::ffi::OsStr> = subroot.components().map(|c| c.as_os_str()).collect();
    for part in Path::new(rel).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop()?;
            }
            Component::Normal(name) => out.push(name),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out.iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 接ぎ木はリポジトリの外に出るものだけを落とす() {
        let api = Path::new("api");
        assert_eq!(
            graft(api, "src/ok.go"),
            Some(PathBuf::from("api/src/ok.go"))
        );
        assert_eq!(
            graft(api, "./src/ok.go"),
            Some(PathBuf::from("api/src/ok.go"))
        );
        // 索引ルートから上に登っても、リポジトリの中に収まるなら正当な位置。
        // ここを落とすと、scip-go が出す隣のモジュールへの参照が全部消える。
        assert_eq!(
            graft(api, "../shared/x.go"),
            Some(PathBuf::from("shared/x.go"))
        );
        assert_eq!(graft(api, "../../outside.go"), None);
        assert_eq!(graft(api, "/etc/passwd"), None);
        // 索引ルートがリポジトリルートそのものなら、1 つ上がるだけで外に出る。
        assert_eq!(graft(Path::new(""), "../outside.go"), None);
    }
}
