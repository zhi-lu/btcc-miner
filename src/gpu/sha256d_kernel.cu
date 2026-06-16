// CUDA SHA-256d mining kernel.
// Compiled at build time to PTX via build.rs + nvcc.
// Target: compute_89 (RTX 4090).

#define ROTR32(x, n) (((x) >> (n)) | ((x) << (32 - (n))))

__device__ inline unsigned int bswap32(unsigned int x) {
    return __byte_perm(x, 0, 0x0123);
}

__constant__ unsigned int K[64] = {
    0x428a2f98u, 0x71374491u, 0xb5c0fbcfu, 0xe9b5dba5u,
    0x3956c25bu, 0x59f111f1u, 0x923f82a4u, 0xab1c5ed5u,
    0xd807aa98u, 0x12835b01u, 0x243185beu, 0x550c7dc3u,
    0x72be5d74u, 0x80deb1feu, 0x9bdc06a7u, 0xc19bf174u,
    0xe49b69c1u, 0xefbe4786u, 0x0fc19dc6u, 0x240ca1ccu,
    0x2de92c6fu, 0x4a7484aau, 0x5cb0a9dcu, 0x76f988dau,
    0x983e5152u, 0xa831c66du, 0xb00327c8u, 0xbf597fc7u,
    0xc6e00bf3u, 0xd5a79147u, 0x06ca6351u, 0x14292967u,
    0x27b70a85u, 0x2e1b2138u, 0x4d2c6dfcu, 0x53380d13u,
    0x650a7354u, 0x766a0abbu, 0x81c2c92eu, 0x92722c85u,
    0xa2bfe8a1u, 0xa81a664bu, 0xc24b8b70u, 0xc76c51a3u,
    0xd192e819u, 0xd6990624u, 0xf40e3585u, 0x106aa070u,
    0x19a4c116u, 0x1e376c08u, 0x2748774cu, 0x34b0bcb5u,
    0x391c0cb3u, 0x4ed8aa4au, 0x5b9cca4fu, 0x682e6ff3u,
    0x748f82eeu, 0x78a5636fu, 0x84c87814u, 0x8cc70208u,
    0x90befffau, 0xa4506cebu, 0xbef9a3f7u, 0xc67178f2u,
};

__constant__ unsigned int H0[8] = {
    0x6a09e667u, 0xbb67ae85u, 0x3c6ef372u, 0xa54ff53au,
    0x510e527fu, 0x9b05688cu, 0x1f83d9abu, 0x5be0cd19u,
};

__device__ inline void sha256_compress(
    unsigned int h[8],
    const unsigned int w_in[16]
) {
    unsigned int w[64];
    for (int i = 0; i < 16; i++) w[i] = w_in[i];
    for (int i = 16; i < 64; i++) {
        unsigned int s0 = ROTR32(w[i-15], 7) ^ ROTR32(w[i-15], 18) ^ (w[i-15] >> 3);
        unsigned int s1 = ROTR32(w[i-2], 17) ^ ROTR32(w[i-2], 19) ^ (w[i-2] >> 10);
        w[i] = w[i-16] + s0 + w[i-7] + s1;
    }
    unsigned int a=h[0], b=h[1], c=h[2], d=h[3],
                 e=h[4], f_=h[5], g=h[6], hh=h[7];
    for (int i = 0; i < 64; i++) {
        unsigned int S1 = ROTR32(e,6) ^ ROTR32(e,11) ^ ROTR32(e,25);
        unsigned int ch = (e & f_) ^ ((~e) & g);
        unsigned int t1 = hh + S1 + ch + K[i] + w[i];
        unsigned int S0 = ROTR32(a,2) ^ ROTR32(a,13) ^ ROTR32(a,22);
        unsigned int mj = (a & b) ^ (a & c) ^ (b & c);
        unsigned int t2 = S0 + mj;
        hh = g; g = f_; f_ = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
    }
    h[0]+=a; h[1]+=b; h[2]+=c; h[3]+=d;
    h[4]+=e; h[5]+=f_; h[6]+=g; h[7]+=hh;
}

extern "C" __global__ void sha256d_search(
    const unsigned int *__restrict__ midstate,    // 8 u32: SHA-256 state after chunk1
    const unsigned int *__restrict__ tail_words,  // 3 u32: chunk2 bytes 0..11 as BE u32
    const unsigned int *__restrict__ target_be,   // 8 u32: target as BE u32 (MSB first)
    unsigned int start_nonce,
    unsigned int *__restrict__ result_flag,
    unsigned int *__restrict__ result_nonce,
    unsigned int *__restrict__ result_hash,
    unsigned long long total_nonces
) {
    unsigned long long gid = blockIdx.x * (unsigned long long)blockDim.x + threadIdx.x;
    if (gid >= total_nonces) return;

    unsigned int nonce_val = start_nonce + (unsigned int)gid;

    // Build chunk2 message schedule (16 u32 BE words)
    unsigned int w[16];
    w[0] = tail_words[0];       // merkle_root[28..32] as BE u32
    w[1] = tail_words[1];       // ntime as BE u32
    w[2] = tail_words[2];       // nbits as BE u32
    w[3] = bswap32(nonce_val);  // nonce LE -> BE (SHA-256 reads BE words)
    w[4] = 0x80000000u;         // SHA-256 padding byte 0x80
    w[5] = 0; w[6] = 0; w[7] = 0;
    w[8] = 0; w[9] = 0; w[10] = 0; w[11] = 0;
    w[12] = 0; w[13] = 0; w[14] = 0;
    w[15] = 640u;               // 80 bytes * 8 bits

    // First SHA-256: resume from midstate with chunk2
    unsigned int h[8];
    for (int i = 0; i < 8; i++) h[i] = midstate[i];
    sha256_compress(h, w);

    // Second SHA-256: hash the 32-byte first-hash
    unsigned int w2[16];
    for (int i = 0; i < 8; i++) w2[i] = h[i];
    w2[8]  = 0x80000000u;
    w2[9]  = 0; w2[10] = 0; w2[11] = 0;
    w2[12] = 0; w2[13] = 0; w2[14] = 0;
    w2[15] = 256u;              // 32 bytes * 8 bits

    unsigned int h2[8];
    for (int i = 0; i < 8; i++) h2[i] = H0[i];
    sha256_compress(h2, w2);

    // Compare display hash (byte-reversed SHA-256d output) with target.
    // Display hash = bswap32(h2[7-i]) for i in 0..7, compared MSB first.
    bool below = false;
    bool decided = false;
    for (int i = 0; i < 8; i++) {
        unsigned int d = bswap32(h2[7 - i]);
        if (!decided) {
            if (d < target_be[i])      { below = true;  decided = true; }
            else if (d > target_be[i]) { below = false; decided = true; }
        }
    }
    if (!decided) below = true;  // equal counts as valid

    if (below) {
        // Atomically claim the result slot — only the first thread succeeds
        unsigned int expected = 0u;
        if (atomicCAS(result_flag, expected, 1u) == expected) {
            *result_nonce = nonce_val;
            for (int i = 0; i < 8; i++) {
                result_hash[i] = bswap32(h2[7 - i]);  // store display hash as BE u32
            }
        }
    }
}
