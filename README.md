# Constellation erasure encoder

A **Constellation-parameterised Reed-Solomon erasure encoder**, built on the same
crate Agave uses and re-parameterised from Agave's current Turbine FEC sizing to
Solana Constellation's proposer/attester sizing.

An experiment: take the erasure coding Solana ships today, re-tune it to the sizing
a proposed multi-proposer consensus change would need, and measure whether it still
fits the latency budget.

## The idea in one paragraph

Constellation (Anza's Multiple Concurrent Proposers proposal) erasure-codes a
proposer's batch of transactions (a **pslice**) into 256 pieces (**pshreds**), one
per attester, so that **any 64** pshreds reconstruct the pslice. That is a
Reed-Solomon `(data = 64, parity = 192)` code: 256 total shards. Because no small
group of attesters holds "the" copy, censoring a pslice means stopping it from
reaching more than ~75% of attesters (you must suppress more than `256 - 64 = 192`
of 256), instead of silencing a handful of nodes. Erasure coding, not replication,
is what makes inclusion censorship-resistant.

## Grounded in Agave

The starting point is Agave's shredder, `ledger/src/shredder.rs` and
`ledger/src/shred.rs` in
[anza-xyz/agave](https://github.com/anza-xyz/agave). Today Agave shreds at:

```rust
// ledger/src/shred.rs
pub const DATA_SHREDS_PER_FEC_BLOCK: usize = 32;
pub const CODING_SHREDS_PER_FEC_BLOCK: usize = 32; // 32:32 = 64 per FEC set
```

and caches one encoder per `(data, parity)` pair:

```rust
// ledger/src/shredder.rs
pub struct ReedSolomonCache(
    LruCacheOnce<(usize, usize), Result<Arc<ReedSolomon>, reed_solomon_erasure::Error>>,
);
// .get(data, parity) -> Arc<ReedSolomon>, lock-free OnceLock init, capacity 128
```

The encode call site (`ledger/src/shred/merkle.rs::finish_erasure_batch`):

```rust
reed_solomon_cache
    .get(num_data_shreds, num_coding_shreds)?
    .encode(shreds.iter_mut().map(Shred::erasure_shard_mut).collect::<Result<Vec<_>, _>>()?)?;
```

This crate keeps that cache shape ([`ReedSolomonCache`](src/constellation.rs)) and
swaps the parameters:

| Concept | Agave Turbine (today) | Constellation (this crate) |
|---|---|---|
| data shards | 32 | **64** (`gamma_p`, recovery threshold) |
| coding/parity shards | 32 | **192** |
| total shards | 64 | **256** (`Gamma_p`, one per attester) |
| recover from | any 32 | **any 64** |
| expansion | 2x | **4x** (`Gamma_p / gamma_p`) |

Source pins (Agave `master`, commit `f8bc56e`):
[shred.rs#L118](https://github.com/anza-xyz/agave/blob/f8bc56ec839edc6a81facb10bace51e2634badd9/ledger/src/shred.rs#L118-L123)
· [shredder.rs#L32](https://github.com/anza-xyz/agave/blob/f8bc56ec839edc6a81facb10bace51e2634badd9/ledger/src/shredder.rs#L32-L306)
· [merkle.rs#L1245](https://github.com/anza-xyz/agave/blob/f8bc56ec839edc6a81facb10bace51e2634badd9/ledger/src/shred/merkle.rs#L1245-L1295)

## Why 256 is the exact ceiling

`reed_solomon_erasure::galois_8::ReedSolomon` works over GF(2^8), which has 256
field elements. The crate allows `data + parity <= 256` (`ORDER = 256`; it errors
with `TooManyShards` only above 256). So Constellation's `64 + 192 = 256` is the
**largest configuration GF(2^8) can express** - one rung higher (`new(65, 192)`)
fails. That is exactly why systems that need more shards (Polkadot's data
availability runs to thousands of validators) move to GF(2^16). Constellation
stays in GF(2^8), right at the edge.

## Layout

```
src/constellation.rs   Pslice/Pshred types, ReedSolomonCache, Proposer (encode), Attester (decode)
src/bin/proposer.rs    sends 256 pshreds as UDP datagrams (for the netem demo)
src/bin/attester.rs    receives survivors, reconstructs, verifies
tests/roundtrip.rs     encode -> drop -> reconstruct -> assert equal (the core RS correctness test)
benches/encode_decode.rs   Criterion bench extending Agave PR #5695's structure
scripts/netem_sim.sh   tc netem attester-loss simulation in an isolated netns
```

## Run it

### Round-trip correctness

```bash
cargo test
```

Encodes pslices of many sizes, drops pshreds down to (and below) the 64-survivor
threshold across 200 random erasure patterns, reconstructs, and asserts the
original bytes return. Includes the boundary: 64 survivors recover, 63 return
`TooFewShardsPresent`.

### Simulated attester loss with `tc netem`

```bash
sudo ./scripts/netem_sim.sh 70 16384    # 70% packet loss, 16 KB pslice
```

Builds an isolated network namespace + veth pair (your real loopback is never
touched), applies `tc netem loss 70%` on the link to the attester, fires all 256
pshreds, and reconstructs from whatever survives. At 64-of-256 the pslice survives
any loss that still delivers >= 64 pshreds (so ~75% loss is the theoretical edge;
70% leaves comfortable margin). Requires root and `iproute2`.

### Benchmarks (extends PR #5695)

```bash
cargo bench
```

[PR #5695](https://github.com/anza-xyz/agave/pull/5695) added a Criterion recovery
benchmark (`run_recover_shreds` / `bench_recover_shreds`, sweeping
`num_packets x num_code`). This crate mirrors that structure and extends the sweep
to Constellation's `(64, 192)` next to Agave's `(32, 32)`.

## Results: does it fit the 50 ms cycle?

Constellation's fundamental unit is a **50 ms cycle** - assemble the pslice,
erasure-code it, and disseminate must all fit inside it. Measured on a dev laptop,
**non-SIMD** path (Agave enables `simd-accel`, which is faster), so these are
conservative upper bounds. Indicative medians, 1 KB shards (64 KB pslice):

| Operation | Turbine (32:32) | Constellation (64:192) |
|---|---|---|
| encode | ~0.32 ms | ~3.4 ms |
| recover, 32 lost | ~0.27 ms | ~0.55 ms |
| recover, 64 lost (max data loss) | - | ~1.1 ms |
| recover, 192 lost (only 64 survive) | - | ~1.1 ms |

Takeaways:

* Even the naive GF(2^8) matrix encoder fits the 50 ms budget with **~10x
  headroom** for a 64 KB pslice; recovery is ~1 ms worst case.
* Constellation costs ~10x Turbine's encode at equal shard size - expected, since
  encode is O(data x parity) and `64 x 192 = 12288` vs `32 x 32 = 1024` (~12x more
  field multiplies). The 4x expansion is bandwidth bought for censorship
  resistance.
* Recovery time plateaus once all 64 data shards are missing: the work is
  rebuilding 64 data shards from 64 survivors, independent of how many parity
  shards were also lost.

## Scope and caveats

* Benchmarks are single-machine, non-SIMD, short-sample - directional, not Anza
  production figures. Re-run with `simd-accel` and longer measurement for real
  comparisons.
* This is the erasure-coding layer only. Constellation's hiding property (HECC, so
  fewer than 64 pshreds reveal nothing about the batch) is **not** implemented;
  v1 Constellation itself uses plain Reed-Solomon with partial hiding.
* Constellation is a proposal (whitepaper v0.9), not ratified protocol.

## References

* Constellation whitepaper v0.9 (Kniep, Resnick, Sliwinski, Wattenhofer, Anza, 2026-03-25), Table 1 parameters.
* `reed-solomon-erasure` v6.0.0 (the crate + version Agave depends on).
* Agave PR [#5695](https://github.com/anza-xyz/agave/pull/5695).
