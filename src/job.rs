use serde::{Deserialize, Serialize};

/// Represents a mining job received from the stratum server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningJob {
    pub job_id: String,
    pub prev_hash: String,
    pub coinb1: String,
    pub coinb2: String,
    pub merkle_branch: Vec<String>,
    pub version: String,
    pub nbits: String,
    pub ntime: String,
    pub clean_jobs: Option<bool>,
}

impl MiningJob {
    /// Build the coinbase transaction (coinb1 + extranonce1 + extranonce2 + coinb2).
    pub fn build_coinbase(&self, extranonce1: &str, extranonce2: &str) -> String {
        format!(
            "{}{}{}{}",
            self.coinb1, extranonce1, extranonce2, self.coinb2
        )
    }

    /// Compute the Merkle root from the coinbase and merkle branch.
    pub fn compute_merkle_root(&self, coinbase_hex: &str) -> [u8; 32] {
        let coinbase_hash = double_sha256(&hex::decode(coinbase_hex).unwrap());

        let mut current = coinbase_hash;
        for branch in &self.merkle_branch {
            let branch_bytes = hex::decode(branch).unwrap();
            let mut combined = Vec::with_capacity(64);
            combined.extend_from_slice(&current);
            combined.extend_from_slice(&branch_bytes);
            current = double_sha256(&combined);
        }
        current
    }

    /// Build the 80-byte block header for hashing.
    pub fn build_header(&self, merkle_root: &[u8; 32], ntime: u32, nonce: u32) -> [u8; 80] {
        let mut header = [0u8; 80];

        // Version (4 bytes, little-endian)
        let version = u32::from_str_radix(&self.version, 16).unwrap_or(0x20000000);
        header[0..4].copy_from_slice(&version.to_le_bytes());

        // Previous block hash (32 bytes).
        // Stratum sends prevhash as 32 bytes whose 8 4-byte words are each
        // in big-endian order. To produce the canonical little-endian-on-the-wire
        // field, reverse each 4-byte word in place (matching Python reference).
        let prev_hash = hex::decode(&self.prev_hash).unwrap();
        for i in 0..8 {
            let base = 4 + i * 4;
            header[base..base + 4].copy_from_slice(&[
                prev_hash[base - 4 + 3],
                prev_hash[base - 4 + 2],
                prev_hash[base - 4 + 1],
                prev_hash[base - 4 + 0],
            ]);
        }

        // Merkle root (32 bytes) — sha256d output is already in internal
        // byte order; copy directly (matching Python reference).
        header[36..68].copy_from_slice(merkle_root);

        // Time (4 bytes, little-endian)
        header[68..72].copy_from_slice(&ntime.to_le_bytes());

        // Bits / difficulty target (4 bytes)
        let nbits = u32::from_str_radix(&self.nbits, 16).unwrap_or(0x1d00ffff);
        header[72..76].copy_from_slice(&nbits.to_le_bytes());

        // Nonce (4 bytes, little-endian)
        header[76..80].copy_from_slice(&nonce.to_le_bytes());

        header
    }
}

/// Double SHA-256 hash.
pub fn double_sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let hash1 = Sha256::digest(data);
    let hash2 = Sha256::digest(&hash1);
    let mut result = [0u8; 32];
    result.copy_from_slice(&hash2);
    result
}

/// Check if a hash meets the target difficulty.
/// Both hash and target are compared as big-endian 256-bit integers.
pub fn hash_meets_target(hash: &[u8; 32], target: &[u8; 32]) -> bool {
    for i in 0..32 {
        match hash[i].cmp(&target[i]) {
            std::cmp::Ordering::Less => return true,
            std::cmp::Ordering::Greater => return false,
            std::cmp::Ordering::Equal => continue,
        }
    }
    true // equal also counts
}

/// Convert nbits (compact target) to a 32-byte big-endian target.
pub fn nbits_to_target(nbits: u32) -> [u8; 32] {
    let exponent = (nbits >> 24) as usize;
    let mantissa = nbits & 0x00ff_ffff;

    let mut target = [0u8; 32];
    if exponent <= 3 {
        // Very small target
        let shift = 8 * (3 - exponent);
        let val = mantissa >> shift;
        target[29..32].copy_from_slice(&val.to_be_bytes());
    } else {
        let start = 32 - exponent;
        target[start..start + 3].copy_from_slice(&mantissa.to_be_bytes()[1..4]);
    }
    target
}

/// Convert pool share difficulty to a 32-byte big-endian target.
/// target = DIFF1_TARGET / difficulty
pub fn diff_to_target(diff: f64) -> [u8; 32] {
    if diff <= 0.0 {
        return [0xFF; 32];
    }
    // DIFF1_TARGET = 0x00000000FFFF0000000000000000000000000000000000000000000000000000
    // = 0xFFFF * 2^208
    // target = DIFF1 / diff = (0xFFFF * 2^208) / diff
    //
    // For integer difficulty: target = (0xFFFF / N) * 2^208 + ((0xFFFF % N) * 2^208) / N
    // The first term gives bytes 4-5, the second fills bytes 6+.
    //
    // For fractional difficulty, we scale up and then shift down.

    // Scale diff to integer with 32-bit fractional precision
    let diff_scaled = (diff * (1u64 << 32) as f64) as u64;
    if diff_scaled == 0 {
        return [0xFF; 32];
    }

    // Compute (0xFFFF * 2^240) / diff_scaled
    // This gives us the target mantissa shifted left by 240-208=32 bits
    // Actually: (0xFFFF * 2^208) / (diff_scaled / 2^32) = (0xFFFF * 2^240) / diff_scaled
    let val: u128 = (0xFFFFu128 << 32) / diff_scaled as u128;
    // val is now the target's significant part (up to 48 bits: 16 bits from 0xFFFF + 32 bits fraction)
    // The top 16 bits of val go to bytes 4-5, the lower bits fill bytes 6+

    let mut result = [0u8; 32];
    // val = 0xFFFF / diff, so val is in range [0, 0xFFFF]
    // 0xFFFF occupies 2 bytes. val always occupies 2 bytes in the target.
    // Place the last 2 bytes of val.to_be_bytes() at result[4..5]
    let bytes = val.to_be_bytes();
    result[4] = bytes[14];
    result[5] = bytes[15];
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that prev_hash is per-4-byte-word reversed, matching Python's
    /// stratum_prevhash_to_le32.
    #[test]
    fn test_prev_hash_per_word_reverse() {
        let job = MiningJob {
            job_id: "test".into(),
            // Each 8-char group is one 4-byte word in big-endian hex.
            // Word 0: 0x11223344, Word 1: 0x55667788, ... Word 7: 0xFFEEDDCC
            prev_hash: "112233445566778899aabbccddeeff0000112233445566778899aabbffeeddcc".into(),
            coinb1: "".into(),
            coinb2: "".into(),
            merkle_branch: vec![],
            version: "20000000".into(),
            nbits: "1d00ffff".into(),
            ntime: "00000000".into(),
            clean_jobs: None,
        };

        let merkle_root = [0u8; 32];
        let header = job.build_header(&merkle_root, 0, 0);

        // After per-4-byte-word reversal:
        // Word 0: 0x11223344 -> bytes [0x44, 0x33, 0x22, 0x11]
        assert_eq!(header[4], 0x44);
        assert_eq!(header[5], 0x33);
        assert_eq!(header[6], 0x22);
        assert_eq!(header[7], 0x11);

        // Word 1: 0x55667788 -> bytes [0x88, 0x77, 0x66, 0x55]
        assert_eq!(header[8], 0x88);
        assert_eq!(header[9], 0x77);
        assert_eq!(header[10], 0x66);
        assert_eq!(header[11], 0x55);

        // Word 7: 0xFFEEDDCC -> bytes [0xCC, 0xDD, 0xEE, 0xFF]
        assert_eq!(header[32], 0xCC);
        assert_eq!(header[33], 0xDD);
        assert_eq!(header[34], 0xEE);
        assert_eq!(header[35], 0xFF);
    }

    /// Verify merkle_root is copied directly (no reversal).
    #[test]
    fn test_merkle_root_direct_copy() {
        let job = MiningJob {
            job_id: "test".into(),
            prev_hash: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            coinb1: "".into(),
            coinb2: "".into(),
            merkle_branch: vec![],
            version: "20000000".into(),
            nbits: "1d00ffff".into(),
            ntime: "00000000".into(),
            clean_jobs: None,
        };

        let mut merkle_root = [0u8; 32];
        merkle_root[0] = 0xAA;
        merkle_root[31] = 0xBB;

        let header = job.build_header(&merkle_root, 0, 0);

        // Direct copy: header[36] == merkle_root[0], header[67] == merkle_root[31]
        assert_eq!(header[36], 0xAA);
        assert_eq!(header[67], 0xBB);
    }
}
