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

        // Previous block hash (32 bytes, reversed)
        let prev_hash = hex::decode(&self.prev_hash).unwrap();
        for i in 0..32 {
            header[4 + i] = prev_hash[31 - i];
        }

        // Merkle root (32 bytes, reversed)
        for i in 0..32 {
            header[36 + i] = merkle_root[31 - i];
        }

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
    let bytes = val.to_be_bytes(); // 16 bytes (u128), value right-aligned
                                   // val represents the mantissa; target = val * 2^208
                                   // So val's bytes go to result[4..], with val's MSB at result[4]
                                   // Find how many bytes val actually occupies (skip leading zeros, but keep at least 1)
    let mut first_nonzero = 0;
    while first_nonzero < 15 && bytes[first_nonzero] == 0 {
        first_nonzero += 1;
    }
    // Copy from first_nonzero to end, placing at result[4..]
    for j in 0..(16 - first_nonzero) {
        let d = 4 + j;
        if d < 32 {
            result[d] = bytes[first_nonzero + j];
        }
    }
    result
}
