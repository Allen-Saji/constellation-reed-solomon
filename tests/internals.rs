//! Tests for the from-scratch GF(2^8) + Reed-Solomon bonus module.

use adv_svm_erasure_lab::internals::gf256;
use adv_svm_erasure_lab::internals::rs::RsGf256;

#[test]
fn gf_boundary_values() {
    // The five canonical boundary cases (irreducible 0x11D, generator 0x02).
    assert_eq!(gf256::mul(0x00, 0xAB), 0x00, "zero annihilates");
    assert_eq!(gf256::mul(0x01, 0xAB), 0xAB, "one is identity");
    assert_eq!(gf256::mul(0x02, 0x80), 0x1D, "exactly one reduction");
    assert_eq!(gf256::mul(0xFF, 0xFF), 0xE2, "max value squared");
    assert_eq!(gf256::mul(0x53, 0xCA), 0x8F, "field-mismatch canary (0x01 under AES)");
}

#[test]
fn gf_inverse_holds_for_all_nonzero() {
    for a in 1u16..=255 {
        let a = a as u8;
        assert_eq!(gf256::mul(a, gf256::inv(a)), 1, "a * a^-1 must be 1 for a={a:#04x}");
    }
}

#[test]
fn gf_table_matches_slow_path() {
    for a in 0u16..=255 {
        for b in 0u16..=255 {
            assert_eq!(
                gf256::mul(a as u8, b as u8),
                gf256::mul_slow(a as u8, b as u8),
                "table and russian-peasant disagree on {a:#04x} * {b:#04x}"
            );
        }
    }
}

#[test]
fn vandermonde_points_cap_at_255() {
    // The 255 distinct nonzero powers, then the wrap that caps GF(2^8).
    use std::collections::HashSet;
    let points: HashSet<u8> = (0..255).map(gf256::nonzero_point).collect();
    assert_eq!(points.len(), 255, "GENERATOR powers give 255 distinct nonzero elements");
    assert_eq!(
        gf256::nonzero_point(255),
        gf256::nonzero_point(0),
        "point 255 wraps to point 0 - no 256th distinct nonzero point exists"
    );
}

fn make_data_shards(data: usize, len: usize) -> Vec<Vec<u8>> {
    (0..data)
        .map(|i| (0..len).map(|b| ((i * 31 + b * 17 + 7) & 0xff) as u8).collect())
        .collect()
}

#[test]
fn rs_roundtrip_small() {
    let rs = RsGf256::new(4, 2);
    let len = 16;
    let original = make_data_shards(4, len);
    let mut shards = original.clone();
    shards.resize(rs.total(), vec![0u8; len]);
    rs.encode(&mut shards);

    // Drop 2 shards (one data, one parity); keep 4 = data threshold.
    let present: Vec<(usize, Vec<u8>)> =
        [0usize, 2, 4, 5].iter().map(|&i| (i, shards[i].clone())).collect();
    let recovered = rs.reconstruct_data(&present).unwrap();
    assert_eq!(recovered, original);
}

#[test]
fn rs_roundtrip_constellation_64_192() {
    // Same parameters as the main crate path: 64 data : 192 parity = 256.
    let rs = RsGf256::new(64, 192);
    assert_eq!(rs.total(), 256);
    let len = 200;
    let original = make_data_shards(64, len);
    let mut shards = original.clone();
    shards.resize(256, vec![0u8; len]);
    rs.encode(&mut shards);

    // Keep only the last 64 (all parity-heavy) - the hardest recovery.
    let present: Vec<(usize, Vec<u8>)> =
        (192..256).map(|i| (i, shards[i].clone())).collect();
    let recovered = rs.reconstruct_data(&present).unwrap();
    assert_eq!(recovered, original, "from-scratch RS must round-trip at 64-of-256");
}

#[test]
fn rs_fails_below_threshold() {
    let rs = RsGf256::new(64, 192);
    let len = 64;
    let mut shards = make_data_shards(64, len);
    shards.resize(256, vec![0u8; len]);
    rs.encode(&mut shards);
    let present: Vec<(usize, Vec<u8>)> = (0..63).map(|i| (i, shards[i].clone())).collect();
    assert!(rs.reconstruct_data(&present).is_none(), "63 < 64 must not reconstruct");
}
