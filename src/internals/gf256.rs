//! GF(2^8) arithmetic from scratch.
//!
//! Field elements are bytes (polynomials of degree <= 7 over GF(2)). Addition is
//! XOR; multiplication is carryless polynomial multiply reduced modulo the
//! irreducible polynomial `0x11D = x^8 + x^4 + x^3 + x^2 + 1`, with `0x02` as the
//! primitive generator. This is the standard Reed-Solomon field (not the AES
//! field `0x11B` - they give different products: `0x53 * 0xCA` is `0x8F` here but
//! `0x01` under AES).

use std::sync::OnceLock;

/// Irreducible polynomial `x^8 + x^4 + x^3 + x^2 + 1`. Low byte `0x1D` is XORed in
/// during reduction (the `x^8` term is the implicit bit that overflowed).
const POLY_LOW: u8 = 0x1D;
/// Primitive element: its powers cycle through all 255 nonzero elements.
const GENERATOR: u8 = 0x02;

/// Field addition (and subtraction - identical in characteristic 2).
#[inline]
pub fn add(a: u8, b: u8) -> u8 {
    a ^ b
}

/// Russian-peasant multiply: carryless multiply, reducing whenever a bit spills
/// past degree 7. This is the readable reference; [`mul`] uses tables for speed.
pub fn mul_slow(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0u8;
    for _ in 0..8 {
        if b & 1 == 1 {
            r ^= a; // add the current shifted copy of a
        }
        let overflow = a & 0x80; // will bit 7 spill into x^8 on the next shift?
        a <<= 1;
        if overflow != 0 {
            a ^= POLY_LOW; // reduce mod the irreducible polynomial
        }
        b >>= 1;
    }
    r
}

struct Tables {
    /// `exp[i] = GENERATOR^i` for `i in 0..255` (covers every nonzero element).
    exp: [u8; 255],
    /// `log[x]` is the discrete log of `x` (undefined / 0 for `x == 0`).
    log: [u8; 256],
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut exp = [0u8; 255];
        let mut log = [0u8; 256];
        let mut x = 1u8;
        for (i, slot) in exp.iter_mut().enumerate() {
            *slot = x;
            log[x as usize] = i as u8;
            x = mul_slow(x, GENERATOR);
        }
        // If 0x02 is primitive, multiplying by it 255 times returns to 1.
        debug_assert_eq!(x, 1, "0x02 must be a primitive element mod 0x11D");
        Tables { exp, log }
    })
}

/// Field multiplication via log/exp tables: `a*b = exp[(log a + log b) mod 255]`.
#[inline]
pub fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let t = tables();
    let l = t.log[a as usize] as usize + t.log[b as usize] as usize;
    t.exp[l % 255]
}

/// Multiplicative inverse: `a^-1 = exp[(255 - log a) mod 255]`. Panics on zero.
#[inline]
pub fn inv(a: u8) -> u8 {
    assert!(a != 0, "0 has no multiplicative inverse in GF(2^8)");
    let t = tables();
    t.exp[(255 - t.log[a as usize] as usize) % 255]
}

/// The `i`-th distinct nonzero element, `GENERATOR^i`.
///
/// This is how a *Vandermonde* Reed-Solomon code picks evaluation points - and it
/// is exactly why GF(2^8) tops out at 255 total shards: `GENERATOR^255 ==
/// GENERATOR^0 == 1`, so there are only 255 distinct nonzero powers. (A *Cauchy*
/// code, like [`super::rs`], can reach 256 by also using the zero element.)
pub fn nonzero_point(i: usize) -> u8 {
    tables().exp[i % 255]
}
