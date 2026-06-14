//! Reed-Solomon from scratch over GF(2^8).
//!
//! The main crate builds on the `reed-solomon-erasure` crate. This module is
//! **not** that - it reimplements what the crate's `galois_8` backend does
//! internally, to show the machinery rather than treat it as a black box:
//!
//! * [`gf256`] - GF(2^8) field arithmetic (XOR add, log/exp-table multiply,
//!   inverse), with the irreducible polynomial `0x11D` and primitive element
//!   `0x02`, matching the standard Reed-Solomon field convention.
//! * [`rs`] - a systematic Cauchy Reed-Solomon code built on that field: encode
//!   `k` data shards into `n` total, reconstruct from any `k`, via a Gauss-Jordan
//!   matrix inverse over GF(2^8).
//!
//! It round-trips at the same `(64, 192)` Constellation parameters as the main
//! crate path - useful as an independent cross-check of correctness.

pub mod gf256;
pub mod rs;
