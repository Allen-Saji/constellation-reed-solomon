//! Attester side of the `tc netem` attester-loss demo.
//!
//! Listens for pshred datagrams, collects whatever survives the lossy link, then
//! tries to reconstruct the pslice. Succeeds iff >= 64 distinct pshreds arrived.
//! Regenerates the deterministic demo pslice locally to confirm the bytes match.
//!
//! Usage: `attester <bind addr> [pslice_len_bytes]`
//!   e.g. `attester 10.55.0.2:9000 16384`

use std::env;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use adv_svm_erasure_lab::{demo_pslice, Attester, Pshred, GAMMA_P, N_PSHREDS};

fn main() -> std::io::Result<()> {
    let bind = env::args().nth(1).unwrap_or_else(|| "10.55.0.2:9000".to_string());
    let pslice_len: usize = env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(16_384);

    let sock = UdpSocket::bind(&bind)?;
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    println!("attester: listening on {bind}, expecting up to {N_PSHREDS} pshreds");

    let mut received: Vec<Pshred> = Vec::with_capacity(N_PSHREDS);
    let mut seen = vec![false; N_PSHREDS];
    let mut buf = vec![0u8; 70_000];
    let start = Instant::now();

    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, _)) if n >= 2 => {
                let idx = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                if idx < N_PSHREDS && !seen[idx] {
                    seen[idx] = true;
                    received.push(Pshred {
                        index: idx as u16,
                        bytes: buf[2..n].to_vec(),
                    });
                }
            }
            Ok(_) => {} // runt packet, ignore
            Err(_) => {
                // Timed out. If we've already heard pshreds, the sender is done.
                if !received.is_empty() {
                    break;
                }
                // Otherwise keep waiting for the sender to start, up to 5s.
                if start.elapsed() > Duration::from_secs(5) {
                    break;
                }
            }
        }
    }

    let got = received.len();
    let lost = N_PSHREDS - got;
    let loss_pct = 100.0 * lost as f64 / N_PSHREDS as f64;
    println!("attester: received {got}/{N_PSHREDS} pshreds ({loss_pct:.1}% lost), threshold is {GAMMA_P}");

    match Attester::new().reconstruct(&received) {
        Ok(pslice) if pslice == demo_pslice(pslice_len) => {
            println!("attester: OK - reconstructed {pslice_len}-byte pslice from {got} of {N_PSHREDS} pshreds (survived {lost} losses)");
            Ok(())
        }
        Ok(_) => {
            eprintln!("attester: FAIL - reconstructed, but bytes did not match the expected pslice");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("attester: FAIL - could not reconstruct ({e}); had {got} pshreds, need >= {GAMMA_P}");
            std::process::exit(2);
        }
    }
}
