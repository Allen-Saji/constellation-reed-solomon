//! # Constellation erasure encoder
//!
//! A Constellation-parameterised Reed-Solomon erasure encoder, built on the same
//! crate Agave uses (`reed-solomon-erasure`, `galois_8::ReedSolomon`), and
//! re-parameterised from Agave's current Turbine FEC sizing to Solana
//! Constellation's proposer/attester sizing.
//!
//! ## What this is
//!
//! Constellation (Anza's Multiple Concurrent Proposers proposal) erasure-codes a
//! proposer's transaction batch (a *pslice*) into 256 pieces (*pshreds*), one per
//! attester, such that **any 64** reconstruct the pslice. That is a Reed-Solomon
//! `(data=64, parity=192)` code over GF(2^8): 256 total shards, the *exact* ceiling
//! of the 8-bit field.
//!
//! ## Grounding in Agave (read before the code)
//!
//! Today Agave shreds at `DATA_SHREDS_PER_FEC_BLOCK = 32` and
//! `CODING_SHREDS_PER_FEC_BLOCK = 32` (32:32 = 64 per FEC set), and caches one
//! `ReedSolomon` encoder per `(data, parity)` pair in `ReedSolomonCache`
//! (`ledger/src/shredder.rs`). This crate keeps that cache shape and swaps the
//! parameters to Constellation's `(64, 192)`.
//!
//! | Concept | Agave Turbine (today) | Constellation (this crate) |
//! |---|---|---|
//! | data shards | 32 | 64  (`gamma_p`, recovery threshold) |
//! | coding/parity shards | 32 | 192 |
//! | total shards | 64 | 256 (`Gamma_p`, one per attester) |
//! | recover from | any 32 | any 64 |
//! | expansion | 2x | 4x |
//!
//! ## Vocabulary (Constellation whitepaper, Anza, v0.9 2026-03-25)
//!
//! * **pslice** - a proposer's batch of accepted transactions (the data to encode).
//! * **pshred** - one erasure-coded piece of a pslice (the "p" is silent, to
//!   distinguish it from Alpenglow's shreds). There are `Gamma_p = 256`.
//! * **proposer** - builds a pslice and shreds it.
//! * **attester** - holds one pshred; any `gamma_p = 64` of them reconstruct.

pub mod constellation;
pub mod internals;

pub use constellation::{
    Attester, Error, Proposer, Pshred, Pslice, ReedSolomonCache, GAMMA_P, N_PSHREDS, PARITY_SHARDS,
    Q_ATTESTERS,
};

/// A deterministic demo pslice of `len` bytes (xorshift PRNG).
///
/// Shared by the `proposer` and `attester` binaries so the receiver can
/// regenerate the exact payload and assert that reconstruction matched, without
/// the two processes exchanging the original out-of-band.
pub fn demo_pslice(len: usize) -> Pslice {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15 ^ (len as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
    let bytes = (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x & 0xff) as u8
        })
        .collect::<Vec<u8>>();
    Pslice(bytes)
}
