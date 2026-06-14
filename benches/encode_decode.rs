//! Encode + recover benchmarks, extending Agave PR #5695's structure.
//!
//! PR #5695 ("adds benchmarks for recovering (chained) Merkle shreds from erasure
//! codes", anza-xyz/agave) added a Criterion bench to
//! `ledger/benches/make_shreds_from_entries.rs` with this shape:
//!
//! ```text
//! fn run_recover_shreds(name, c, num_packets, num_code, is_last_in_slot) {
//!     // build shreds, isolate one FEC set, drop `num_code` shreds, then:
//!     c.bench_function(name, |b| b.iter(|| recover(shreds.clone())));
//! }
//! fn bench_recover_shreds(c) {
//!     for is_last_in_slot in [false, true] {
//!         for num_packets in [28, 32, 48, 56] {
//!             for num_code in [1, 8, 16, 32] { run_recover_shreds(..) }}}
//! }
//! ```
//!
//! Here we keep the same `run_* / bench_* / criterion_group!` structure and the
//! same "drop N shards, then recover" methodology, but extend the parameter sweep
//! to cover Constellation's `(data=64, parity=192)` alongside Agave's status-quo
//! `(32, 32)`, operating directly on the `reed_solomon_erasure::galois_8`
//! encoder that Agave's `ReedSolomonCache` wraps.

#![allow(clippy::arithmetic_side_effects)]

use std::hint::black_box;
use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use reed_solomon_erasure::galois_8::ReedSolomon;

/// Deterministic shard payloads (xorshift) - reproducible, no rng dependency.
fn make_shards(data: usize, parity: usize, shard_len: usize) -> Vec<Vec<u8>> {
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    let mut shards = Vec::with_capacity(data + parity);
    for _ in 0..data {
        let shard = (0..shard_len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x & 0xff) as u8
            })
            .collect();
        shards.push(shard);
    }
    shards.extend((0..parity).map(|_| vec![0u8; shard_len]));
    shards
}

/// Mirror of PR #5695's `run_recover_shreds`: encode, drop `num_lost` shards
/// (data-first, to force genuine reconstruction work), then bench `reconstruct`.
fn run_recover(
    name: &str,
    c: &mut Criterion,
    rs: &Arc<ReedSolomon>,
    data: usize,
    parity: usize,
    shard_len: usize,
    num_lost: usize,
) {
    let mut shards = make_shards(data, parity, shard_len);
    rs.encode(&mut shards).unwrap();

    // Drop the first `num_lost` shards (data shards first) so the decoder always
    // has real work to do - dropping only parity would be a no-op recovery.
    let mut lossy: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
    for slot in lossy.iter_mut().take(num_lost) {
        *slot = None;
    }

    c.bench_function(name, |b| {
        b.iter(|| {
            let mut work = lossy.clone();
            rs.reconstruct_data(&mut work).unwrap();
            black_box(work);
        })
    });
}

/// The "make shreds" analog: bench a full encode (pslice -> 256 pshreds).
fn run_encode(name: &str, c: &mut Criterion, rs: &Arc<ReedSolomon>, data: usize, parity: usize, shard_len: usize) {
    c.bench_function(name, |b| {
        b.iter(|| {
            let mut shards = make_shards(data, parity, shard_len);
            rs.encode(&mut shards).unwrap();
            black_box(shards);
        })
    });
}

/// (label, data shards, parity shards, loss sweep) - status quo vs Constellation.
const CONFIGS: &[(&str, usize, usize, &[usize])] = &[
    // Agave Turbine today: 32 data : 32 coding. PR #5695 swept num_code [1,8,16,32].
    ("turbine_32_32", 32, 32, &[1, 8, 16, 32]),
    // Constellation: 64 data : 192 parity = 256 total (the GF(2^8) ceiling).
    ("constellation_64_192", 64, 192, &[1, 32, 64, 128, 192]),
];

fn bench_recover(c: &mut Criterion) {
    let shard_len = 1024;
    for (label, data, parity, losses) in CONFIGS {
        let rs = Arc::new(ReedSolomon::new(*data, *parity).unwrap());
        for &num_lost in *losses {
            let name = format!("recover_{label}_len{shard_len}_lost{num_lost}");
            run_recover(&name, c, &rs, *data, *parity, shard_len, num_lost);
        }
    }
}

fn bench_encode(c: &mut Criterion) {
    for shard_len in [256usize, 1024] {
        for (label, data, parity, _) in CONFIGS {
            let rs = Arc::new(ReedSolomon::new(*data, *parity).unwrap());
            let name = format!("encode_{label}_len{shard_len}");
            run_encode(&name, c, &rs, *data, *parity, shard_len);
        }
    }
}

criterion_group!(benches, bench_encode, bench_recover);
criterion_main!(benches);
