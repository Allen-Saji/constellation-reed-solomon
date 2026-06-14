//! Constellation-parameterised Reed-Solomon erasure coding.
//!
//! The proposer turns a [`Pslice`] into [`N_PSHREDS`] = 256 [`Pshred`]s; any
//! [`GAMMA_P`] = 64 of them reconstruct the pslice. Encoding uses the exact crate
//! Agave uses (`reed_solomon_erasure::galois_8::ReedSolomon`), re-parameterised
//! from Agave's 32:32 FEC sizing to Constellation's 64:192.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use reed_solomon_erasure::galois_8::ReedSolomon;

// ---------------------------------------------------------------------------
// Parameters (Constellation whitepaper, Table 1; Anza v0.9, 2026-03-25)
// ---------------------------------------------------------------------------

/// `q` - attesters per cycle. One pshred is sent to each.
pub const Q_ATTESTERS: usize = 256;

/// `Gamma_p` - pshreds created/sent per pslice. Equal to `q`, so one per attester.
pub const N_PSHREDS: usize = 256;

/// `gamma_p` - the recovery threshold: pshreds needed to reconstruct (= `q / 4`).
/// In Reed-Solomon terms this is the number of **data** shards.
pub const GAMMA_P: usize = 64;

/// Reed-Solomon data-shard count = `gamma_p`.
pub const DATA_SHARDS: usize = GAMMA_P;

/// Reed-Solomon parity-shard count = `Gamma_p - gamma_p`.
pub const PARITY_SHARDS: usize = N_PSHREDS - GAMMA_P; // 192

/// Data expansion rate `Gamma_p / gamma_p` (whitepaper s2.2). 256 / 64 = 4x.
pub const EXPANSION: usize = N_PSHREDS / GAMMA_P;

/// Bytes used to length-prefix a pslice so padding can be trimmed on decode.
const LEN_PREFIX: usize = 8;

const _: () = {
    assert!(DATA_SHARDS + PARITY_SHARDS == N_PSHREDS);
    // GF(2^8) holds at most 256 total shards. Constellation sits exactly on it.
    assert!(N_PSHREDS <= 256, "galois_8 ReedSolomon caps total shards at 256");
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from shredding / reconstruction.
#[derive(Debug)]
pub enum Error {
    /// Underlying Reed-Solomon error (e.g. `TooFewShardsPresent` when fewer than
    /// `gamma_p` pshreds survive).
    Rs(reed_solomon_erasure::Error),
    /// A reconstructed pslice was structurally invalid (bad length prefix).
    Corrupt(&'static str),
}

impl From<reed_solomon_erasure::Error> for Error {
    fn from(e: reed_solomon_erasure::Error) -> Self {
        Error::Rs(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Rs(e) => write!(f, "reed-solomon error: {e}"),
            Error::Corrupt(why) => write!(f, "corrupt pslice: {why}"),
        }
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A proposer's batch of accepted transactions: the data to be erasure-coded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pslice(pub Vec<u8>);

impl Pslice {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Pslice(bytes.into())
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One erasure-coded piece of a pslice, addressed to attester `index`.
///
/// `index < N_PSHREDS`. The first [`DATA_SHARDS`] indices are systematic (they
/// carry the original pslice bytes unchanged); the rest are parity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pshred {
    pub index: u16,
    pub bytes: Vec<u8>,
}

impl Pshred {
    /// True if this is a systematic (data) pshred rather than a parity pshred.
    pub fn is_systematic(&self) -> bool {
        (self.index as usize) < DATA_SHARDS
    }
}

// ---------------------------------------------------------------------------
// ReedSolomonCache - mirrors ledger/src/shredder.rs::ReedSolomonCache
// ---------------------------------------------------------------------------

/// Caches one `ReedSolomon` encoder per `(data, parity)` pair.
///
/// Building a `ReedSolomon` precomputes its coding matrix, so Agave caches the
/// encoders rather than rebuilding per FEC set. Agave keys a `lazy_lru::LruCache`
/// by `(data, parity)` and wraps each value in `Arc<OnceLock<..>>` for a
/// lock-free hot path (capacity `4 * DATA_SHREDS_PER_FEC_BLOCK = 128`). Our
/// keyspace is tiny (usually just `(64, 192)`), so a `Mutex<HashMap>` is plenty;
/// the cache *shape* - one shared `Arc<ReedSolomon>` per parameterisation,
/// constructed lazily - matches Agave.
#[derive(Default)]
pub struct ReedSolomonCache(Mutex<HashMap<(usize, usize), Arc<ReedSolomon>>>);

impl ReedSolomonCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get (or lazily build and cache) the encoder for `(data, parity)`.
    pub fn get(&self, data: usize, parity: usize) -> Result<Arc<ReedSolomon>, Error> {
        let mut map = self.0.lock().unwrap();
        if let Some(rs) = map.get(&(data, parity)) {
            return Ok(Arc::clone(rs));
        }
        let rs = Arc::new(ReedSolomon::new(data, parity)?);
        map.insert((data, parity), Arc::clone(&rs));
        Ok(rs)
    }
}

// ---------------------------------------------------------------------------
// Proposer - encode a pslice into pshreds
// ---------------------------------------------------------------------------

/// The encoding side: turns a pslice into [`N_PSHREDS`] pshreds.
#[derive(Default)]
pub struct Proposer {
    cache: ReedSolomonCache,
}

impl Proposer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Erasure-code a pslice into `Gamma_p = 256` pshreds (64 systematic + 192
    /// parity). Any `gamma_p = 64` of them reconstruct the pslice.
    ///
    /// Layout: the pslice is length-prefixed (8 bytes, big-endian), zero-padded up
    /// to a multiple of `DATA_SHARDS`, then split into 64 equal contiguous data
    /// shards. The crate fills the 192 parity shards in place.
    pub fn shred(&self, pslice: &Pslice) -> Result<Vec<Pshred>, Error> {
        // Length-prefix + payload, padded so it splits evenly into DATA_SHARDS.
        let mut buf = Vec::with_capacity(LEN_PREFIX + pslice.0.len());
        buf.extend_from_slice(&(pslice.0.len() as u64).to_be_bytes());
        buf.extend_from_slice(&pslice.0);
        let shard_len = buf.len().div_ceil(DATA_SHARDS).max(1);
        buf.resize(shard_len * DATA_SHARDS, 0);

        // 64 data shards (contiguous slices) + 192 zeroed parity slots.
        let mut shards: Vec<Vec<u8>> = Vec::with_capacity(N_PSHREDS);
        for i in 0..DATA_SHARDS {
            shards.push(buf[i * shard_len..(i + 1) * shard_len].to_vec());
        }
        shards.extend((0..PARITY_SHARDS).map(|_| vec![0u8; shard_len]));

        // Encode: parity slots overwritten in place; data shards untouched.
        self.cache.get(DATA_SHARDS, PARITY_SHARDS)?.encode(&mut shards)?;

        Ok(shards
            .into_iter()
            .enumerate()
            .map(|(i, bytes)| Pshred {
                index: i as u16,
                bytes,
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Attester / reconstruction - recover a pslice from surviving pshreds
// ---------------------------------------------------------------------------

/// The decoding side: reconstruct a pslice from the pshreds that survived.
#[derive(Default)]
pub struct Attester {
    cache: ReedSolomonCache,
}

impl Attester {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconstruct the pslice from however many pshreds arrived.
    ///
    /// `received` holds the pshreds that survived transit (in any order; missing
    /// ones simply absent). Succeeds iff at least `gamma_p = 64` distinct pshreds
    /// are present; otherwise returns `Error::Rs(TooFewShardsPresent)`.
    pub fn reconstruct(&self, received: &[Pshred]) -> Result<Pslice, Error> {
        // Place each survivor at its index; missing slots stay `None`.
        let mut slots: Vec<Option<Vec<u8>>> = vec![None; N_PSHREDS];
        for p in received {
            let idx = p.index as usize;
            if idx < N_PSHREDS {
                slots[idx] = Some(p.bytes.clone());
            }
        }

        // Recover the data shards (parity not needed for read-out).
        self.cache
            .get(DATA_SHARDS, PARITY_SHARDS)?
            .reconstruct_data(&mut slots)?;

        // Concatenate the 64 systematic shards, then strip prefix + padding.
        let shard_len = slots[0]
            .as_ref()
            .ok_or(Error::Corrupt("data shard 0 missing after reconstruct"))?
            .len();
        let mut buf = Vec::with_capacity(DATA_SHARDS * shard_len);
        for slot in slots.iter().take(DATA_SHARDS) {
            buf.extend_from_slice(
                slot.as_ref()
                    .ok_or(Error::Corrupt("data shard missing after reconstruct"))?,
            );
        }

        if buf.len() < LEN_PREFIX {
            return Err(Error::Corrupt("pslice shorter than length prefix"));
        }
        let len = u64::from_be_bytes(buf[..LEN_PREFIX].try_into().unwrap()) as usize;
        if LEN_PREFIX + len > buf.len() {
            return Err(Error::Corrupt("length prefix exceeds reconstructed buffer"));
        }
        Ok(Pslice(buf[LEN_PREFIX..LEN_PREFIX + len].to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_are_constellation() {
        assert_eq!(N_PSHREDS, 256);
        assert_eq!(GAMMA_P, 64);
        assert_eq!(DATA_SHARDS, 64);
        assert_eq!(PARITY_SHARDS, 192);
        assert_eq!(EXPANSION, 4);
    }

    #[test]
    fn shred_produces_256_pshreds() {
        let proposer = Proposer::new();
        let pshreds = proposer.shred(&Pslice::new(b"hello constellation".to_vec())).unwrap();
        assert_eq!(pshreds.len(), N_PSHREDS);
        assert!(pshreds[..DATA_SHARDS].iter().all(Pshred::is_systematic));
        assert!(pshreds[DATA_SHARDS..].iter().all(|p| !p.is_systematic()));
    }

    #[test]
    fn cache_returns_same_encoder() {
        let cache = ReedSolomonCache::new();
        let a = cache.get(DATA_SHARDS, PARITY_SHARDS).unwrap();
        let b = cache.get(DATA_SHARDS, PARITY_SHARDS).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "cache should hand back the same Arc");
    }
}
