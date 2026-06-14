//! Systematic Cauchy Reed-Solomon over GF(2^8), from scratch.
//!
//! This is the algorithm `reed_solomon_erasure::galois_8::ReedSolomon` runs
//! internally, written out so the moving parts are visible:
//!
//! * The generator is an `n x k` matrix: the top `k x k` block is the identity
//!   (so the first `k` shards are the data unchanged - "systematic"), and the
//!   bottom `n - k` rows form a **Cauchy matrix** `C[r][j] = 1 / (x_r + y_j)`.
//!   Cauchy matrices are superregular: every square submatrix is invertible, which
//!   is the MDS property that makes "any k of n" recovery work.
//! * Encode = multiply the generator by the data (over GF(2^8)).
//! * Decode = take the `k` generator rows for the shards you still have, invert
//!   that `k x k` matrix (Gauss-Jordan over GF(2^8)), and multiply.
//!
//! Reaches Constellation's `(data=64, parity=192)` = 256 total, the GF(2^8)
//! ceiling.

#![allow(clippy::needless_range_loop)] // explicit indices read clearer for matrices

use super::gf256::{add, inv, mul};

/// A systematic Cauchy Reed-Solomon code over GF(2^8).
pub struct RsGf256 {
    data: usize,
    total: usize,
    /// `total x data` generator matrix (identity rows then Cauchy rows).
    gen: Vec<Vec<u8>>,
}

impl RsGf256 {
    /// Build a code with `data` data shards and `parity` parity shards.
    pub fn new(data: usize, parity: usize) -> Self {
        let total = data + parity;
        assert!(data > 0 && total <= 256, "GF(2^8) holds at most 256 shards");

        let mut gen = vec![vec![0u8; data]; total];
        // Top k x k: identity (systematic data shards pass through unchanged).
        for i in 0..data {
            gen[i][i] = 1;
        }
        // Bottom rows: Cauchy. Data points y_j = j, parity points x_r = data + r.
        // The two sets are disjoint, so x_r + y_j (XOR) is never zero -> inverse
        // always exists.
        for r in 0..parity {
            let x = (data + r) as u8;
            for j in 0..data {
                let y = j as u8;
                gen[data + r][j] = inv(add(x, y));
            }
        }
        Self { data, total, gen }
    }

    pub fn data(&self) -> usize {
        self.data
    }
    pub fn total(&self) -> usize {
        self.total
    }

    /// Encode in place. `shards` must have `total` entries; the first `data` are
    /// filled with equal-length data, and the parity slots are overwritten.
    pub fn encode(&self, shards: &mut [Vec<u8>]) {
        assert_eq!(shards.len(), self.total, "need exactly `total` shard slots");
        let len = shards[0].len();
        for r in self.data..self.total {
            let mut out = vec![0u8; len];
            for j in 0..self.data {
                let coef = self.gen[r][j];
                if coef == 0 {
                    continue;
                }
                let dj = &shards[j];
                for b in 0..len {
                    out[b] ^= mul(coef, dj[b]);
                }
            }
            shards[r] = out;
        }
    }

    /// Reconstruct the `data` data shards from any `>= data` surviving shards,
    /// each given as `(original_index, bytes)`. Returns `None` if too few survive.
    pub fn reconstruct_data(&self, present: &[(usize, Vec<u8>)]) -> Option<Vec<Vec<u8>>> {
        let k = self.data;
        if present.len() < k {
            return None;
        }
        let len = present[0].1.len();

        // Form the k x k matrix from the generator rows of any k present shards.
        let mut m = vec![vec![0u8; k]; k];
        for (r, (idx, _)) in present.iter().take(k).enumerate() {
            m[r] = self.gen[*idx].clone();
        }
        let minv = invert(&m)?;

        // data = M^-1 * received, computed per byte column.
        let recv: Vec<&Vec<u8>> = present.iter().take(k).map(|(_, d)| d).collect();
        let mut data = vec![vec![0u8; len]; k];
        for b in 0..len {
            for r in 0..k {
                let mut acc = 0u8;
                for c in 0..k {
                    let coef = minv[r][c];
                    if coef != 0 {
                        acc ^= mul(coef, recv[c][b]);
                    }
                }
                data[r][b] = acc;
            }
        }
        Some(data)
    }
}

/// Gauss-Jordan inverse of a square matrix over GF(2^8). `None` if singular.
fn invert(m: &[Vec<u8>]) -> Option<Vec<Vec<u8>>> {
    let n = m.len();
    let mut a: Vec<Vec<u8>> = m.to_vec();
    let mut out = vec![vec![0u8; n]; n];
    for i in 0..n {
        out[i][i] = 1;
    }

    for col in 0..n {
        // Find a nonzero pivot in this column.
        let mut pivot = col;
        while pivot < n && a[pivot][col] == 0 {
            pivot += 1;
        }
        if pivot == n {
            return None; // singular (cannot happen for a Cauchy submatrix)
        }
        a.swap(col, pivot);
        out.swap(col, pivot);

        // Scale the pivot row so a[col][col] == 1.
        let pinv = inv(a[col][col]);
        for j in 0..n {
            a[col][j] = mul(a[col][j], pinv);
            out[col][j] = mul(out[col][j], pinv);
        }

        // Eliminate this column from every other row.
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = a[r][col];
            if factor == 0 {
                continue;
            }
            for j in 0..n {
                a[r][j] ^= mul(factor, a[col][j]);
                out[r][j] ^= mul(factor, out[col][j]);
            }
        }
    }
    Some(out)
}
