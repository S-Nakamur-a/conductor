//! OSC 8 のリンク。セルは番号だけを持ち、URL は画面ごとの表に置く。

use std::collections::HashMap;
use std::num::NonZeroU16;
use std::sync::Arc;

pub(crate) type LinkId = NonZeroU16;

/// vte が 1 つの OSC で割る params の上限。達したら後ろの `;` 以降は捨てられている。
const MAX_OSC_PARAMS: usize = 16;
/// vte (既定の no_std) が `;` を除いた OSC の中身を溜める上限。満ちたら後ろは黙って捨てる。
const MAX_OSC_RAW: usize = 1024;

#[derive(Clone, Debug, Default)]
pub(crate) struct Links {
    urls: Vec<Arc<str>>,
    ids: HashMap<Arc<str>, LinkId>,
}

impl Links {
    pub(crate) fn url(&self, id: LinkId) -> &str {
        &self.urls[usize::from(id.get()) - 1]
    }

    /// OSC 8 (先頭の `8` を含む params) を受けて、以後の字に付けるリンクを返す。
    /// 途中までの URL を開かせるより開かせない方がよいので、欠けたかもしれない URL
    /// と外の端末へ書き戻せない字を含む URL はリンクにしない。
    pub(crate) fn open(&mut self, params: &[&[u8]]) -> Option<LinkId> {
        let raw_len: usize = params.iter().map(|param| param.len()).sum();
        if params.len() >= MAX_OSC_PARAMS || raw_len >= MAX_OSC_RAW {
            return None;
        }
        let url = params.get(2..).unwrap_or_default().join(&b';');
        let url = std::str::from_utf8(&url).ok()?;
        if url.is_empty() || url.chars().any(char::is_control) {
            return None;
        }
        self.intern(url)
    }

    /// 番号を使い回すと別の URL を開くので、尽きたらリンクにしない。
    fn intern(&mut self, url: &str) -> Option<LinkId> {
        if let Some(&id) = self.ids.get(url) {
            return Some(id);
        }
        let id = LinkId::new(u16::try_from(self.urls.len() + 1).ok()?)?;
        let url: Arc<str> = url.into();
        self.urls.push(Arc::clone(&url));
        self.ids.insert(url, id);
        Some(id)
    }

    pub(crate) fn write_switch(
        &self,
        contents: &mut Vec<u8>,
        to: Option<LinkId>,
    ) {
        contents.extend_from_slice(b"\x1b]8;;");
        if let Some(id) = to {
            contents.extend_from_slice(self.url(id).as_bytes());
        }
        contents.extend_from_slice(b"\x1b\\");
    }
}

#[cfg(test)]
mod tests;
