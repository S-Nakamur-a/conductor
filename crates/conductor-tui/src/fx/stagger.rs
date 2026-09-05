//! 複数の矩形へ順に効かせるための、矩形ごとの開始時点。

use std::time::{SystemTime, UNIX_EPOCH};

/// 矩形ごとの開始時点 (進捗の割合)。左から順に、少しずつ遅れて始まる。
#[derive(Debug, Clone, PartialEq)]
pub struct Stagger {
    offsets: Vec<f64>,
}

impl Stagger {
    /// `step` ずつずらし、±`jitter` のゆらぎを乗せる。順序は入れ替わらない。
    pub fn jittered(count: usize, step: f64, jitter: f64) -> Self {
        Self::from_jitter(step, &random_jitter(count, jitter))
    }

    /// ゆらぎ幅がずらしより広いと素朴な加算では順序が入れ替わるので、前の矩形より
    /// 早く始まらないよう押し出す。
    pub(super) fn from_jitter(step: f64, jitter: &[f64]) -> Self {
        let mut offsets = Vec::with_capacity(jitter.len());
        let mut prev = f64::NEG_INFINITY;
        for (i, j) in jitter.iter().enumerate() {
            let v = (step * i as f64 + j).max(prev);
            offsets.push(v);
            prev = v;
        }
        Self { offsets }
    }

    /// 矩形 i にとっての進捗。offsets のぶん遅れて始まり、最後の矩形が 1.0 で終わる。
    pub fn local(&self, progress: f64, i: usize) -> f64 {
        let offset = self.offsets.get(i).copied().unwrap_or(0.0);
        ((progress - offset) / (1.0 - self.last()).max(0.1)).clamp(0.0, 1.0)
    }

    /// 最後の矩形が始まる時点。
    pub fn last(&self) -> f64 {
        self.offsets.last().copied().unwrap_or(0.0)
    }

    #[cfg(test)]
    pub fn uniform(count: usize) -> Self {
        Self {
            offsets: vec![0.0; count],
        }
    }
}

fn random_jitter(count: usize, span: f64) -> Vec<f64> {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (0..count as u32)
        // 起動ごとに違えばよいだけなので、質のよい乱数は要らない。
        .map(|i| (f64::from(seed.rotate_left(i * 8) % 1000) / 1000.0 * 2.0 - 1.0) * span)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ゆらぎは指定した幅に収まる() {
        let got = random_jitter(5, 0.0625);
        assert_eq!(got.len(), 5);
        for j in got {
            assert!(j.abs() <= 0.0625, "{j} が幅を超えた");
        }
    }

    /// 入れ替わると「毎回違う順で開く」ことになり、固定順を選んだ意図と食い違う。
    #[test]
    fn ゆらぎが広くても順序は入れ替わらない() {
        let wide = 0.32;
        for case in [
            vec![wide, -wide, wide, -wide],
            vec![-wide, wide, -wide, wide],
            vec![0.0, 0.0, 0.0, 0.0],
        ] {
            let got = Stagger::from_jitter(0.08, &case);
            for pair in got.offsets.windows(2) {
                assert!(
                    pair[0] <= pair[1],
                    "順序が入れ替わった: {got:?} (jitter={case:?})"
                );
            }
        }
    }
}
