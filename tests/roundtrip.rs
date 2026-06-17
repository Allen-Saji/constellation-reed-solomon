//! Round-trip correctness tests.
//!
//! Running the encoder's output back through the decoder and verifying the
//! original data is recovered is the core test of RS correctness. So every test
//! here encodes a pslice, simulates attester loss by dropping pshreds,
//! reconstructs, and asserts the original pslice comes back.

use constellation_reed_solomon::constellation::{DATA_SHARDS, EXPANSION};
use constellation_reed_solomon::{Attester, Error, Proposer, Pshred, Pslice, GAMMA_P, N_PSHREDS};

/// Deterministic pseudo-random bytes (xorshift) so tests need no rng dependency.
fn pseudo_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x & 0xff) as u8
        })
        .collect()
}

/// Keep `keep` pshreds chosen by a deterministic stride, drop the rest.
fn keep_subset(pshreds: &[Pshred], keep: usize, stride: usize) -> Vec<Pshred> {
    let stride = stride.max(1);
    let mut survivors = Vec::with_capacity(keep);
    let mut i = 0usize;
    while survivors.len() < keep {
        survivors.push(pshreds[i % pshreds.len()].clone());
        i += stride;
        // Guard against a stride that revisits indices before filling `keep`.
        if i > pshreds.len() * stride {
            break;
        }
    }
    // De-dup by index in case the stride wrapped.
    survivors.sort_by_key(|p| p.index);
    survivors.dedup_by_key(|p| p.index);
    survivors
}

#[test]
fn recovers_from_exactly_gamma_p_survivors() {
    let proposer = Proposer::new();
    let attester = Attester::new();
    let pslice = Pslice::new(pseudo_bytes(1, 4096));
    let pshreds = proposer.shred(&pslice).unwrap();

    // Keep exactly the last 64 (all parity-heavy) - the hardest realistic case.
    let survivors: Vec<Pshred> = pshreds[N_PSHREDS - GAMMA_P..].to_vec();
    assert_eq!(survivors.len(), GAMMA_P);

    let recovered = attester.reconstruct(&survivors).unwrap();
    assert_eq!(recovered, pslice);
}

#[test]
fn recovers_with_only_data_shards() {
    let proposer = Proposer::new();
    let attester = Attester::new();
    let pslice = Pslice::new(pseudo_bytes(2, 10_000));
    let pshreds = proposer.shred(&pslice).unwrap();

    // Drop every parity pshred; keep the 64 systematic ones (no work to do).
    let survivors: Vec<Pshred> = pshreds[..DATA_SHARDS].to_vec();
    let recovered = attester.reconstruct(&survivors).unwrap();
    assert_eq!(recovered, pslice);
}

#[test]
fn recovers_with_only_parity_shards() {
    let proposer = Proposer::new();
    let attester = Attester::new();
    let pslice = Pslice::new(pseudo_bytes(3, 10_000));
    let pshreds = proposer.shred(&pslice).unwrap();

    // Drop every systematic pshred; reconstruct purely from parity.
    let survivors: Vec<Pshred> = pshreds[DATA_SHARDS..DATA_SHARDS + GAMMA_P].to_vec();
    assert_eq!(survivors.len(), GAMMA_P);
    let recovered = attester.reconstruct(&survivors).unwrap();
    assert_eq!(recovered, pslice);
}

#[test]
fn fails_below_recovery_threshold() {
    let proposer = Proposer::new();
    let attester = Attester::new();
    let pslice = Pslice::new(pseudo_bytes(4, 4096));
    let pshreds = proposer.shred(&pslice).unwrap();

    // 63 survivors - one short of gamma_p. Must fail, not silently mis-decode.
    let survivors: Vec<Pshred> = pshreds[..GAMMA_P - 1].to_vec();
    match attester.reconstruct(&survivors) {
        Err(Error::Rs(reed_solomon_erasure::Error::TooFewShardsPresent)) => {}
        other => panic!("expected TooFewShardsPresent, got {other:?}"),
    }
}

#[test]
fn handles_many_pslice_sizes() {
    let proposer = Proposer::new();
    let attester = Attester::new();
    for &len in &[0usize, 1, 7, 63, 64, 65, 255, 1024, 65_536, 200_000] {
        let pslice = Pslice::new(pseudo_bytes(len as u64 + 7, len));
        let pshreds = proposer.shred(&pslice).unwrap();
        // Keep a spread-out 64 survivors.
        let survivors = keep_subset(&pshreds, GAMMA_P, 4);
        assert!(survivors.len() >= GAMMA_P, "stride sampling failed for len {len}");
        let recovered = attester.reconstruct(&survivors).unwrap();
        assert_eq!(recovered, pslice, "round-trip failed at pslice len {len}");
    }
}

#[test]
fn many_random_erasure_patterns() {
    let proposer = Proposer::new();
    let attester = Attester::new();
    let pslice = Pslice::new(pseudo_bytes(99, 8192));
    let pshreds = proposer.shred(&pslice).unwrap();

    // 200 deterministic "random" erasure patterns: each keeps a different 64.
    for trial in 0..200u64 {
        let mut order: Vec<usize> = (0..N_PSHREDS).collect();
        // Fisher-Yates with a deterministic LCG.
        let mut state = trial.wrapping_mul(6364136223846793005).wrapping_add(1);
        for i in (1..order.len()).rev() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let j = (state >> 33) as usize % (i + 1);
            order.swap(i, j);
        }
        let survivors: Vec<Pshred> = order[..GAMMA_P].iter().map(|&i| pshreds[i].clone()).collect();
        let recovered = attester.reconstruct(&survivors).unwrap();
        assert_eq!(recovered, pslice, "trial {trial} failed");
    }
}

#[test]
fn expansion_is_4x() {
    let proposer = Proposer::new();
    let pslice = Pslice::new(pseudo_bytes(5, 6400));
    let pshreds = proposer.shred(&pslice).unwrap();
    let encoded_bytes: usize = pshreds.iter().map(|p| p.bytes.len()).sum();
    let shard_len = pshreds[0].bytes.len();
    // Total encoded bytes / systematic (data) bytes == Gamma_p / gamma_p == 4.
    assert_eq!(encoded_bytes, N_PSHREDS * shard_len);
    assert_eq!(encoded_bytes / (DATA_SHARDS * shard_len), EXPANSION);
}

/// Property-based round-trip. The assignment's tip says round-tripping is the
/// only meaningful test of RS correctness, so this asserts it as a universally
/// quantified property: for any pslice and any choice of exactly `gamma_p`
/// surviving pshreds, the decoder returns the exact original bytes. This widens
/// the deterministic tests above to randomised sizes and erasure patterns.
mod property {
    use super::*;
    use proptest::prelude::*;
    use std::sync::OnceLock;

    // Build the (64, 192) encoder/decoder once and share it across all cases;
    // their caches memoise the ReedSolomon matrix, so we pay for it once rather
    // than rebuilding it per case.
    fn proposer() -> &'static Proposer {
        static P: OnceLock<Proposer> = OnceLock::new();
        P.get_or_init(Proposer::new)
    }
    fn attester() -> &'static Attester {
        static A: OnceLock<Attester> = OnceLock::new();
        A.get_or_init(Attester::new)
    }

    proptest! {
        // 64 cases keeps this snappy; size is bounded since the property under
        // test is the erasure pattern, not large payloads (the deterministic
        // tests above already cover sizes up to 200 KB).
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn roundtrip_any_pslice_any_64_survivors(
            data in proptest::collection::vec(any::<u8>(), 0..=8_192),
            survivors in proptest::sample::subsequence(
                (0..N_PSHREDS).collect::<Vec<usize>>(),
                GAMMA_P,
            ),
        ) {
            let pslice = Pslice::new(data);
            let pshreds = proposer().shred(&pslice).unwrap();
            let kept: Vec<Pshred> = survivors.iter().map(|&i| pshreds[i].clone()).collect();
            let recovered = attester().reconstruct(&kept).unwrap();
            prop_assert_eq!(recovered, pslice);
        }
    }
}
