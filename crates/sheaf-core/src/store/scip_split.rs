//! 索引ファイルを Document 単位のバイト範囲に切る。
//!
//! `Index` 全体を protobuf でデコードしない。実測で 12.4MB の索引が常駐 86.9MB になり、
//! 範囲を切り出すだけなら 17.0MB で済む。常駐メモリを増やさないことが sheaf の存在理由なので、
//! ここで 7 倍払うわけにいかない。
//!
//! rust-protobuf の `CodedInputStream` は「公開 API ではない」と上流が明言しているので、
//! 走査は自前で持つ。切り出したあとの型付きアクセスは scip crate に任せる。

use crate::{Result, SheafError};
use std::ops::Range;

const WIRE_VARINT: u8 = 0;
const WIRE_I64: u8 = 1;
const WIRE_LEN: u8 = 2;
const WIRE_I32: u8 = 5;

// Index のフィールド番号
const INDEX_METADATA: u64 = 1;
const INDEX_DOCUMENTS: u64 = 2;
// Document のフィールド番号
const DOC_RELATIVE_PATH: u64 = 1;
const DOC_POSITION_ENCODING: u64 = 6;
// Metadata のフィールド番号
const META_TEXT_DOCUMENT_ENCODING: u64 = 4;

// TextEncoding と PositionEncoding は別の enum だが、0=Unspecified / 1=UTF8 / 2=UTF16
// という並びは共通している。
const ENCODING_UNSPECIFIED: i32 = 0;
const ENCODING_UTF8: i32 = 1;
const ENCODING_UTF16: i32 = 2;
#[cfg(test)]
const ENCODING_UTF32: i32 = 3;

fn varint(buf: &[u8], pos: &mut usize) -> Result<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *buf
            .get(*pos)
            .ok_or_else(|| SheafError::Malformed(format!("varint が {} で途切れた", pos)))?;
        *pos += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            return Err(SheafError::Malformed("varint が 10 バイトを超えた".into()));
        }
    }
}

/// 固定長フィールドを読み飛ばす。境界を検査しないと、途中で切れた索引が
/// エラーにならずに「そこまでで終わり」として通ってしまう。
fn skip_fixed(buf: &[u8], pos: &mut usize, width: usize) -> Result<Option<Range<usize>>> {
    *pos = pos
        .checked_add(width)
        .filter(|e| *e <= buf.len())
        .ok_or_else(|| SheafError::Malformed("固定長フィールドが本体を超えている".into()))?;
    Ok(None)
}

/// 現在位置のフィールドを読み飛ばす。長さ限定フィールドなら中身の範囲を返す。
fn skip_field(buf: &[u8], pos: &mut usize, wire: u8) -> Result<Option<Range<usize>>> {
    match wire {
        WIRE_VARINT => {
            varint(buf, pos)?;
            Ok(None)
        }
        WIRE_I64 => skip_fixed(buf, pos, 8),
        WIRE_I32 => skip_fixed(buf, pos, 4),
        WIRE_LEN => {
            let len = varint(buf, pos)? as usize;
            let start = *pos;
            let end = start
                .checked_add(len)
                .filter(|e| *e <= buf.len())
                .ok_or_else(|| SheafError::Malformed("長さが本体を超えている".into()))?;
            *pos = end;
            Ok(Some(start..end))
        }
        other => Err(SheafError::Malformed(format!("未知の wire type {other}"))),
    }
}

pub(super) struct Split {
    pub metadata: Option<Range<usize>>,
    pub documents: Vec<Range<usize>>,
}

/// 索引のトップレベルを1回歩いて、metadata と各 Document のバイト範囲を得る。
pub(super) fn split(buf: &[u8]) -> Result<Split> {
    let mut pos = 0usize;
    let mut out = Split {
        metadata: None,
        documents: Vec::new(),
    };
    while pos < buf.len() {
        let tag = varint(buf, &mut pos)?;
        let field = tag >> 3;
        let wire = (tag & 7) as u8;
        let span = skip_field(buf, &mut pos, wire)?;
        match (field, span) {
            (INDEX_METADATA, Some(r)) => out.metadata = Some(r),
            (INDEX_DOCUMENTS, Some(r)) => out.documents.push(r),
            _ => {}
        }
    }
    Ok(out)
}

/// Document のトップレベルを歩いて、相対パスと列エンコーディングだけを取る。
///
/// occurrence を1件もデコードしないので、345 Document で 50µs 程度で済む。
pub(super) fn document_header(buf: &[u8]) -> Result<(String, i32)> {
    let mut pos = 0usize;
    let mut path = None;
    let mut encoding = ENCODING_UNSPECIFIED;
    while pos < buf.len() {
        let tag = varint(buf, &mut pos)?;
        let field = tag >> 3;
        let wire = (tag & 7) as u8;
        if field == DOC_POSITION_ENCODING && wire == WIRE_VARINT {
            encoding = varint(buf, &mut pos)? as i32;
            continue;
        }
        // let-chain で書かないのは rust-version 1.85 を守るため（安定化は 1.88）。
        if let (DOC_RELATIVE_PATH, Some(r)) = (field, skip_field(buf, &mut pos, wire)?) {
            path = Some(String::from_utf8_lossy(&buf[r]).into_owned());
        }
    }
    path.map(|p| (p, encoding))
        .ok_or_else(|| SheafError::Malformed("Document に relative_path が無い".into()))
}

/// metadata から text_document_encoding だけを取る。
pub(super) fn metadata_encoding(buf: &[u8]) -> Result<i32> {
    let mut pos = 0usize;
    let mut encoding = ENCODING_UNSPECIFIED;
    while pos < buf.len() {
        let tag = varint(buf, &mut pos)?;
        let field = tag >> 3;
        let wire = (tag & 7) as u8;
        if field == META_TEXT_DOCUMENT_ENCODING && wire == WIRE_VARINT {
            encoding = varint(buf, &mut pos)? as i32;
            continue;
        }
        skip_field(buf, &mut pos, wire)?;
    }
    Ok(encoding)
}

/// Document.position_encoding から分かる、occurrence の列を実際にどう扱えるか。
///
/// Unspecified は「宣言していない」であって「バイトオフセットである」ではない。
/// scip-go は宣言せずにバイトオフセットを出すが、scip-typescript は宣言せずに
/// UTF-16 コードユニットオフセットを出す。索引の中にはこの2つを区別する手段が無いので、
/// Unspecified を一律 UTF-8 とは決め打ちできない。occurrence ごとに Store 側で判定する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ColumnEncoding {
    /// バイトオフセットとして確定している。
    Utf8,
    /// UTF-16 コードユニットオフセットとして確定している。Store がバイトへ変換する。
    Utf16,
    /// 未指定。occurrence ごとに、開始位置より前が全部 ASCII かどうかで使えるかを判定する。
    Ambiguous,
}

/// Document.position_encoding を読む。UTF-32 は変換を実装していないのでここで弾く。
pub(super) fn resolve_column_encoding(document: i32) -> Result<ColumnEncoding> {
    match document {
        ENCODING_UNSPECIFIED => Ok(ColumnEncoding::Ambiguous),
        ENCODING_UTF8 => Ok(ColumnEncoding::Utf8),
        ENCODING_UTF16 => Ok(ColumnEncoding::Utf16),
        other => Err(SheafError::UnsupportedEncoding {
            metadata: 0,
            document: other,
        }),
    }
}

/// metadata.text_document_encoding を確かめる。
///
/// この値は relative_path が指すディスク上のファイルの符号化の話であり、
/// occurrence の列の数え方（Document.position_encoding）とは無関係（SCIP 仕様）。
/// ただしファイルが UTF-8 で符号化されていなければ sheaf はバイト列として安全に
/// 読めない（ハッシュも位置もそもそも成立しない）ので、索引全体をここで弾く。
pub(super) fn check_text_encoding(metadata: i32) -> Result<()> {
    match metadata {
        ENCODING_UNSPECIFIED | ENCODING_UTF8 => Ok(()),
        other => Err(SheafError::UnsupportedEncoding {
            metadata: other,
            document: 0,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varintは複数バイトを読み途切れた入力を拒む() {
        for (buf, want) in [
            (&[0x96u8, 0x01][..], Some((150u64, 2usize))),
            (&[0x96u8][..], None),
        ] {
            let mut pos = 0;
            let got = varint(buf, &mut pos).ok().map(|v| (v, pos));
            assert_eq!(got, want, "{buf:?}");
        }
    }

    #[test]
    fn バッファを越える長さは壊れているとみなす() {
        // field 2, wire 2, 長さ 100 だが本体は 1 バイトしかない
        let mut pos = 0;
        let buf = [0x12u8, 100, 0x00];
        let tag = varint(&buf, &mut pos).unwrap();
        assert!(skip_field(&buf, &mut pos, (tag & 7) as u8).is_err());
    }

    #[test]
    fn documentの符号化が桁の意味を決める() {
        assert_eq!(
            resolve_column_encoding(ENCODING_UNSPECIFIED).unwrap(),
            ColumnEncoding::Ambiguous
        );
        assert_eq!(
            resolve_column_encoding(ENCODING_UTF8).unwrap(),
            ColumnEncoding::Utf8
        );
        assert_eq!(
            resolve_column_encoding(ENCODING_UTF16).unwrap(),
            ColumnEncoding::Utf16
        );
        assert!(resolve_column_encoding(ENCODING_UTF32).is_err());
    }

    #[test]
    fn utf8でないディスク上の符号化は拒む() {
        assert!(check_text_encoding(ENCODING_UNSPECIFIED).is_ok());
        assert!(check_text_encoding(ENCODING_UTF8).is_ok());
        assert!(check_text_encoding(ENCODING_UTF16).is_err());
        assert!(check_text_encoding(ENCODING_UTF32).is_err());
    }
}
