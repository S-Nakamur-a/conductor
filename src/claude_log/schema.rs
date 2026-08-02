//! Claude Code の .jsonl セッションスキーマと一対一の、生の serde 型。

use serde::Deserialize;

/// Claude Code の .jsonl セッションファイルの1レコード。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub is_sidechain: bool,
    /// user ターンとして記録される、隠しコンテキストの注入。skill 定義の
    /// ダンプ（/skill 呼び出しは SKILL.md 全体を meta な user メッセージとして
    /// 追記する）、caveat バナー、単独の system reminder など。Claude Code の
    /// 実際の UI ではこれらを一切表示しないため、トランスクリプトでも
    /// スキップしないと、reflow ビューがユーザの見たことのない文字列の壁を
    /// 開いてしまう。
    #[serde(default)]
    pub is_meta: bool,
    /// /compact のサマリを次のコンテキストウィンドウに引き継ぐ疑似 user
    /// ターンに立つフラグ。Claude Code はこれを一切描画しない（実測: 再開後の
    /// トランスクリプトにサマリ本文はどこにも現れず、
    /// ⎿ Compacted (ctrl+o to see full summary) という行だけが出る）ため、
    /// reflow ビューもサマリ全体をユーザが打ったかのように再生するのではなく
    /// スキップする必要がある。
    ///
    /// 手元のコーパスではこのフラグは常に isVisibleInTranscriptOnly を伴う
    /// （102件中102件）。したがってこの1つだけを見ても等価であり、
    /// フィールド名がそのまま意味を表している。
    #[serde(default)]
    pub is_compact_summary: bool,
    /// type: "system" レコードの種別判別子。compact_boundary だけが
    /// ✻ Conversation compacted として表示される。
    #[serde(default)]
    pub subtype: Option<String>,
    /// type: "attachment" レコードに存在する。CLI がユーザの代わりに
    /// 会話へ注入したコンテキスト。Claude Code はこのうち2種類だけ ⎿ の
    /// 1行として描画する。Attachment 参照。
    #[serde(default)]
    pub attachment: Option<Attachment>,
    #[serde(default)]
    pub message: Option<Message>,
    /// レコードが書き込まれた RFC3339 の壁時計時刻。折りたたまれた Thinking
    /// ブロックの「Thought for Ns」の秒数を計算する（直前に *表示された*
    /// レコードのタイムスタンプとの差分を取る）ためだけに使う。session.rs
    /// 参照。古い/壊れたレコードには無いことがあり、その場合は固定で1秒に
    /// フォールバックする。
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// type: "attachment" レコードのペイロード。CLI がユーザの代わりに会話へ
/// 注入したコンテキスト（compact をまたいで持ち越されたファイル、hook の
/// 出力、skill 一覧など）。
///
/// Claude Code がこれらのうち実際に描画するのはほぼ無い。再開後の
/// トランスクリプトで実測したところ、可視の行になるのは以下2種類のみ。
///
/// * file → ⎿  Read {displayPath} ({numLines} lines)
/// * compact_file_reference → ⎿  Referenced file {displayPath}
///
/// 手元のコーパスには他に27種類の type がある（hook_success だけで
/// 約47000件出現する）が、どれも何かを描画する様子は観測されなかった。
/// そのためレンダラは上記2つの許可リストだけで扱い、残りをすべて
/// カバーしようとはしていない。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    #[serde(rename = "type", default)]
    pub kind: String,
    /// Claude Code が表示するとおりのパス。セッションの cwd からの相対パス
    /// に既になっている（cwd の外のファイルなら ../ が付く）ため、そのまま
    /// 使う。一部の type では無く、その場合は filename にフォールバックする。
    #[serde(default)]
    pub display_path: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub content: Option<AttachmentContent>,
}

/// file 添付における content のラッパー。
#[derive(Deserialize)]
pub struct AttachmentContent {
    #[serde(default)]
    pub file: Option<AttachmentFile>,
}

/// file 添付のファイルペイロード。表示するのは行数だけで、ファイルの
/// テキスト自体は既にトランスクリプトのコンテキストに入っているため描画しない。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentFile {
    #[serde(default)]
    pub num_lines: Option<u64>,
}

/// user / assistant レコードに存在する message フィールド。
#[derive(Deserialize)]
pub struct Message {
    #[serde(default)]
    pub role: Option<String>,
    /// assistant メッセージに存在するモデル名。
    #[serde(default)]
    pub model: Option<String>,
    pub content: Content,
}

/// content は素の文字列か、型付きブロックの配列のどちらか。
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

/// 配列形式の content 内にある、型付きブロック1つ。
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    /// thinking ブロック。ローカルのセッションログには（公開用に削除された
    /// ログと違い）thinking フィールドに推論テキスト全文が入っているので、
    /// それをそのまま取り込む。
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        /// API が呼び出しごとに振る id。パース中に対応する tool_result
        /// ブロックと tool_use_id で対応付けるためだけに使う（session.rs
        /// 参照）。壊れた/古いレコードでは無いことがあり、その場合は
        /// その呼び出しのペアリングが単に失敗するだけ。
        #[serde(default)]
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    ToolResult {
        /// この結果が応答している tool_use ブロックの id。
        #[serde(default)]
        tool_use_id: String,
        #[serde(default)]
        content: ToolResultContent,
        /// ツールがエラーを報告した場合に立つ。Claude Code のトランスクリプト
        /// を模してエラー色のコネクタで描画する。
        #[serde(default)]
        is_error: bool,
    },
    /// 上記で明示的に扱っていないブロック種別すべて。
    #[serde(other)]
    Other,
}

/// tool_result ブロックの content フィールド。素の文字列、テキストのみの
/// オブジェクトの配列、または無し、のいずれか。
#[derive(Deserialize, Default)]
#[serde(untagged)]
pub enum ToolResultContent {
    #[default]
    None,
    Text(String),
    Blocks(Vec<TextOnly>),
}

/// tool_result の content 配列に出てくる { "text": "..." } 形式のオブジェクト。
#[derive(Deserialize)]
pub struct TextOnly {
    #[serde(default)]
    pub text: String,
}
