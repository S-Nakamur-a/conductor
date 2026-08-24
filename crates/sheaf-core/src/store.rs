//! 索引の保持。いまはメモリ上だけで、永続化しない。
//!
//! 位置クエリのたびに該当 Document だけをデコードして捨てる。
//! デコードしたまま持つと常駐がファイルの 7 倍になる。

mod column;
mod container;
#[cfg(test)]
mod fixture;
mod kind;
mod load;
mod scip_split;
mod slot;
#[cfg(test)]
mod tests;

pub use load::IndexSource;
pub use slot::Slot;

use self::column::{Lines, contained_in, location_of, usable_range};
use crate::{
    Enclosing, Found, Implementation, Location, Result, SheafError, Span, SymbolDetail, SymbolId,
    ViaInterface, blob_hash,
};
use protobuf::Message;
use scip::types::{Document, SymbolInformation, SymbolRole};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// 実装しているシンボル -> それが実装しているインタフェース側のシンボル。索引 1 本ぶん。
type Implements = HashMap<Box<str>, Box<[Box<str>]>>;

/// インタフェース側のシンボル -> それを実装しているシンボル。索引 1 本ぶん。
type Implementers = HashMap<Box<str>, Box<[Box<str>]>>;

/// trait の綴り -> その trait を実装している impl ブロック。索引 1 本ぶん。
///
/// 鍵が完全な符号ではなく綴りなのは、聞かれた位置から引けるようにするため。
/// 同名の trait があると混ざるので、答えは `Derived` として返す。
type TraitImpls = HashMap<Box<str>, Vec<Implementation>>;

/// 実装先クエリに索引が返せるもの。索引が書いた関係と、符号の綴りからの導出は
/// 強さが違うので分けて返す。
pub(crate) enum Implemented {
    Declared(Vec<Implementation>),
    Derived(Vec<Implementation>),
}

/// 定義クエリに索引が返せるもの。
pub(crate) enum Resolved {
    /// 聞かれた語そのものの定義。
    Direct(Vec<Location>),
    /// 語そのものの定義は索引に無く、囲んでいる型の定義を返す。
    Enclosing(Vec<Enclosing>),
}

/// `... app/focus/impl#[Focus][...]eq().` から `... app/focus/Focus#` を組み立てる。
///
/// derive が作った impl は定義位置を索引に持たないが、その型は持っている。
/// 実測で自クレートへの参照 37,120 出現のうち 397 出現がこれで回収できる。
///
/// **索引に書いてある符号ではなく、組み立てた符号である。** impl が型とは別の
/// モジュールに置かれていて、しかもそのモジュールに同名の型があると別物に届く。
/// だから直接の定義が引けないときだけ使い、答えも Exact とは分けている。
fn enclosing_type(symbol: &str) -> Option<String> {
    // シンボルは `<scheme> <manager> <package> <version> <descriptors>`。
    let cut = symbol.match_indices(' ').nth(3)?.0 + 1;
    let (coordinate, descriptors) = symbol.split_at(cut);
    let at = descriptors.find("impl#[")?;
    let open = at + "impl#[".len();
    let close = descriptors[open..].find(']')? + open;
    let name = &descriptors[open..close];
    // 素の識別子でなければ諦める。`PanelChrome<'a>` のような綴りから型の符号を
    // 組み立てる規則を持っていないので、当てにいくと違うものに届く。
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(format!("{coordinate}{}{name}#", &descriptors[..at]))
}

/// 符号の中の型・trait の綴りを、照合できる形に均す。
///
/// rust-analyzer はジェネリックな綴りをバッククォートで括る (`` `Bridge<'a>` ``) ので、
/// 括りを外して型引数の手前までを取る。impl 側と定義側で綴りが違うと、実装を
/// 持つ trait が 1 件も引けなくなる。
fn descriptor_name(spelled: &str) -> &str {
    let plain = spelled.trim_matches('`');
    match plain.split_once('<') {
        Some((head, _)) => head,
        None => plain,
    }
}

/// 型を指す符号から、その型の綴りを取る。`syntactic/SyntacticLayer#` から
/// `SyntacticLayer`。
///
/// trait かどうかはここでは判らない。struct の綴りで実装の表を引いても、表に
/// 無いので空振りするだけで、誤った実装先は出ない。
fn trait_name(symbol: &str) -> Option<&str> {
    let descriptors = symbol.split(' ').nth(4)?;
    let name = descriptors.strip_suffix('#')?;
    let name = descriptor_name(name.rsplit(['/', '#']).next()?);
    (!name.is_empty()).then_some(name)
}

/// 実装側の符号から、実装している型の綴りを取る。
/// `demo/Loud#` から `Loud`、`demo/Loud#Greet().` から `Loud`。
///
/// 型を指す descriptor (`#` で終わる) のうち末尾のものを採る。無い符号は末尾の
/// descriptor そのものを綴りとする。
fn implementing_type(symbol: &str) -> Option<&str> {
    let descriptors = symbol.split(' ').nth(4)?;
    let mut last_type = None;
    let mut start = 0;
    for (at, c) in descriptors.char_indices() {
        match c {
            '#' => {
                last_type = Some(&descriptors[start..at]);
                start = at + 1;
            }
            '/' | '.' => start = at + 1,
            _ => {}
        }
    }
    let name = last_type.unwrap_or(&descriptors[start..]);
    let name = descriptor_name(name.split('(').next().unwrap_or(name));
    (!name.is_empty()).then_some(name)
}

/// impl の符号から (実装している型, 実装している trait) の綴りを取る。
/// `.../impl#[Rough][SyntacticLayer]` から `("Rough", "SyntacticLayer")`。
///
/// **ブロック自身の符号だけでは足りない。** ジェネリックな impl は索引にブロックの
/// 符号を持たず (`impl<'a> SyntacticLayer for Bridge<'a>` が実際にそう)、中の
/// メソッドの符号しか出ない。だからメソッドの符号も同じ形として受ける。
/// trait を実装していない impl は 2 つめの角括弧を持たないので None。
fn impl_pair(symbol: &str) -> Option<(&str, &str)> {
    let descriptors = symbol.split(' ').nth(4)?;
    let at = descriptors.rfind("impl#[")?;
    let (ty, rest) = descriptors[at + "impl#[".len()..].split_once(']')?;
    let (implemented, _) = rest.strip_prefix('[')?.split_once(']')?;
    let ty = ty.trim_matches('`');
    let implemented = descriptor_name(implemented);
    (!ty.is_empty() && !implemented.is_empty()).then_some((ty, implemented))
}

/// 宣言の本文。`SymbolInformation.signature_documentation` の中身。
///
/// **型付きで読むだけでは足りない。** scip crate が生成する `Signature` は本文を
/// 2 番に置くが、rust-analyzer が書くのは旧仕様の `Document` で、本文は 5 番にある。
/// フィールド番号が違うので `Signature::text` は必ず空文字列になり、実際の本文は
/// 未知フィールドに落ちる。両方の綴りから読む。
fn signature_text(signature: &scip::types::Signature) -> Option<String> {
    const DOCUMENT_TEXT: u32 = 5;
    if !signature.text.is_empty() {
        return Some(signature.text.clone());
    }
    let unknown = signature.special_fields.unknown_fields();
    let protobuf::UnknownValueRef::LengthDelimited(bytes) = unknown.get(DOCUMENT_TEXT)? else {
        return None;
    };
    let text = String::from_utf8(bytes.to_vec()).ok()?;
    (!text.is_empty()).then_some(text)
}

/// `documentation` の先頭に置かれた宣言と、残りの doc。
///
/// `signature_documentation` を書かない producer がある。scip-typescript は宣言を
/// ```` ```ts ```` で括って `documentation` の先頭に入れるので、そこから取る。
/// 括りが無ければ宣言ではないので、全部 doc のまま返す。
fn fenced_declaration(documentation: &[String]) -> (Option<String>, Vec<String>) {
    let Some(first) = documentation.first() else {
        return (None, Vec::new());
    };
    let Some(body) = first
        .strip_prefix("```")
        .and_then(|rest| rest.split_once('\n'))
        .and_then(|(_, body)| body.trim_end().strip_suffix("```"))
    else {
        return (None, documentation.to_vec());
    };
    let body = body.trim_end().to_string();
    if body.is_empty() {
        return (None, documentation.to_vec());
    }
    (Some(body), documentation[1..].to_vec())
}

struct DocEntry {
    /// どの索引から来たか。[`Store::definitions`] と [`Store::references`] の添字でもある。
    index: usize,
    span: Range<usize>,
    /// 索引を生成したツリーでのソースの内容ハッシュ。呼び出し側が申告する。
    /// 出自を言えないファイルは None にして、その Document 由来の答えを Exact にしない。
    source_hash: Option<String>,
    /// この Document の occurrence.range をどう扱うか。
    column_encoding: scip_split::ColumnEncoding,
}

pub struct Store {
    /// 索引ファイルそのもの。索引ごとに 1 本。span はこの中への範囲。
    bytes: Vec<Vec<u8>>,
    /// リポジトリルートからの相対パスで引く。索引の中の綴りは索引ルート相対なので、
    /// 投入時に接ぎ木して寄せてある。
    docs: HashMap<PathBuf, DocEntry>,
    /// 位置クエリのたびに全 Document を走査しないために持つ。
    /// 同じシンボルIDに定義が複数あることは実索引で起きる（別々の関数の中の同名 const など）。
    /// どれも正しいので、先に見つけたものだけを残すと残りを黙って隠すことになる。
    ///
    /// **索引ごとに分ける。** 索引をまたぐと、別物が同じ符号を持つ組み合わせで
    /// 誤った定義を Exact で返す（scip-typescript は名前の無い package.json に
    /// 索引ルートの情報を含まない座標を振るので、これは実際に起こせる）。
    definitions: Vec<HashMap<Box<str>, Vec<Location>>>,
    /// シンボル -> その参照が載っている Document 番号。定義と違い、位置まで持つと
    /// occurrence の数（実索引で9万を超える）だけ確保することになるので番号だけ持ち、
    /// 引くたびに該当 Document を読み直す。番号は doc_paths の添字と対応する。
    /// definitions と同じ理由で索引ごとに分ける。
    references: Vec<HashMap<Box<str>, Vec<u32>>>,
    /// 実装しているシンボル -> それが実装しているインタフェース側のシンボル。
    /// 辿るのは上向き 1 ホップだけなので、逆向きの表は持たない
    /// （`docs/spec-interface-references.md`）。definitions と同じ理由で索引ごとに分ける。
    implements: Vec<Implements>,
    /// インタフェース側のシンボル -> それを実装しているシンボル。
    /// 参照が届く見込み (数) と、実装先へのジャンプ (符号) の両方に要る。
    implementers: Vec<Implementers>,
    /// trait の綴り -> それを実装している impl ブロック。
    ///
    /// `implements` とは出どころが違う。あちらは索引が書いた `relationships` で、
    /// こちらは impl ブロックの符号の綴りから導出したもの。rust-analyzer は
    /// `relationships` を出さないので、Rust ではこちらだけが埋まる。
    trait_impls: Vec<TraitImpls>,
    /// Document 番号からパスを引く表。references の値を照合先に戻すためだけに使う。
    doc_paths: Vec<PathBuf>,
    /// ルートの外を指していて投入しなかった Document の数。
    /// 無回答が索引の欠落によるものだと分かるようにする。
    outside_root: usize,
    /// `expected` に出自が無かった Document の数。
    /// 無回答が出自の欠落によるものだと分かるようにする。
    missing_provenance: usize,
    /// 複数の索引が同じパスを主張して、負けたほうを捨てた数。
    path_conflicts: usize,
    root: PathBuf,
}

// 索引のバイト列をそのまま出さない。12MB が転がり出てもデバッグの役に立たない。
impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("root", &self.root)
            .field("documents", &self.docs.len())
            .field("indexes", &self.bytes.len())
            .field(
                "definitions",
                &self.definitions.iter().map(HashMap::len).sum::<usize>(),
            )
            .field(
                "references",
                &self.references.iter().map(HashMap::len).sum::<usize>(),
            )
            .field("outside_root", &self.outside_root)
            .field("missing_provenance", &self.missing_provenance)
            .field(
                "index_bytes",
                &self.bytes.iter().map(Vec::len).sum::<usize>(),
            )
            .finish()
    }
}

impl Store {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 索引が持つ Document の数。
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// ルートの外を指していて投入しなかった Document の数。
    pub fn outside_root(&self) -> usize {
        self.outside_root
    }

    /// `expected` に出自が無かった Document の数。
    pub fn missing_provenance(&self) -> usize {
        self.missing_provenance
    }

    /// 複数の索引が同じパスを主張して、負けたほうを捨てた数。
    pub fn path_conflicts(&self) -> usize {
        self.path_conflicts
    }

    /// 保持しているバイト数。アロケータの断片は含まないので、常駐メモリの下限として読む。
    pub fn retained_bytes(&self) -> usize {
        let defs: usize = self
            .definitions
            .iter()
            .flatten()
            .map(|(k, v)| {
                k.len()
                    + v.iter()
                        .map(|l| l.path.as_os_str().len() + size_of::<Location>())
                        .sum::<usize>()
            })
            .sum();
        let refs: usize = self
            .references
            .iter()
            .flatten()
            .map(|(k, v)| k.len() + v.len() * size_of::<u32>())
            .sum();
        let paths: usize = self.docs.keys().map(|p| p.as_os_str().len()).sum();
        let doc_paths: usize = self.doc_paths.iter().map(|p| p.as_os_str().len()).sum();
        self.bytes.iter().map(Vec::len).sum::<usize>() + defs + refs + paths + doc_paths
    }

    /// その位置の語について、索引が書いている説明。
    ///
    /// [`symbols_in`](Self::symbols_in) と同じく位置の主張ではないので、鮮度の
    /// 扱いも同じ。聞かれたファイルと、説明の出どころになった Document の両方が
    /// 索引生成時のままでなければ答えない — 古い綴りを最新のコードの説明として
    /// 見せると、名前だけそれらしい別物になる。
    pub(crate) fn describe_in(&self, rel: &Path, span: Span) -> Option<Vec<SymbolDetail>> {
        if !self.is_current(rel) {
            return None;
        }
        let entry = self.docs.get(rel)?;
        let doc = parse_document(&self.bytes[entry.index][entry.span.clone()]).ok()?;
        let content = (entry.column_encoding != scip_split::ColumnEncoding::Utf8)
            .then(|| std::fs::read(self.root.join(rel)))
            .transpose()
            .ok()?;
        let lines = content.as_deref().map(Lines::of);
        let at = InDocument {
            doc: &doc,
            rel,
            encoding: entry.column_encoding,
            lines: lines.as_ref(),
        };

        let mut out: Vec<SymbolDetail> = Vec::new();
        for occ in &doc.occurrences {
            let Some(range) = usable_range(&occ.range, entry.column_encoding, lines.as_ref())
            else {
                continue;
            };
            if !contained_in(&range, span) {
                continue;
            }
            if out.iter().any(|d| d.symbol.as_str() == occ.symbol) {
                continue;
            }
            out.push(self.detail_of(&occ.symbol, entry.index, &doc, &at));
        }
        Some(out)
    }

    /// その符号の説明を組み立てる。
    ///
    /// 説明はその符号の**定義がある Document** に載る。ローカル束縛は定義も参照も
    /// 同じ Document なので、まず手元を見るだけで当たる。説明が見つからなくても
    /// 符号は答えになる (何を指しているかは分かる) ので、中身が空のまま返す。
    fn detail_of(
        &self,
        symbol: &str,
        index: usize,
        doc: &Document,
        at: &InDocument<'_>,
    ) -> SymbolDetail {
        if let Some(info) = doc.symbols.iter().find(|i| i.symbol == symbol) {
            return self.to_detail(info, at.rel);
        }
        for loc in self.definitions_of(symbol, index, at) {
            if !self.is_current(&loc.path) {
                continue;
            }
            let Some(entry) = self.docs.get(&loc.path) else {
                continue;
            };
            let Ok(defining) = parse_document(&self.bytes[entry.index][entry.span.clone()]) else {
                continue;
            };
            if let Some(info) = defining.symbols.iter().find(|i| i.symbol == symbol) {
                return self.to_detail(info, &loc.path);
            }
        }
        SymbolDetail {
            symbol: SymbolId(symbol.into()),
            kind: crate::SymbolKind::Unknown,
            container: None,
            signature: None,
            documentation: Vec::new(),
        }
    }

    fn to_detail(&self, info: &SymbolInformation, at: &Path) -> SymbolDetail {
        let (signature, documentation) = match info
            .signature_documentation
            .as_ref()
            .and_then(signature_text)
        {
            Some(text) => (Some(text), info.documentation.clone()),
            None => fenced_declaration(&info.documentation),
        };
        let kind = match kind::of(info.kind.value(), &info.symbol) {
            crate::SymbolKind::Unknown => signature
                .as_deref()
                .map_or(crate::SymbolKind::Unknown, kind::from_declaration),
            known => known,
        };
        let enclosing = (!info.enclosing_symbol.is_empty()).then_some(&*info.enclosing_symbol);
        SymbolDetail {
            symbol: SymbolId(info.symbol.as_str().into()),
            kind,
            container: container::of(&info.symbol, enclosing, at),
            signature,
            documentation,
        }
    }

    /// その位置の語が trait なら、それを実装している impl ブロック。
    ///
    /// 索引が最新でなければ None。飛び先の impl ブロックがあるファイルが変わって
    /// いれば、その 1 件を落とすのではなく答えを丸ごと捨てる — 残りだけを返すと
    /// 消えた実装があることが呼び出し側に見えない。
    pub(crate) fn implementations_in(&self, rel: &Path, span: Span) -> Option<Implemented> {
        if !self.is_current(rel) {
            return None;
        }
        let entry = self.docs.get(rel)?;
        let doc = parse_document(&self.bytes[entry.index][entry.span.clone()]).ok()?;
        let content = (entry.column_encoding != scip_split::ColumnEncoding::Utf8)
            .then(|| std::fs::read(self.root.join(rel)))
            .transpose()
            .ok()?;
        let lines = content.as_deref().map(Lines::of);

        let mut declared: Vec<Implementation> = Vec::new();
        let mut derived: Vec<Implementation> = Vec::new();
        let mut fresh = Freshness::starting_at(self, rel);
        for occ in &doc.occurrences {
            let Some(range) = usable_range(&occ.range, entry.column_encoding, lines.as_ref())
            else {
                continue;
            };
            if !contained_in(&range, span) {
                continue;
            }
            for found in self.declared_implementations(&occ.symbol, entry.index, &mut fresh)? {
                if !declared.contains(&found) {
                    declared.push(found);
                }
            }
            let Some(name) = trait_name(&occ.symbol) else {
                continue;
            };
            let Some(found) = self.trait_impls[entry.index].get(name) else {
                continue;
            };
            for imp in found {
                if !fresh.allows(&imp.site.path) {
                    return None;
                }
                if !derived.contains(imp) {
                    derived.push(imp.clone());
                }
            }
        }
        // 索引が書いた関係があるなら綴りからの導出は要らない。両方を混ぜると、
        // 弱いほうに引きずられて答え全体が Derived になる。
        let (mut out, declared) = if declared.is_empty() {
            (derived, false)
        } else {
            (declared, true)
        };
        out.sort_by(|a, b| position_key(&a.site).cmp(&position_key(&b.site)));
        Some(if declared {
            Implemented::Declared(out)
        } else {
            Implemented::Derived(out)
        })
    }

    /// 索引が `relationships` に書いている実装先。rust-analyzer は 1 件も書かないが、
    /// scip-go と scip-typescript は書く。
    fn declared_implementations(
        &self,
        symbol: &str,
        index: usize,
        fresh: &mut Freshness<'_>,
    ) -> Option<Vec<Implementation>> {
        let mut out = Vec::new();
        for implementor in self.implementers[index].get(symbol).into_iter().flatten() {
            let Some(ty) = implementing_type(implementor) else {
                continue;
            };
            for site in self.definitions[index]
                .get(&**implementor)
                .into_iter()
                .flatten()
            {
                if !fresh.allows(&site.path) {
                    return None;
                }
                out.push(Implementation {
                    site: site.clone(),
                    ty: ty.to_string(),
                });
            }
        }
        Some(out)
    }

    /// その語の定義を索引から引く。
    ///
    /// 依拠したファイルが1つでも変わっていれば、行番号はもう索引の言うとおりではないので None。
    /// 変わっていない分だけを返すと、消えた候補があることを呼び出し側が知れないまま
    /// 「索引が答えた」と読まれる。
    pub(crate) fn definitions_in(&self, rel: &Path, span: Span) -> Option<Resolved> {
        if !self.is_current(rel) {
            return None;
        }
        let entry = self.docs.get(rel)?;
        let doc = parse_document(&self.bytes[entry.index][entry.span.clone()]).ok()?;

        // バイトオフセットが確定していない Document は、is_current で内容が索引生成時と
        // 同じだと確かめた今のソースを、変換・判定の根拠として読み直す。読めなければ
        // このファイル由来の答えは出さない（誤った列を Exact に混ぜないため）。
        let content = (entry.column_encoding != scip_split::ColumnEncoding::Utf8)
            .then(|| std::fs::read(self.root.join(rel)))
            .transpose()
            .ok()?;
        let lines = content.as_deref().map(Lines::of);
        let at = InDocument {
            doc: &doc,
            rel,
            encoding: entry.column_encoding,
            lines: lines.as_ref(),
        };

        let mut out: Vec<Location> = Vec::new();
        let mut symbols: Vec<&str> = Vec::new();
        // rel は上で検査済みなので、飛び先が同じファイルのときに読み直さない。
        let mut fresh = Freshness::starting_at(self, rel);
        for occ in &doc.occurrences {
            let Some(range) = usable_range(&occ.range, entry.column_encoding, lines.as_ref())
            else {
                continue;
            };
            if !contained_in(&range, span) {
                continue;
            }
            if !symbols.contains(&occ.symbol.as_str()) {
                symbols.push(&occ.symbol);
            }
            for loc in self.definitions_of(&occ.symbol, entry.index, &at) {
                if !fresh.allows(&loc.path) {
                    return None;
                }
                if !out.contains(&loc) {
                    out.push(loc);
                }
            }
        }
        if !out.is_empty() {
            out.sort_by(|a, b| position_key(a).cmp(&position_key(b)));
            return Some(Resolved::Direct(out));
        }

        // 語そのものの定義が 1 つも引けなかったときだけ、囲んでいる型に落とす。
        // 直接の定義があるのに混ぜると、強い主張だけだった答えに弱いものが紛れる。
        let mut enclosing: Vec<Enclosing> = Vec::new();
        for symbol in &symbols {
            let Some(ty) = enclosing_type(symbol) else {
                continue;
            };
            let Some(locations) = self.definitions[entry.index].get(ty.as_str()) else {
                continue;
            };
            for loc in locations {
                if !fresh.allows(&loc.path) {
                    return None;
                }
                enclosing.push(Enclosing {
                    definition: loc.clone(),
                    ty: SymbolId(ty.as_str().into()),
                });
            }
        }
        enclosing.sort_by(|a, b| position_key(&a.definition).cmp(&position_key(&b.definition)));
        (!enclosing.is_empty()).then_some(Resolved::Enclosing(enclosing))
    }

    /// その語への参照を索引から引く。definitions_in と対称だが、依拠集合に飛び先
    /// （定義）は入らない。参照先のファイルはどれも同じ扱いで依拠集合に入る。
    pub(crate) fn references_in(&self, rel: &Path, span: Span) -> Option<Found> {
        if !self.is_current(rel) {
            return None;
        }
        let entry = self.docs.get(rel)?;
        let doc = parse_document(&self.bytes[entry.index][entry.span.clone()]).ok()?;

        let content = (entry.column_encoding != scip_split::ColumnEncoding::Utf8)
            .then(|| std::fs::read(self.root.join(rel)))
            .transpose()
            .ok()?;
        let lines = content.as_deref().map(Lines::of);
        let at = InDocument {
            doc: &doc,
            rel,
            encoding: entry.column_encoding,
            lines: lines.as_ref(),
        };

        let mut out: Vec<Location> = Vec::new();
        let mut fresh = Freshness::starting_at(self, rel);
        let mut symbols: Vec<&str> = Vec::new();
        for occ in &doc.occurrences {
            let Some(range) = usable_range(&occ.range, entry.column_encoding, lines.as_ref())
            else {
                continue;
            };
            if !contained_in(&range, span) {
                continue;
            }
            if !symbols.contains(&occ.symbol.as_str()) {
                symbols.push(&occ.symbol);
            }
            for loc in self.references_of(&occ.symbol, entry.index, &at, &mut fresh)? {
                if !fresh.allows(&loc.path) {
                    return None;
                }
                if !out.contains(&loc) {
                    out.push(loc);
                }
            }
        }
        // 直接参照が 0 件でも、インタフェース経由があれば答えになる。むしろ 0 件のときが
        // この経路の効きどころで、Go の具象メソッドの 42.0% がそれに当たる。
        let mut via_interface: Vec<ViaInterface> = Vec::new();
        for symbol in &symbols {
            for interface in self.interfaces_of(symbol, entry.index) {
                let implementations = self.implementers[entry.index]
                    .get(interface)
                    .map_or(0, |found| found.len() as u32);
                for reference in self.references_of_symbol(interface, entry.index, &mut fresh)? {
                    if !fresh.allows(&reference.path) {
                        return None;
                    }
                    via_interface.push(ViaInterface {
                        reference,
                        interface_method: SymbolId(interface.clone()),
                        implementations,
                    });
                }
            }
        }

        // 転置索引は Document 番号の順に返し、その番号は投入時の HashMap の走査順で付く。
        // 並べ直さないと、同じ索引に同じことを聞いても実行のたびに順番が変わる。
        out.sort_by(|a, b| position_key(a).cmp(&position_key(b)));
        via_interface.sort_by(|a, b| position_key(&a.reference).cmp(&position_key(&b.reference)));

        (!out.is_empty() || !via_interface.is_empty()).then_some(Found {
            direct: out,
            via_interface,
        })
    }

    /// その符号が実装していると名乗っているインタフェース側の符号。上向き 1 ホップだけ。
    fn interfaces_of(&self, symbol: &str, index: usize) -> &[Box<str>] {
        self.implements[index]
            .get(symbol)
            .map(|v| &**v)
            .unwrap_or(&[])
    }

    /// そのファイルが索引生成時の内容のままか。読めなければ「そのままではない」とする。
    /// 出自を申告されていないファイルもここで落ちる。
    /// そのファイルが、索引を作った時点の内容のままか。
    ///
    /// `false` はそのファイルについて `Exact` が一切返らないことを意味する。
    /// 索引に無いファイルも `false` になる (どちらも「索引はこの内容を説明できない」で、
    /// 呼び出し側にとっては同じ)。組み込む側がこれを見るのは、索引が古いことを
    /// 画面に出すため。出さないと、ジャンプが構文層に落ちたことに気づけない。
    pub fn is_current(&self, rel: &Path) -> bool {
        let Some(recorded) = self.docs.get(rel).and_then(|e| e.source_hash.as_ref()) else {
            return false;
        };
        std::fs::read(self.root.join(rel)).is_ok_and(|src| blob_hash(&src) == *recorded)
    }

    /// ローカル変数のシンボルは Document の中でしか意味を持たない
    /// （`local 35` は別ファイルでは別物）ので、索引全体の表には入れず、その場で引く。
    fn definitions_of(&self, symbol: &str, index: usize, at: &InDocument<'_>) -> Vec<Location> {
        if is_local(symbol) {
            at.definitions(symbol)
        } else {
            self.definitions[index]
                .get(symbol)
                .cloned()
                .unwrap_or_default()
        }
    }

    /// ローカル変数のシンボルは definitions_of と同じ理由で Document 内だけで解決する。
    /// 転置索引にも local を入れないので、ここで解決するのは唯一の経路になる。
    fn references_of(
        &self,
        symbol: &str,
        index: usize,
        at: &InDocument<'_>,
        fresh: &mut Freshness<'_>,
    ) -> Option<Vec<Location>> {
        if is_local(symbol) {
            Some(at.references(symbol))
        } else {
            self.references_of_symbol(symbol, index, fresh)
        }
    }

    /// 非ローカルな符号への参照。転置索引から引くので Document の文脈が要らない。
    ///
    /// **答えが依拠したのは、転置索引が指した Document そのものである。** 出てきた位置だけを
    /// 検査すると、位置が 1 件も出なかった Document が検査を素通りし、そのぶん欠けた答えが
    /// Exact として通る。行が消えていれば occurrence は 1 件残らず落ちるので、これは
    /// ファイルを編集しただけで起きる。
    fn references_of_symbol(
        &self,
        symbol: &str,
        index: usize,
        fresh: &mut Freshness<'_>,
    ) -> Option<Vec<Location>> {
        let Some(doc_ids) = self.references[index].get(symbol) else {
            return Some(Vec::new());
        };
        let mut out = Vec::new();
        for path in doc_ids
            .iter()
            .filter_map(|&doc_id| self.doc_paths.get(doc_id as usize))
        {
            if !fresh.allows(path) {
                return None;
            }
            out.extend(self.references_in_document(symbol, path)?);
        }
        Some(out)
    }

    /// 転置索引が指す Document を1つデコードして、そのシンボルへの参照を集める。
    ///
    /// 数えられなければ None。列の数え方を宣言しない索引では、参照を数えるのに元のソースが
    /// 要る。読めないときに 0 件として返すと、0 件の Document は鮮度の検査に載らないので
    /// （検査は返ってきた位置ごとに回る）、その Document の参照だけが黙って欠けた答えが
    /// Exact として通り抜ける。
    fn references_in_document(&self, symbol: &str, path: &Path) -> Option<Vec<Location>> {
        let entry = self.docs.get(path)?;
        let doc = parse_document(&self.bytes[entry.index][entry.span.clone()]).ok()?;
        let content = (entry.column_encoding != scip_split::ColumnEncoding::Utf8)
            .then(|| std::fs::read(self.root.join(path)))
            .transpose()
            .ok()?;
        let lines = content.as_deref().map(Lines::of);

        Some(
            InDocument {
                doc: &doc,
                rel: path,
                encoding: entry.column_encoding,
                lines: lines.as_ref(),
            }
            .references(symbol),
        )
    }
}

/// local な符号を Document の中だけで解決するのに要る文脈。
///
/// local (`local 35`) は Document をまたぐと別物なので、索引全体の表には入れず、
/// その場でこの Document だけを見る。
struct InDocument<'a> {
    doc: &'a Document,
    rel: &'a Path,
    encoding: scip_split::ColumnEncoding,
    lines: Option<&'a Lines<'a>>,
}

impl InDocument<'_> {
    /// この Document の中でその符号を定義している位置。
    fn definitions(&self, symbol: &str) -> Vec<Location> {
        self.occurrences(symbol, true)
    }

    /// この Document の中でその符号を参照している位置。
    fn references(&self, symbol: &str) -> Vec<Location> {
        self.occurrences(symbol, false)
    }

    fn occurrences(&self, symbol: &str, definitions: bool) -> Vec<Location> {
        self.doc
            .occurrences
            .iter()
            .filter(|o| o.symbol == symbol && is_definition(o.symbol_roles) == definitions)
            .filter_map(|o| {
                let range = usable_range(&o.range, self.encoding, self.lines)?;
                location_of(&range, self.rel)
            })
            .collect()
    }
}

/// 索引全体の表に入れない符号か。Document をまたぐと別物になる。
fn is_local(symbol: &str) -> bool {
    symbol.starts_with("local ")
}

/// 答えが依拠したファイルの鮮度をまとめて見る。
///
/// 1 つでも索引生成時と違えば、その答えを丸ごと捨てる。残っている分だけを返すと、
/// 消えた候補があることが呼び出し側に見えないまま「索引が答えた」と読まれる。
///
/// 同じファイルを何度も読み直さないよう、判定を覚えておく。
struct Freshness<'a> {
    store: &'a Store,
    checked: HashMap<PathBuf, bool>,
}

impl<'a> Freshness<'a> {
    /// 聞かれたファイルは呼び出し前に検査済みなので、検査済みとして始める。
    fn starting_at(store: &'a Store, rel: &Path) -> Self {
        Freshness {
            store,
            checked: HashMap::from([(rel.to_path_buf(), true)]),
        }
    }

    /// その位置に依拠してよいか。false なら答えを丸ごと捨てる合図。
    fn allows(&mut self, path: &Path) -> bool {
        // 当たりが常態なので entry() は使わない。鍵を持たない引数で引けるぶん、
        // 参照 1 件ごとに PathBuf を確保せずに済む。
        if let Some(&known) = self.checked.get(path) {
            return known;
        }
        let ok = self.store.is_current(path);
        self.checked.insert(path.to_path_buf(), ok);
        ok
    }
}

fn position_key(loc: &Location) -> (&Path, u32, u32) {
    (loc.path.as_path(), loc.line, loc.col)
}

fn is_definition(roles: i32) -> bool {
    roles & SymbolRole::Definition as i32 != 0
}

fn parse_document(bytes: &[u8]) -> Result<Document> {
    Document::parse_from_bytes(bytes).map_err(|e| SheafError::Malformed(e.to_string()))
}
