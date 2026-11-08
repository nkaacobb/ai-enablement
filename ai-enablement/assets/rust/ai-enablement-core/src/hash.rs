//! Content fingerprints for the context-sync flag (`core/hash.js`).
//!
//! cyrb53: a fast 53-bit string hash over the UTF-16 code units of the text, written as 14 lowercase hex digits.
//! Not cryptographic; it only has to notice change. The same text gives the same hash in every runtime, so a chat
//! saved by one runtime keeps its sync state in another.

pub use crate::json::stable_stringify;

/// cyrb53 of `text` (UTF-16 code units), as 14 lowercase hex digits.
pub fn hash_text(text: &str) -> String {
    let mut h1: u32 = 0xdead_beef;
    let mut h2: u32 = 0x41c6_ce57;
    for unit in text.encode_utf16() {
        let ch = u32::from(unit);
        h1 = (h1 ^ ch).wrapping_mul(2_654_435_761);
        h2 = (h2 ^ ch).wrapping_mul(1_597_334_677);
    }
    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2_246_822_507) ^ (h2 ^ (h2 >> 13)).wrapping_mul(3_266_489_909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2_246_822_507) ^ (h1 ^ (h1 >> 13)).wrapping_mul(3_266_489_909);
    let n = 4_294_967_296_u64 * u64::from(2_097_151 & h2) + u64::from(h1);
    format!("{n:014x}")
}

/// The short form shown to the user: the first 7 hex digits, like a git commit.
pub fn short_hash(hash: &str) -> &str {
    crate::text::js_head(hash, 7)
}
