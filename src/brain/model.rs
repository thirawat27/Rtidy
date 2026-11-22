//! Online logistic regression: a few floats, learns one example at a time.
//! Tiny on purpose — it runs in microseconds and never needs a GPU or a download.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Logit {
    pub w: Vec<f64>,
    /// examples learned so far
    pub n: u64,
    /// running accuracy (EWMA of correct predictions on unseen examples)
    pub acc: f64,
}

pub fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z.clamp(-30.0, 30.0)).exp())
}

impl Logit {
    /// Start from hand-set prior weights so predictions are sensible on day one.
    pub fn with_prior(prior: &[f64]) -> Self {
        Self { w: prior.to_vec(), n: 0, acc: 0.5 }
    }

    /// `x[0]` must be 1.0 (bias).
    pub fn predict(&self, x: &[f64]) -> f64 {
        sigmoid(self.w.iter().zip(x).map(|(w, x)| w * x).sum())
    }

    /// One step of SGD on log-loss with light L2 (bias not regularised).
    /// The learning rate decays so the model settles as it sees more of your habits.
    pub fn learn(&mut self, x: &[f64], y: bool) {
        if x.len() != self.w.len() {
            return;
        }
        let p = self.predict(x);
        let correct = (p >= 0.5) == y;
        self.acc = 0.95 * self.acc + 0.05 * if correct { 1.0 } else { 0.0 };
        let lr = 0.3 / (1.0 + self.n as f64 / 40.0).sqrt();
        let g = p - if y { 1.0 } else { 0.0 };
        for (i, w) in self.w.iter_mut().enumerate() {
            let reg = if i == 0 { 0.0 } else { 1e-3 * *w };
            *w -= lr * (g * x[i] + reg);
        }
        self.n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_a_simple_rule() {
        // y = (x1 < 0.5): starts with a wrong prior and must flip it.
        let mut m = Logit::with_prior(&[0.0, 2.0]);
        let mut seed = 7u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f64 / (1u64 << 31) as f64
        };
        for _ in 0..3000 {
            let x1 = rnd();
            m.learn(&[1.0, x1], x1 < 0.5);
        }
        assert!(m.predict(&[1.0, 0.1]) > 0.7, "{:?}", m.w);
        assert!(m.predict(&[1.0, 0.9]) < 0.3, "{:?}", m.w);
        assert!(m.acc > 0.8, "acc {}", m.acc);
    }
}
