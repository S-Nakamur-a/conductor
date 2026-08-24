//! sheaf: SCIP 索引をファイル単位に切って内容ハッシュで持ち、位置クエリに確信度つきで答える。
//!
//! 中心にあるのは [`Definition`] の形である。位置は必ず variant の中にあり、
//! 確信度を読まずに取り出す経路が存在しない。索引が古いことを申告できない答えは、
//! 無回答より悪い（信じて行動してしまう）ため、型で塞いでいる。

mod hash;
mod regenerate;
mod resolve;
mod store;
mod syntactic;

pub use hash::blob_hash;
pub use regenerate::{
    Outcome, Producer, Regenerated, Regenerator, RustAnalyzer, ScipGo, ScipTypescript, Target,
    generate_once, read_provenance, write_provenance,
};
pub use resolve::{definition_at, describe_at, implementations_at, references_at};
pub use store::{IndexSource, Slot, Store};
pub use syntactic::{SyntacticAnswer, SyntacticLayer, Token};

use std::path::PathBuf;

/// ソース上の位置。行と列はともに 0 始まり。
///
/// 列は UTF-8 のバイトオフセット。SCIP の仕様では UTF-16/UTF-32 コードユニットも
/// 選べるが、[`Store`] がここまでに直すか、直せない occurrence を答えから外している。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: u32,
    pub col: u32,
}

/// ソース上の範囲。行・列はともに 0 始まり、終端は含まない。
///
/// 語が複数行にまたがることは無いはずだが、それに依存しないよう行を持たせてある。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

/// 定義ジャンプの答え。
///
/// 位置を取り出すには必ずどの variant かを判定しなければならない。
///
/// ```
/// # use sheaf_core::{Definition, Location};
/// # let answer = Definition::NotCode;
/// match answer {
///     Definition::Exact(locs) => println!("{} 件（索引由来）", locs.len()),
///     Definition::Enclosing(found) => println!("{} を囲む型なら分かる", found.len()),
///     Definition::Syntactic(locs) => println!("{} 件（構文由来）", locs.len()),
///     Definition::NotCode => println!("識別子ではない"),
/// }
/// ```
///
/// 確信度を素通りして位置を取る書き方は用意していない。
///
/// ```compile_fail
/// # use sheaf_core::{Definition, Location};
/// let answer = Definition::NotCode;
/// let loc: Location = answer.location();
/// ```
///
/// ```compile_fail
/// # use sheaf_core::{Definition, Location};
/// let answer = Definition::NotCode;
/// let loc: Location = answer.into();
/// ```
///
/// [`Exact`](Definition::Exact) と [`Enclosing`](Definition::Enclosing) は別の型を持つので、
/// 素通しで混ぜられない。前者は聞かれた語そのものの定義で、後者はそれを囲む型の定義である。
///
/// ```compile_fail
/// # use sheaf_core::{Enclosing, Location};
/// # let found: Vec<Enclosing> = Vec::new();
/// let _: &Location = &found[0];
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Definition {
    /// 意味索引が答えた。聞かれた位置のファイルも飛び先のファイルも索引生成時のまま。必ず 1 件以上ある。
    ///
    /// 1 件とは限らない。同じ範囲にローカル変数の読みとフィールドの指定が両方乗ることがあり
    /// （`Struct { file_path }`）、片方だけ返すともう一方を黙って隠すことになる。
    Exact(Vec<Location>),
    /// **聞かれた語そのものの定義は索引に無い。** 代わりに、それを囲んでいる型の定義を返す。
    ///
    /// derive が作った impl がこれに当たる。`GitStatusMap::default()` の `default` は
    /// 索引に定義位置を持たないが、`GitStatusMap` は持っている。実測で、自クレートへの
    /// 参照 37,120 出現のうち 397 出現（1.1%）がこの形で回収できる。
    ///
    /// 飛び先は**クリックした語の定義ではない**。[`Exact`](Definition::Exact) と混ぜると、
    /// 弱い主張が強い主張に紛れるので、別の型で持たせてある。
    Enclosing(Vec<Enclosing>),
    /// 構文層が答えた。候補は 0〜N 件。
    Syntactic(Vec<Location>),
    /// その位置に識別子が無い。コメントや文字列リテラルの地の文。
    ///
    /// `Syntactic(vec![])`（探したが見つからない）とは別物として持つ。
    /// 誤った回答をしないためには、問いが成立していないことも言える必要がある。
    NotCode,
}

/// 語を囲んでいる型の定義 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enclosing {
    pub definition: Location,
    /// 囲んでいる型。
    ///
    /// この符号は索引に書いてあったものではなく、**シンボルIDから組み立てたもの**である。
    /// 同じモジュールに同名の型があると別物に届き得るので、利用側が何に飛ぶのかを
    /// 見せられるようにしてある。
    pub ty: SymbolId,
}

/// 参照検索の答え。
///
/// [`Definition`] と同じく、位置を取り出すには必ずどの variant かを判定しなければならない。
///
/// ```
/// # use sheaf_core::{References, Location};
/// # let answer = References::NotCode;
/// match answer {
///     References::Exact(found) => println!("{} 件（直接）", found.direct.len()),
///     References::Syntactic(locs) => println!("{} 件（構文由来）", locs.len()),
///     References::NotCode => println!("識別子ではない"),
/// }
/// ```
///
/// 確信度を素通りして位置を取る書き方は用意していない。
///
/// ```compile_fail
/// # use sheaf_core::{References, Location};
/// let answer = References::NotCode;
/// let v: Vec<Location> = answer;
/// ```
///
/// direct と via_interface は別の型なので、素通しで混ぜられない。主張の強さが違うので、
/// 混ぜられると弱いほうが強いほうに紛れる。
///
/// ```compile_fail
/// # use sheaf_core::{Found, Location};
/// # let found = Found { direct: Vec::new(), via_interface: Vec::new() };
/// let _: &Location = &found.via_interface[0];
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum References {
    /// 意味索引が答えた。依拠した全ファイルが索引生成時のまま。
    Exact(Found),
    /// 構文層が答えた。
    Syntactic(Vec<Location>),
    /// その位置に識別子が無い。
    NotCode,
}

/// 意味索引が答えた参照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// そのシンボル自身を指している occurrence。
    pub direct: Vec<Location>,
    /// そのシンボルが実装しているインタフェースメソッドが参照されている箇所。
    ///
    /// **その実装に到達するとは限らない。** 実測で、9 件返るうち 8 件が静的に
    /// 別の実装へ解決される例がある。「到達し得る箇所」として読むと誤りになるので、
    /// [`direct`](Found::direct) とは別の型で持たせて混ぜられないようにしてある。
    pub via_interface: Vec<ViaInterface>,
}

/// インタフェースメソッドの参照 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViaInterface {
    pub reference: Location,
    /// 参照されているインタフェースメソッド。
    pub interface_method: SymbolId,
    /// そのインタフェースメソッドを実装している型の数。
    ///
    /// 1 なら、その参照が届く先はこの実装しかない。多いほど、この実装に届く見込みは薄い
    /// （実測で 51 実装が 1 箇所の呼び出しを共有している例がある）。
    pub implementations: u32,
}

/// trait / interface の実装先の答え。
///
/// producer によって根拠の強さが違うので variant を分けてある。scip-go と
/// scip-typescript は SCIP の `relationships` に実装関係を書くので、それを
/// そのまま返せる。rust-analyzer は 1 件も書かない (実測: このリポジトリの索引で
/// SymbolInformation 22,434 件すべてが空) ので、符号の綴りから導出するしかない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Implementations {
    /// 索引が `relationships` に書いている実装先。0 件は「探したが無い」。
    Exact(Vec<Implementation>),
    /// 符号の綴りから導出した。0 件は「探したが無い」。
    ///
    /// 同名の trait がリポジトリに 2 つあると混ざる。飛び先は正しい impl ブロック
    /// だが、その trait が聞かれたものと同じとは限らない。
    Derived(Vec<Implementation>),
    /// 索引が答えられない。索引が無い、聞かれたファイルが索引生成時と違う、
    /// その位置に occurrence が無い、のいずれか。
    ///
    /// `Derived(vec![])` (探したが無い) とは別物として持つ。
    Unknown,
    /// その位置に識別子が無い。
    NotCode,
}

/// trait を実装している impl ブロック 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Implementation {
    /// impl ブロックの位置。型の定義ではなく `impl X for T` の行を指す。
    pub site: Location,
    /// 実装している型の綴り。符号の断片であって完全な符号ではない。
    pub ty: String,
}

/// 索引がそのシンボルについて書いている説明。
///
/// 位置についての主張ではないので [`Definition`] とは別の口にしてある。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolDetail {
    pub symbol: SymbolId,
    pub kind: SymbolKind,
    /// その語を囲んでいるものの綴り (`app::types::App`)。組み立てられない形では None。
    pub container: Option<String>,
    /// 索引が書いた宣言 (`fn token_at(&self, path: &Path) -> Token`)。
    /// 型は producer が解決したもので、ソースの字面ではない。
    pub signature: Option<String>,
    /// doc コメント。索引が持っていなければ空。
    pub documentation: Vec<String>,
}

/// シンボルの種別。
///
/// 言語をまたいで名前が要るので、綴りは Rust に寄せていない。Go の interface と
/// Rust の trait は分けてある — 実装を持てるかどうかが違い、ジャンプの意味も違う。
/// 読み取れなかった種別は [`Unknown`](SymbolKind::Unknown) にする。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    AssociatedType,
    Constant,
    Enum,
    EnumMember,
    Class,
    Field,
    Function,
    ImplBlock,
    Interface,
    Method,
    Module,
    Package,
    Parameter,
    SelfParameter,
    Static,
    Struct,
    Trait,
    TypeAlias,
    TypeParameter,
    Variable,
    /// 索引が種別を書いていないか、書いた番号が表に無かった。
    Unknown,
}

/// シンボルの識別子。中身の構造は後で決めるので、いまは SCIP のシンボル文字列を
/// 持つだけの newtype。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolId(Box<str>);

impl SymbolId {
    /// SCIP のシンボル文字列。何を経由したのかを利用側が見せるのに要る。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SheafError {
    #[error("索引を読めない: {0}")]
    Io(#[from] std::io::Error),
    #[error("索引の protobuf が壊れている: {0}")]
    Malformed(String),
    /// UTF-32 の列（変換未対応）、またはディスク上で UTF-8 として安全に読めない
    /// ソースを申告する索引は投入しない。黙って位置がずれるより、投入時に落ちるほうがよい。
    /// UTF-16 の列は [`Store::load`] が変換するので、ここでは弾かない。
    #[error(
        "列を安全に解釈できない索引 (text_document_encoding={metadata}, position_encoding={document})"
    )]
    UnsupportedEncoding { metadata: i32, document: i32 },
}

pub type Result<T> = std::result::Result<T, SheafError>;
