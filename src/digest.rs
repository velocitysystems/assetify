//! Content digests: the expected hash a resolver states for a file,
//! and parsing it from hex. The engine computes and checks the digest
//! during acquisition (see [`crate::Fetcher`]); this module only holds
//! the expectation and the comparison.

use thiserror::Error;

/// An expected content digest, as a resolver states it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Digest {
   /// SHA-256 of the file's bytes.
   Sha256([u8; 32]),
}

impl Digest {
   /// Parse a SHA-256 digest from its usual lowercase-or-uppercase
   /// hex spelling (64 characters).
   pub fn sha256_hex(hex_digest: &str) -> Result<Self, InvalidDigest> {
      let bytes = hex::decode(hex_digest).map_err(|_| InvalidDigest {
         detail: "not valid hexadecimal".to_string(),
      })?;
      let bytes: [u8; 32] = bytes.try_into().map_err(|_| InvalidDigest {
         detail: "a SHA-256 digest is 64 hex characters".to_string(),
      })?;
      Ok(Digest::Sha256(bytes))
   }

   /// Whether a computed SHA-256 matches this expectation.
   pub(crate) fn matches_sha256(&self, computed: &[u8; 32]) -> bool {
      match self {
         Digest::Sha256(expected) => expected == computed,
      }
   }
}

/// A digest string that could not be parsed.
#[derive(Debug, Error)]
#[error("invalid digest: {detail}")]
pub struct InvalidDigest {
   detail: String,
}

#[cfg(test)]
mod tests {
   use sha2::Digest as _;

   use super::*;

   #[test]
   fn parses_and_matches_a_sha256_hex_digest() {
      // SHA-256 of the empty input, a well-known vector.
      let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
      let digest = Digest::sha256_hex(empty).unwrap();

      let computed: [u8; 32] = sha2::Sha256::digest(b"").into();
      assert!(digest.matches_sha256(&computed));

      let other: [u8; 32] = sha2::Sha256::digest(b"x").into();
      assert!(!digest.matches_sha256(&other));
   }

   #[test]
   fn rejects_malformed_digest_strings() {
      assert!(Digest::sha256_hex("not-hex").is_err());
      assert!(Digest::sha256_hex("abcd").is_err(), "too short");
   }
}
