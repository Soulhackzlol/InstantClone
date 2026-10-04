//! Zero-dependency crypto for the OPTIONAL dashboard auth: PBKDF2-HMAC-SHA256
//! password hashing, an OS CSPRNG for tokens, and a constant-time compare.
//!
//! Built entirely on our own `sha256` and the OS RNG we already link
//! (BCryptGenRandom via `windows-sys`, `/dev/urandom` on Unix), so enabling
//! auth pulls in NO new crate. For the default install that never sets a
//! password this is a few KB of never-executed code and nothing more.
//!
//! Correctness is pinned to the published RFC 4231 (HMAC) and RFC 7914 /
//! PBKDF2-HMAC-SHA256 test vectors in the test module. Do not touch the round
//! functions without re-running them.

use crate::sha256;

const SHA256_BLOCK: usize = 64;

/// Iteration count for new password hashes. PBKDF2 sets both our login cost
/// and the attacker's per-guess cost; paired with the login rate limiter
/// (the primary defence against online guessing) this is a sound balance for
/// a self-hosted tool where the hash only leaks under a deeper compromise.
/// The value is stored inside each hash, so raising it later re-hashes on the
/// next password change without breaking existing logins.
const PBKDF2_ITERATIONS: u32 = 210_000;

const SALT_LEN: usize = 16;

/// HMAC-SHA256(key, msg) per RFC 2104.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    // Keys longer than the block are hashed down first (RFC 2104).
    let mut k = [0u8; SHA256_BLOCK];
    if key.len() > SHA256_BLOCK {
        k[..32].copy_from_slice(&sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; SHA256_BLOCK];
    let mut opad = [0x5cu8; SHA256_BLOCK];
    for i in 0..SHA256_BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }

    // inner = H(ipad || msg)
    let mut inner = Vec::with_capacity(SHA256_BLOCK + msg.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(msg);
    let inner_digest = sha256::digest(&inner);

    // H(opad || inner)
    let mut outer = [0u8; SHA256_BLOCK + 32];
    outer[..SHA256_BLOCK].copy_from_slice(&opad);
    outer[SHA256_BLOCK..].copy_from_slice(&inner_digest);
    sha256::digest(&outer)
}

/// PBKDF2-HMAC-SHA256 producing a 32-byte key (RFC 8018). One output block, so
/// the derived length equals one HMAC output and no block loop is needed.
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    // U1 = PRF(pw, salt || INT_32_BE(1)); Ui = PRF(pw, Ui-1); T = U1 ^ ... ^ Uc.
    let mut salt_block = Vec::with_capacity(salt.len() + 4);
    salt_block.extend_from_slice(salt);
    salt_block.extend_from_slice(&1u32.to_be_bytes());

    let mut u = hmac_sha256(password, &salt_block);
    let mut t = u;
    for _ in 1..iterations {
        u = hmac_sha256(password, &u);
        for i in 0..32 {
            t[i] ^= u[i];
        }
    }
    t
}

/// Constant-time byte comparison: no early return, so a caller cannot learn
/// how many leading bytes matched by timing the response.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Fill `buf` with cryptographically secure random bytes from the OS. Used for
/// salts and session/dock tokens. A failure here means the OS RNG is
/// unavailable, which is not a recoverable state for a server that must mint
/// unguessable tokens, so we fail hard rather than emit weak randomness.
pub fn os_random(buf: &mut [u8]) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::{
            BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        };
        // SAFETY: null algorithm handle is valid with USE_SYSTEM_PREFERRED_RNG;
        // buffer + length describe a live, writable slice.
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        assert!(status == 0, "BCryptGenRandom failed: {status:#x}");
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut f = std::fs::File::open("/dev/urandom").expect("open /dev/urandom");
        f.read_exact(buf).expect("read /dev/urandom");
    }
}

/// A fresh 256-bit token as 64 lowercase hex chars. Used for session and dock
/// tokens; 256 bits of CSPRNG output is not brute-forceable.
pub fn random_token() -> String {
    let mut b = [0u8; 32];
    os_random(&mut b);
    sha256::hex_bytes(&b)
}

/// Hash a password for storage as `pbkdf2-sha256$<iter>$<salt_hex>$<dk_hex>`.
/// A fresh random salt is generated per call, so identical passwords never
/// share a hash.
pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; SALT_LEN];
    os_random(&mut salt);
    let dk = pbkdf2_sha256(password.as_bytes(), &salt, PBKDF2_ITERATIONS);
    format!(
        "pbkdf2-sha256${}${}${}",
        PBKDF2_ITERATIONS,
        sha256::hex_bytes(&salt),
        sha256::hex_bytes(&dk)
    )
}

/// Verify `password` against a stored hash. Constant-time on the digest
/// compare; returns false on any parse failure (an unrecognised or corrupt
/// stored value can never authenticate).
pub fn verify_password(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    if parts.len() != 4 || parts[0] != "pbkdf2-sha256" {
        return false;
    }
    let Ok(iterations) = parts[1].parse::<u32>() else {
        return false;
    };
    if iterations == 0 {
        return false;
    }
    let (Some(salt), Some(expected)) = (unhex(parts[2]), unhex(parts[3])) else {
        return false;
    };
    let dk = pbkdf2_sha256(password.as_bytes(), &salt, iterations);
    constant_time_eq(&dk, &expected)
}

/// Decode a lowercase/uppercase hex string to bytes. None on odd length or a
/// non-hex character.
fn unhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    let val = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    for pair in b.as_chunks::<2>().0 {
        out.push((val(pair[0])? << 4) | val(pair[1])?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 HMAC-SHA256 test case 1: key = 0x0b x20, data = "Hi There".
    #[test]
    fn hmac_rfc4231_case1() {
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            sha256::hex_bytes(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    /// RFC 4231 test case 2: key = "Jefe", data = "what do ya want for nothing?".
    #[test]
    fn hmac_rfc4231_case2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            sha256::hex_bytes(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    /// RFC 4231 test case 6: a 131-byte key, longer than the 64-byte block, so
    /// it must be hashed down first. Cases 1 and 2 use short keys and never
    /// reach that branch; a password longer than 64 bytes does.
    #[test]
    fn hmac_rfc4231_case6_hashes_an_oversized_key() {
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            sha256::hex_bytes(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    /// RFC 4231 test case 7: an oversized key AND a message longer than one
    /// block, so the inner hash spans several blocks.
    #[test]
    fn hmac_rfc4231_case7_oversized_key_and_message() {
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"This is a test using a larger than block-size key and a larger than \
              block-size data. The key needs to be hashed before being used by the \
              HMAC algorithm.",
        );
        assert_eq!(
            sha256::hex_bytes(&mac),
            "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2"
        );
    }

    /// A key of exactly one block is used as-is, not hashed. This is the
    /// boundary of the `>` in `hmac_sha256`: turning it into `>=` would hash a
    /// 64-byte key and change every MAC made with one. Expected value is the
    /// NIST HMAC-SHA256 "keylen=blocklen" example (key = bytes 0x00..0x3f).
    #[test]
    fn hmac_key_of_exactly_one_block_is_not_hashed() {
        let key: Vec<u8> = (0u8..64).collect();
        let mac = hmac_sha256(&key, b"Sample message for keylen=blocklen");
        assert_eq!(
            sha256::hex_bytes(&mac),
            "8bb9a1db9806f20df7f77b82138c7914d174d59e13dc4d0169c9057b133e1d62"
        );
    }

    /// Published PBKDF2-HMAC-SHA256 vectors (password "password", salt "salt").
    #[test]
    fn pbkdf2_known_vectors() {
        assert_eq!(
            sha256::hex_bytes(&pbkdf2_sha256(b"password", b"salt", 1)),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        assert_eq!(
            sha256::hex_bytes(&pbkdf2_sha256(b"password", b"salt", 2)),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
        assert_eq!(
            sha256::hex_bytes(&pbkdf2_sha256(b"password", b"salt", 4096)),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn password_round_trip() {
        let h = hash_password("correct horse battery staple");
        assert!(verify_password("correct horse battery staple", &h));
        assert!(!verify_password("wrong password", &h));
        // Same password, fresh salt -> different stored hash.
        let h2 = hash_password("correct horse battery staple");
        assert_ne!(h, h2);
    }

    /// A stored hash in the exact on-disk format, built from the published
    /// PBKDF2 vector (password "password", salt "salt" = hex 73616c74, c = 1).
    ///
    /// `password_round_trip` only proves `hash_password` and `verify_password`
    /// agree with each other, so a format change made to both sides passes it
    /// while locking every existing user out of their dashboard. This one is
    /// what a config file written by an older build actually contains.
    const STORED_VECTOR: &str = "pbkdf2-sha256$1$73616c74$\
        120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b";

    #[test]
    fn an_existing_stored_hash_still_verifies() {
        assert!(verify_password("password", STORED_VECTOR));
        assert!(!verify_password("Password", STORED_VECTOR));
    }

    #[test]
    fn verify_rejects_garbage() {
        assert!(!verify_password("x", ""));
        assert!(!verify_password("x", "not-a-hash"));
        assert!(!verify_password("x", "pbkdf2-sha256$0$aa$bb"));
        assert!(!verify_password("x", "pbkdf2-sha256$1$zz$bb")); // non-hex salt
    }

    /// Every malformed variant of a hash that is otherwise correct for this
    /// password. Starting from a valid value is the point: each case fails for
    /// exactly the one reason it names, not because the password is wrong.
    #[test]
    fn verify_rejects_damaged_copies_of_a_valid_hash() {
        let dk_len = STORED_VECTOR.len();
        for (stored, why) in [
            (
                STORED_VECTOR.replace("pbkdf2-sha256$", "pbkdf2-sha1$"),
                "another algorithm's prefix",
            ),
            (
                STORED_VECTOR[..dk_len - 1].to_string(),
                "an odd-length digest",
            ),
            (
                STORED_VECTOR[..dk_len - 2].to_string(),
                "a truncated digest (31 bytes)",
            ),
            (
                format!("{STORED_VECTOR}00"),
                "an overlong digest (33 bytes)",
            ),
            (format!("{STORED_VECTOR}$extra"), "five fields"),
            (
                STORED_VECTOR.replacen("$1$", "$", 1),
                "three fields (no iteration count)",
            ),
            (
                STORED_VECTOR.replacen("$1$", "$x$", 1),
                "a non-numeric count",
            ),
            (STORED_VECTOR.replacen("$1$", "$-1$", 1), "a negative count"),
        ] {
            assert!(!verify_password("password", &stored), "{why}: {stored}");
        }
    }

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn tokens_are_unique_and_sized() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }
}
