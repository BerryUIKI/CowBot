//! The two cryptographic operations Feishu's webhook transport needs
//! (ADR-0062).
//!
//! The long connection needs none of this: it authenticates by opening a socket.
//! A webhook has no connection to authenticate, so Feishu wraps every delivery in
//! an encryption envelope and signs the request. This module is that work, and it
//! is written by hand for the same reason [`crate::frame`] decodes protobuf by
//! hand — the scheme is small and fully specified, and the alternative is a
//! dependency for two functions.
//!
//! Two things here are easy to get wrong and are stated once:
//!
//! * **The IV is the first block of the ciphertext**, not a configured value. The
//!   reference implementations differ on how they treat it (one strips it, one
//!   trims to the outermost JSON object), so this unpads strictly and then
//!   trims, which is tolerant of both.
//! * **Events and card callbacks sign with different hashes and different
//!   secrets.** An event is SHA-256 over `timestamp + nonce + encrypt_key + body`;
//!   a card callback is SHA-1 over `timestamp + nonce + verification_token +
//!   body`. Using one for the other refuses every delivery of that kind with no
//!   clue why.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// The AES block size, in bytes.
const BLOCK: usize = 16;

/// The number of AES rounds for a 256-bit key — 14, so 14 key schedules.
const ROUNDS: usize = 14;

/// How old a signed request may be, in seconds.
///
/// Feishu does not document a window the way Slack does; five minutes matches
/// what its SDKs tolerate and keeps a captured request from working for ever.
pub const MAX_AGE_SECS: i64 = 60 * 5;

/// Why a delivery was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A header or field the check needs is missing.
    Missing(&'static str),
    /// The body is not the shape the envelope promises.
    Malformed(&'static str),
    /// The ciphertext is not a whole number of AES blocks.
    BadCiphertext,
    /// The signature does not match.
    BadSignature,
    /// No key or token is configured, so nothing can be verified.
    Unconfigured(&'static str),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Missing(name) => write!(formatter, "{name} is missing"),
            Refused::Malformed(what) => write!(formatter, "the delivery is malformed: {what}"),
            Refused::BadCiphertext => {
                formatter.write_str("the ciphertext is not a whole number of AES blocks")
            }
            Refused::BadSignature => formatter.write_str("the signature does not match"),
            Refused::Unconfigured(what) => {
                write!(
                    formatter,
                    "no {what} is configured, so nothing can be verified"
                )
            }
        }
    }
}

/// Decrypt the `encrypt` field of a Feishu delivery.
///
/// AES-256-CBC, key = `SHA-256(encrypt_key)`, IV = the first sixteen bytes of the
/// decoded ciphertext, then PKCS#7 padding removed. The reference
/// implementations differ on how they treat padding, so this unpads strictly and
/// only then trims to the outermost JSON object — either convention then works.
pub fn decrypt(encrypt_key: &str, encoded: &str) -> Result<String, Refused> {
    if encrypt_key.trim().is_empty() {
        return Err(Refused::Unconfigured("encrypt key"));
    }
    let raw =
        base64_decode(encoded).ok_or(Refused::Malformed("the encrypt field is not base64"))?;
    if raw.len() <= BLOCK || !(raw.len() - BLOCK).is_multiple_of(BLOCK) {
        return Err(Refused::BadCiphertext);
    }

    let key: [u8; 32] = Sha256::digest(encrypt_key.as_bytes()).into();
    let iv: [u8; BLOCK] = raw[..BLOCK].try_into().expect("checked above");
    let mut block = [0u8; BLOCK];
    block.copy_from_slice(&iv);

    let mut plain = Vec::with_capacity(raw.len() - BLOCK);
    let mut previous = block;
    let mut round_keys = expand_key(&key);
    for chunk in raw[BLOCK..].chunks(BLOCK) {
        let mut current = [0u8; BLOCK];
        current.copy_from_slice(chunk);
        let mut decrypted = current;
        decrypt_block(&mut decrypted, &round_keys);
        for (byte, prev) in decrypted.iter_mut().zip(previous.iter()) {
            *byte ^= prev;
        }
        plain.extend_from_slice(&decrypted);
        previous = current;
    }
    // The key schedule is per call and holds nothing worth keeping.
    round_keys.fill(0);

    let unpadded = unpad(&plain)?;
    let text = String::from_utf8(unpadded.to_vec())
        .map_err(|_| Refused::Malformed("the decrypted payload is not UTF-8"))?;
    // Some payloads carry the JSON with surrounding whitespace or a stray
    // newline; trimming to the outermost object is what the SDKs do.
    Ok(match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if end > start => text[start..=end].to_string(),
        _ => text,
    })
}

/// Sign an **event** delivery: SHA-256 over `timestamp + nonce + key + body`.
pub fn sign_event(key: &str, timestamp: &str, nonce: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("hmac accepts any key length");
    mac.update(timestamp.as_bytes());
    mac.update(nonce.as_bytes());
    mac.update(key.as_bytes());
    mac.update(body);
    hex(&mac.finalize().into_bytes())
}

/// Sign a **card callback**: SHA-1 over `timestamp + nonce + token + body`.
///
/// A different hash and a different secret from [`sign_event`]. Feishu's card
/// callbacks predate the event scheme and were never unified with it, which is
/// why one function cannot serve both.
pub fn sign_card(token: &str, timestamp: &str, nonce: &str, body: &[u8]) -> String {
    use sha1::Sha1;
    let mut mac = Hmac::<Sha1>::new_from_slice(token.as_bytes()).expect("hmac accepts any length");
    mac.update(timestamp.as_bytes());
    mac.update(nonce.as_bytes());
    mac.update(token.as_bytes());
    mac.update(body);
    hex(&mac.finalize().into_bytes())
}

/// Verify a signature and the age of its timestamp.
pub fn verify(
    expected: &str,
    presented: Option<&str>,
    timestamp: Option<&str>,
    now: i64,
) -> Result<(), Refused> {
    let presented = presented.ok_or(Refused::Missing("the signature header"))?;
    let timestamp = timestamp.ok_or(Refused::Missing("the timestamp header"))?;

    let sent: i64 = timestamp
        .parse()
        .map_err(|_| Refused::Malformed("the timestamp is not a number"))?;
    if (now - sent).abs() > MAX_AGE_SECS {
        return Err(Refused::Malformed(
            "the timestamp is outside the replay window",
        ));
    }

    // Both sides are hex of a digest; compare in constant time so a caller
    // cannot recover the signature a character at a time.
    if expected.as_bytes().ct_eq(presented.as_bytes()).into() {
        Ok(())
    } else {
        Err(Refused::BadSignature)
    }
}

/// Remove PKCS#7 padding, refusing a padding byte that cannot be right.
///
/// Strict rather than tolerant: a wrong padding length after a wrong key is the
/// one signal that the key is wrong, and swallowing it would turn a
/// misconfiguration into a confusing parse error.
fn unpad(padded: &[u8]) -> Result<&[u8], Refused> {
    let Some(&last) = padded.last() else {
        return Err(Refused::Malformed("the plaintext is empty"));
    };
    let count = last as usize;
    if count == 0 || count > BLOCK || count > padded.len() {
        return Err(Refused::BadCiphertext);
    }
    if !padded[padded.len() - count..]
        .iter()
        .all(|byte| *byte == last)
    {
        return Err(Refused::BadCiphertext);
    }
    Ok(&padded[..padded.len() - count])
}

// -- AES-256, decryption only -------------------------------------------------
//
// Only the decrypt direction is implemented: this module never encrypts. The
// tables below are the standard AES S-box and the round constants; the structure
// follows FIPS-197 directly.

/// The AES substitution box.
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// The inverse S-box, which decryption needs.
const INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];

/// Rounds constants for the key schedule.
const RCON: [u8; 15] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36, 0x6c, 0xd8, 0xab, 0x4d,
];

/// Multiply two elements of GF(2^8).
fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        let high = a & 0x80;
        a <<= 1;
        if high != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    product
}

/// Expand a 256-bit key into its fifteen round keys.
fn expand_key(key: &[u8; 32]) -> [u8; BLOCK * (ROUNDS + 1)] {
    let mut expanded = [0u8; BLOCK * (ROUNDS + 1)];
    expanded[..32].copy_from_slice(key);

    // A 256-bit key has 8 words per round key and 60 words in total, and its key
    // schedule applies SubWord every 8 words rather than every 4.
    let total = BLOCK * (ROUNDS + 1) / 4;
    let mut temp = [0u8; 4];
    for word in 8..total {
        temp.copy_from_slice(&expanded[(word - 1) * 4..word * 4]);
        if word % 8 == 0 {
            temp.rotate_left(1);
            for byte in temp.iter_mut() {
                *byte = SBOX[*byte as usize];
            }
            temp[0] ^= RCON[word / 8];
        } else if word % 8 == 4 {
            for byte in temp.iter_mut() {
                *byte = SBOX[*byte as usize];
            }
        }
        for index in 0..4 {
            expanded[word * 4 + index] = expanded[(word - 8) * 4 + index] ^ temp[index];
        }
    }
    expanded
}

/// Decrypt one block in place.
fn decrypt_block(block: &mut [u8; BLOCK], round_keys: &[u8; BLOCK * (ROUNDS + 1)]) {
    add_round_key(block, &round_keys[ROUNDS * BLOCK..]);

    for round in (1..ROUNDS).rev() {
        inv_shift_rows(block);
        for byte in block.iter_mut() {
            *byte = INV_SBOX[*byte as usize];
        }
        add_round_key(block, &round_keys[round * BLOCK..]);
        inv_mix_columns(block);
    }

    inv_shift_rows(block);
    for byte in block.iter_mut() {
        *byte = INV_SBOX[*byte as usize];
    }
    add_round_key(block, &round_keys[..BLOCK]);
}

fn add_round_key(block: &mut [u8; BLOCK], key: &[u8]) {
    for (byte, key) in block.iter_mut().zip(key.iter()) {
        *byte ^= key;
    }
}

/// The state is column-major: byte `4 * column + row`.
fn inv_shift_rows(block: &mut [u8; BLOCK]) {
    let copy = *block;
    for row in 0..4 {
        for column in 0..4 {
            // Row `r` shifts right by `r` when decrypting.
            let source = (column + 4 - row) % 4;
            block[4 * column + row] = copy[4 * source + row];
        }
    }
}

fn inv_mix_columns(block: &mut [u8; BLOCK]) {
    for column in 0..4 {
        let at = 4 * column;
        let [a0, a1, a2, a3] = [block[at], block[at + 1], block[at + 2], block[at + 3]];
        block[at] = mul(a0, 14) ^ mul(a1, 11) ^ mul(a2, 13) ^ mul(a3, 9);
        block[at + 1] = mul(a0, 9) ^ mul(a1, 14) ^ mul(a2, 11) ^ mul(a3, 13);
        block[at + 2] = mul(a0, 13) ^ mul(a1, 9) ^ mul(a2, 14) ^ mul(a3, 11);
        block[at + 3] = mul(a0, 11) ^ mul(a1, 13) ^ mul(a2, 9) ^ mul(a3, 14);
    }
}

// -- encoding helpers ---------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Standard base64 with padding, matching what Feishu sends.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for character in text.trim().chars() {
        if character == '=' {
            break;
        }
        let index = ALPHABET
            .iter()
            .position(|byte| *byte as char == character)? as u32;
        buffer = (buffer << 6) | index;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(first) << 16) | (u32::from(second) << 8) | u32::from(third);
        out.push(ALPHABET[(triple >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3F] as char
        } else {
            '='
        });
    }
    out
}

/// Encrypt with the same scheme, for tests only.
///
/// The adapter never encrypts — Feishu does — but a round-trip test is the only
/// way to prove the decrypt path without pasting a fixture whose provenance
/// nobody can check.
/// Seal a payload the way Feishu does, for an integration test.
///
/// Behind `cfg(test)` in the crate and `#[doc(hidden)]` here so the test crate can
/// reach it: the adapter never encrypts, but an integration test that pasted a
/// fixture would be testing a string nobody can verify, and a round trip through
/// the real scheme is the only honest alternative.
#[doc(hidden)]
pub fn seal_for_test(encrypt_key: &str, plaintext: &str) -> String {
    encrypt(encrypt_key, plaintext)
}

fn encrypt(encrypt_key: &str, plaintext: &str) -> String {
    let key: [u8; 32] = Sha256::digest(encrypt_key.as_bytes()).into();
    // A zero IV, as Feishu uses; it is prepended to the ciphertext.
    let iv = [0u8; BLOCK];
    let mut padded = plaintext.as_bytes().to_vec();
    let padding = BLOCK - padded.len() % BLOCK;
    padded.extend(std::iter::repeat_n(padding as u8, padding));

    let round_keys = expand_key(&key);
    let mut out = iv.to_vec();
    let mut previous = iv;
    for chunk in padded.chunks(BLOCK) {
        let mut block = [0u8; BLOCK];
        for (index, byte) in chunk.iter().enumerate() {
            block[index] = byte ^ previous[index];
        }
        encrypt_block(&mut block, &round_keys);
        out.extend_from_slice(&block);
        previous = block;
    }
    base64_encode(&out)
}

fn encrypt_block(block: &mut [u8; BLOCK], round_keys: &[u8; BLOCK * (ROUNDS + 1)]) {
    add_round_key(block, &round_keys[..BLOCK]);
    for round in 1..ROUNDS {
        for byte in block.iter_mut() {
            *byte = SBOX[*byte as usize];
        }
        shift_rows(block);
        mix_columns(block);
        add_round_key(block, &round_keys[round * BLOCK..]);
    }
    for byte in block.iter_mut() {
        *byte = SBOX[*byte as usize];
    }
    shift_rows(block);
    add_round_key(block, &round_keys[ROUNDS * BLOCK..]);
}

fn shift_rows(block: &mut [u8; BLOCK]) {
    let copy = *block;
    for row in 0..4 {
        for column in 0..4 {
            let destination = (column + 4 - row) % 4;
            block[4 * destination + row] = copy[4 * column + row];
        }
    }
}

fn mix_columns(block: &mut [u8; BLOCK]) {
    for column in 0..4 {
        let at = 4 * column;
        let [a0, a1, a2, a3] = [block[at], block[at + 1], block[at + 2], block[at + 3]];
        block[at] = mul(a0, 2) ^ mul(a1, 3) ^ a2 ^ a3;
        block[at + 1] = a0 ^ mul(a1, 2) ^ mul(a2, 3) ^ a3;
        block[at + 2] = a0 ^ a1 ^ mul(a2, 2) ^ mul(a3, 3);
        block[at + 3] = mul(a0, 3) ^ a1 ^ a2 ^ mul(a3, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "test-encrypt-key";

    #[test]
    fn the_aes_matches_the_published_fips_197_vector() {
        // A round-trip test would pass with a symmetric bug in both directions.
        // This is the single known-answer vector that pins the implementation:
        // FIPS-197 Appendix C.3, one block, AES-256.
        let key: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let plaintext: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let expected: [u8; 16] = [
            0x8e, 0xa2, 0xb7, 0xca, 0x51, 0x67, 0x45, 0xbf, 0xea, 0xfc, 0x49, 0x90, 0x4b, 0x49,
            0x60, 0x89,
        ];

        let round_keys = expand_key(&key);
        let mut block = plaintext;
        encrypt_block(&mut block, &round_keys);
        assert_eq!(block, expected, "the encryption direction");

        // And the decryption direction is the inverse, which is what the adapter
        // actually uses.
        decrypt_block(&mut block, &round_keys);
        assert_eq!(block, plaintext, "the decryption direction");
    }

    #[test]
    fn a_payload_round_trips_through_the_envelope() {
        let payload = r#"{"schema":"2.0","header":{"event_type":"im.message.receive_v1"}}"#;
        let sealed = encrypt(KEY, payload);
        assert_eq!(decrypt(KEY, &sealed).unwrap(), payload);
    }

    #[test]
    fn a_payload_with_a_length_that_is_already_a_block_still_round_trips() {
        // PKCS#7 always adds padding, so a 16-byte plaintext becomes 32; a
        // scheme that skipped it would fail here.
        let payload = "0123456789abcdef";
        let sealed = encrypt(KEY, payload);
        assert_eq!(decrypt(KEY, &sealed).unwrap(), payload);
    }

    #[test]
    fn the_wrong_key_is_refused_rather_than_producing_garbage() {
        // Strict unpadding is what turns a wrong key into a clear refusal
        // instead of a JSON parse error somewhere further along.
        let sealed = encrypt(KEY, r#"{"a":1}"#);
        assert!(decrypt("a-different-key", &sealed).is_err());
    }

    #[test]
    fn a_ciphertext_that_is_not_a_whole_number_of_blocks_is_refused() {
        assert_eq!(decrypt(KEY, "AAAA"), Err(Refused::BadCiphertext));
        // A valid base64 payload with no room for an IV.
        assert_eq!(
            decrypt(KEY, &base64_encode(&[0u8; 8])),
            Err(Refused::BadCiphertext)
        );
    }

    #[test]
    fn no_key_refuses_rather_than_decrypting_with_an_empty_one() {
        let sealed = encrypt(KEY, "{}");
        assert_eq!(
            decrypt("   ", &sealed),
            Err(Refused::Unconfigured("encrypt key"))
        );
    }

    #[test]
    fn an_event_and_a_card_callback_sign_with_different_schemes() {
        // The two are not interchangeable: an event is SHA-256 over the encrypt
        // key, a card callback SHA-1 over the verification token. Using one for
        // the other refuses every delivery of that kind with no clue why.
        let timestamp = "1700000000";
        let nonce = "n0nce";
        let body = br#"{"type":"event"}"#;

        let event = sign_event("the-encrypt-key", timestamp, nonce, body);
        let card = sign_card("the-encrypt-key", timestamp, nonce, body);
        assert_ne!(event, card);
        assert_eq!(event.len(), 64, "sha-256 is 32 bytes of hex");
        assert_eq!(card.len(), 40, "sha-1 is 20 bytes of hex");

        // Both are verified with the same age rule.
        assert_eq!(
            verify(&event, Some(&event), Some(timestamp), 1_700_000_000),
            Ok(())
        );
        assert_eq!(
            verify(&event, Some(&card), Some(timestamp), 1_700_000_000),
            Err(Refused::BadSignature)
        );
    }

    #[test]
    fn a_stale_or_future_dated_request_is_refused() {
        let timestamp = "1700000000";
        let body = b"{}";
        let signature = sign_event("k", timestamp, "n", body);
        assert_eq!(
            verify(&signature, Some(&signature), Some(timestamp), 1_700_000_000),
            Ok(())
        );
        assert!(verify(
            &signature,
            Some(&signature),
            Some(timestamp),
            1_700_000_000 + MAX_AGE_SECS + 1
        )
        .is_err());
        assert!(verify(
            &signature,
            Some(&signature),
            Some(timestamp),
            1_700_000_000 - MAX_AGE_SECS - 1
        )
        .is_err());
    }

    #[test]
    fn a_missing_header_is_named() {
        assert_eq!(
            verify("abc", None, Some("1700000000"), 1_700_000_000),
            Err(Refused::Missing("the signature header"))
        );
        assert_eq!(
            verify("abc", Some("abc"), None, 1_700_000_000),
            Err(Refused::Missing("the timestamp header"))
        );
    }
}
