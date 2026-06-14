//! Proposer side of the `tc netem` attester-loss demo.
//!
//! Encodes a deterministic demo pslice into 256 pshreds and fires each as one UDP
//! datagram at the attester. When `tc netem` drops a fraction of those datagrams,
//! the attester should still reconstruct as long as >= 64 survive.
//!
//! Datagram wire format: `[2-byte big-endian pshred index][shard bytes]`.
//!
//! Usage: `proposer <dest addr> [pslice_len_bytes]`
//!   e.g. `proposer 10.55.0.2:9000 16384`

use std::env;
use std::net::UdpSocket;

use constellation_reed_solomon::{demo_pslice, Proposer, N_PSHREDS};

fn main() -> std::io::Result<()> {
    let dest = env::args().nth(1).unwrap_or_else(|| "10.55.0.2:9000".to_string());
    let pslice_len: usize = env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(16_384);

    let pslice = demo_pslice(pslice_len);
    let pshreds = Proposer::new()
        .shred(&pslice)
        .expect("shredding the demo pslice");

    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let mut sent = 0usize;
    for p in &pshreds {
        let mut buf = Vec::with_capacity(2 + p.bytes.len());
        buf.extend_from_slice(&p.index.to_be_bytes());
        buf.extend_from_slice(&p.bytes);
        sock.send_to(&buf, &dest)?;
        sent += 1;
    }

    println!(
        "proposer: sent {sent}/{N_PSHREDS} pshreds ({} bytes each) to {dest} \
         | pslice {pslice_len} bytes, expansion 4x",
        pshreds[0].bytes.len(),
    );
    Ok(())
}
