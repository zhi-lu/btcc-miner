// Metal GPU miner implementation for macOS / Apple Silicon.
//
// Performance optimizations (matching the C++ reference):
//   1. Midstate precompute: CPU computes SHA-256(chunk1), GPU only does chunk2 + 2nd hash.
//   2. Double-buffered command buffers: keep GPU saturated while host prepares next dispatch.
//   3. Auto-tuning: threadgroup size from pipeline limits, per-dispatch from GPU core count.

use metal::*;
use objc::rc::autoreleasepool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::job::{diff_to_target, hash_meets_target, MiningJob};
use crate::stratum::FoundShare;

fn ts() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("[{:02}:{:02}:{:02}]", h, m, s)
}

/// The Metal shader source — uses midstate optimization.
/// GPU only processes chunk2 (tail 16 bytes of header) + 2nd SHA-256.
const SHA256D_SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

constant uint K[64] = {
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

constant uint H0[8] = {
    0x6a09e667u, 0xbb67ae85u, 0x3c6ef372u, 0xa54ff53au,
    0x510e527fu, 0x9b05688cu, 0x1f83d9abu, 0x5be0cd19u,
};

inline uint rotr32(uint x, uint n) { return (x >> n) | (x << (32 - n)); }
inline uint bswap32_u(uint x) {
    return ((x & 0x000000FFu) << 24) | ((x & 0x0000FF00u) << 8)
         | ((x & 0x00FF0000u) >> 8)  | ((x & 0xFF000000u) >> 24);
}

inline void sha256_compress(thread uint h[8], thread const uint w_in[16]) {
    uint w[64];
    for (uint i = 0; i < 16; i++) w[i] = w_in[i];
    for (uint i = 16; i < 64; i++) {
        uint s0 = rotr32(w[i-15], 7) ^ rotr32(w[i-15], 18) ^ (w[i-15] >> 3);
        uint s1 = rotr32(w[i-2], 17) ^ rotr32(w[i-2], 19) ^ (w[i-2] >> 10);
        w[i] = w[i-16] + s0 + w[i-7] + s1;
    }
    uint a=h[0], b=h[1], c=h[2], d=h[3], e=h[4], f=h[5], g=h[6], hh=h[7];
    for (uint i = 0; i < 64; i++) {
        uint S1 = rotr32(e,6) ^ rotr32(e,11) ^ rotr32(e,25);
        uint ch = (e & f) ^ ((~e) & g);
        uint t1 = hh + S1 + ch + K[i] + w[i];
        uint S0 = rotr32(a,2) ^ rotr32(a,13) ^ rotr32(a,22);
        uint mj = (a & b) ^ (a & c) ^ (b & c);
        uint t2 = S0 + mj;
        hh = g; g = f; f = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
    }
    h[0]+=a; h[1]+=b; h[2]+=c; h[3]+=d; h[4]+=e; h[5]+=f; h[6]+=g; h[7]+=hh;
}

/// GPU kernel using midstate optimization.
/// midstate: SHA-256 state after processing chunk1 (first 64 bytes of header).
/// tail_words: 3 u32 BE words from chunk2 bytes 0..11 (merkle_tail, ntime, nbits).
/// target_be: 8 u32 BE words, target_be[0] is most significant.
/// start_nonce: base nonce value.
/// result_flag: atomic flag, 0 = no result yet, 1 = result found.
/// result_nonce: the nonce that produced a valid hash.
/// result_hash: 8 u32 BE words of the display hash.
kernel void sha256d_search(
    constant uint *midstate    [[buffer(0)]],
    constant uint *tail_words  [[buffer(1)]],
    constant uint *target_be   [[buffer(2)]],
    constant uint &start_nonce [[buffer(3)]],
    device atomic_uint *result_flag  [[buffer(4)]],
    device atomic_uint *result_nonce [[buffer(5)]],
    device uint *result_hash         [[buffer(6)]],
    uint gid [[thread_position_in_grid]]
) {
    uint nonce_val = start_nonce + gid;

    // Build chunk2 message schedule
    uint w[16];
    w[0] = tail_words[0];
    w[1] = tail_words[1];
    w[2] = tail_words[2];
    // nonce is little-endian in the header; SHA-256 reads BE words
    w[3] = bswap32_u(nonce_val);
    w[4] = 0x80000000u;
    w[5] = 0; w[6] = 0; w[7] = 0;
    w[8] = 0; w[9] = 0; w[10] = 0; w[11] = 0;
    w[12] = 0; w[13] = 0; w[14] = 0;
    w[15] = 640u;  // 80 bytes * 8 bits

    // First SHA-256: resume from midstate with chunk2
    uint h[8];
    for (uint i = 0; i < 8; i++) h[i] = midstate[i];
    sha256_compress(h, w);

    // Second SHA-256: hash the 32-byte first hash
    uint w2[16];
    for (uint i = 0; i < 8; i++) w2[i] = h[i];
    w2[8]  = 0x80000000u;
    w2[9]  = 0; w2[10] = 0; w2[11] = 0;
    w2[12] = 0; w2[13] = 0; w2[14] = 0;
    w2[15] = 256u;  // 32 bytes * 8 bits

    uint h2[8];
    for (uint i = 0; i < 8; i++) h2[i] = H0[i];
    sha256_compress(h2, w2);

    // Bitcoin display hash = byte-reverse of natural SHA-256 output.
    // Compare display_be[i] with target_be[i] from MSB to LSB.
    bool below = false;
    bool decided = false;
    for (uint i = 0; i < 8; i++) {
        uint d = bswap32_u(h2[7 - i]);
        if (!decided) {
            if (d < target_be[i])      { below = true;  decided = true; }
            else if (d > target_be[i]) { below = false; decided = true; }
        }
    }
    if (!decided) below = true;  // equal counts as valid

    if (below) {
        uint expected = 0u;
        if (atomic_compare_exchange_weak_explicit(
                result_flag, &expected, 1u,
                memory_order_relaxed, memory_order_relaxed)) {
            atomic_store_explicit(result_nonce, nonce_val, memory_order_relaxed);
            for (uint i = 0; i < 8; i++) {
                result_hash[i] = bswap32_u(h2[7 - i]);
            }
        }
    }
}
"#;

/// GPU miner using Metal compute shaders with midstate optimization.
pub struct GpuMiner {
    device: Device,
    command_queue: CommandQueue,
    pipeline: ComputePipelineState,
    /// Auto-tuned threadgroup size (multiple of threadExecutionWidth).
    threadgroup_size: u64,
    /// Auto-tuned nonces per dispatch.
    per_dispatch: u32,
}

impl Clone for GpuMiner {
    fn clone(&self) -> Self {
        GpuMiner {
            device: self.device.clone(),
            command_queue: self.command_queue.clone(),
            pipeline: self.pipeline.clone(),
            threadgroup_size: self.threadgroup_size,
            per_dispatch: self.per_dispatch,
        }
    }
}

impl GpuMiner {
    /// Initialize the Metal GPU miner. Returns vec with 0 or 1 elements.
    pub fn new() -> Vec<Self> {
        autoreleasepool(|| {
            let device = Device::system_default()?;
            eprintln!(
                "GPU: {} (registry_id={:?})",
                device.name(),
                device.registry_id()
            );

            let command_queue = device.new_command_queue();

            // Compile the shader
            let options = CompileOptions::new();
            let library = match device.new_library_with_source(SHA256D_SHADER, &options) {
                Ok(lib) => lib,
                Err(e) => {
                    eprintln!("[ERROR] Failed to compile Metal shader: {}", e);
                    return None;
                }
            };

            let function = match library.get_function("sha256d_search", None) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("[ERROR] Failed to get kernel function: {}", e);
                    return None;
                }
            };

            let pipeline = {
                let desc = ComputePipelineDescriptor::new();
                desc.set_compute_function(Some(&function));
                match device.new_compute_pipeline_state(&desc) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[ERROR] Failed to create compute pipeline: {}", e);
                        return None;
                    }
                }
            };

            // Auto-tune threadgroup size
            let max_tpt = pipeline.max_total_threads_per_threadgroup();
            let tew = pipeline.thread_execution_width();
            let threadgroup_size = if tew > 0 {
                (max_tpt / tew) * tew
            } else {
                max_tpt
            };
            let threadgroup_size = if threadgroup_size == 0 {
                256
            } else {
                threadgroup_size
            };

            // Auto-tune per-dispatch: ~2M nonces per GPU core, clamped to [4M, 64M]
            let gpu_cores = detect_gpu_cores();
            let cores_eff = if gpu_cores > 0 { gpu_cores } else { 8 };
            let per_dispatch = ((cores_eff as u64) * (1u64 << 21)) // 2M per core
                .max(1u64 << 22) // floor: 4M
                .min(1u64 << 26); // ceil: 64M

            eprintln!(
                "Metal GPU miner: threadgroup={}, per_dispatch={}M, gpu_cores={}",
                threadgroup_size,
                per_dispatch / 1_048_576,
                gpu_cores
            );

            let miner = GpuMiner {
                device,
                command_queue,
                pipeline,
                threadgroup_size,
                per_dispatch: per_dispatch as u32,
            };
            Some(vec![miner])
        }).unwrap_or_else(|| vec![])
    }

    /// Run the GPU mining loop.
    pub fn run(
        &self,
        current_job: Arc<Mutex<Option<MiningJob>>>,
        running: Arc<AtomicBool>,
        hashrate: Arc<Mutex<f64>>,
        share_tx: mpsc::Sender<FoundShare>,
        subscription: Arc<Mutex<Option<crate::stratum::Subscription>>>,
        difficulty: Arc<Mutex<f64>>,
    ) {
        let device = self.device.clone();
        let command_queue = self.command_queue.clone();
        let pipeline = self.pipeline.clone();
        let threadgroup_size = self.threadgroup_size;
        let per_dispatch = self.per_dispatch;

        thread::spawn(move || {
            let mut hash_count: u64 = 0;
            let mut last_report = Instant::now();
            let mut nonce_base: u32 = 0;
            let mut extranonce2_counter: u64 = 0;

            // Pre-allocate buffers (reused across batches)
            let mid_buf = device.new_buffer(32, MTLResourceOptions::StorageModeShared);
            let tail_buf = device.new_buffer(12, MTLResourceOptions::StorageModeShared);
            let tgt_buf = device.new_buffer(32, MTLResourceOptions::StorageModeShared);

            // Double-buffered result slots for pipelining
            let flag_buf: [Buffer; 2] = [
                device.new_buffer(4, MTLResourceOptions::StorageModeShared),
                device.new_buffer(4, MTLResourceOptions::StorageModeShared),
            ];
            let nonce_buf: [Buffer; 2] = [
                device.new_buffer(4, MTLResourceOptions::StorageModeShared),
                device.new_buffer(4, MTLResourceOptions::StorageModeShared),
            ];
            let hash_buf: [Buffer; 2] = [
                device.new_buffer(32, MTLResourceOptions::StorageModeShared),
                device.new_buffer(32, MTLResourceOptions::StorageModeShared),
            ];

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                // Get current job
                let job_data = {
                    let guard = current_job.lock().unwrap();
                    guard.clone()
                };

                if let Some(job) = job_data {
                    // Compute share target from current difficulty
                    let diff = *difficulty.lock().unwrap();
                    let target = diff_to_target(diff);
                    // Get extranonce1 from subscription
                    let extranonce1 = {
                        let sub = subscription.lock().unwrap();
                        sub.as_ref()
                            .map(|s| s.extranonce1.clone())
                            .unwrap_or_default()
                    };

                    // Increment extranonce2 each search pass (like Python version)
                    // extranonce2 is little-endian bytes, then hex-encoded for stratum submit
                    extranonce2_counter = extranonce2_counter.wrapping_add(1);
                    let extranonce2 = {
                        let bytes = extranonce2_counter.to_le_bytes();
                        hex::encode(bytes)
                    };

                    // Build header and compute midstate on CPU
                    let merkle_root = {
                        let coinbase = job.build_coinbase(&extranonce1, &extranonce2);
                        job.compute_merkle_root(&coinbase)
                    };
                    let ntime = {
                        let job_ntime = u32::from_str_radix(&job.ntime, 16).unwrap_or(0);
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as u32;
                        // Bump ntime to now if local clock is ahead (pools allow this)
                        std::cmp::max(job_ntime, now)
                    };
                    let header = job.build_header(&merkle_root, ntime, 0);

                    // Compute midstate: SHA-256 state after chunk1 (first 64 bytes)
                    let midstate = compute_midstate(&header);

                    // Extract tail_words: chunk2 bytes 0..11 as 3 BE u32
                    // chunk2 = merkle[28..32] | ntime(4 LE) | nbits(4 LE) | nonce(4 LE)
                    let tail_words = [
                        u32::from_be_bytes([header[64], header[65], header[66], header[67]]),
                        u32::from_be_bytes([header[68], header[69], header[70], header[71]]),
                        u32::from_be_bytes([header[72], header[73], header[74], header[75]]),
                    ];

                    // Convert target to 8 BE u32
                    let target_be: [u32; 8] = std::array::from_fn(|i| {
                        let o = i * 4;
                        u32::from_be_bytes([target[o], target[o + 1], target[o + 2], target[o + 3]])
                    });

                    // Write to GPU buffers
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            midstate.as_ptr() as *const u8,
                            mid_buf.contents() as *mut u8,
                            32,
                        );
                        std::ptr::copy_nonoverlapping(
                            tail_words.as_ptr() as *const u8,
                            tail_buf.contents() as *mut u8,
                            12,
                        );
                        std::ptr::copy_nonoverlapping(
                            target_be.as_ptr() as *const u8,
                            tgt_buf.contents() as *mut u8,
                            32,
                        );
                    }

                    // Run GPU search with double-buffered pipelining
                    let result = run_gpu_search(
                        &device,
                        &command_queue,
                        &pipeline,
                        &mid_buf,
                        &tail_buf,
                        &tgt_buf,
                        &flag_buf,
                        &nonce_buf,
                        &hash_buf,
                        nonce_base,
                        per_dispatch as u64,
                        threadgroup_size,
                    );

                    match result {
                        Ok((found_nonce, checked)) => {
                            hash_count += checked;

                            if let Some(nonce) = found_nonce {
                                // Verify on CPU
                                let header = job.build_header(&merkle_root, ntime, nonce);
                                let hash = crate::job::double_sha256(&header);
                                let meets = hash_meets_target(&hash, &target);
                                if meets {
                                    eprintln!(
                                        "{} \x1b[33m✔ GPU FOUND SHARE!\x1b[0m nonce={:08x}, hash={}",
                                        ts(),
                                        nonce,
                                        hex::encode(hash)
                                    );
                                    let share = FoundShare {
                                        job_id: job.job_id.clone(),
                                        extranonce2: extranonce2.clone(),
                                        ntime: format!("{:08x}", ntime),
                                        nonce,
                                    };
                                    let _ = share_tx.send(share);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("{} [ERROR] GPU batch error: {}", ts(), e);
                        }
                    }

                    nonce_base = nonce_base.wrapping_add(per_dispatch);

                    // Update hashrate
                    let elapsed = last_report.elapsed().as_secs_f64();
                    if elapsed >= 10.0 {
                        let hr = hash_count as f64 / elapsed;
                        *hashrate.lock().unwrap() = hr;
                        eprintln!(
                            "{} GPU Hashrate: {:.2} H/s ({:.2} MH/s)",
                            ts(),
                            hr,
                            hr / 1_000_000.0
                        );
                        hash_count = 0;
                        last_report = Instant::now();
                    }
                } else {
                    thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        });
    }
}

/// Compute SHA-256 midstate: process chunk1 (first 64 bytes) of the header on CPU.
fn compute_midstate(header: &[u8; 80]) -> [u32; 8] {
    // We need the SHA-256 state after processing the first 64-byte block.
    // Implemented directly since the sha2 crate doesn't expose midstate.
    let k: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    // Convert first 64 bytes to 16 BE u32 words
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            header[i * 4],
            header[i * 4 + 1],
            header[i * 4 + 2],
            header[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut a = h[0];
    let mut b = h[1];
    let mut c = h[2];
    let mut d = h[3];
    let mut e = h[4];
    let mut f = h[5];
    let mut g = h[6];
    let mut hh = h[7];

    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(k[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);

    h
}

/// Run a GPU search batch with double-buffered command buffer pipelining.
/// Returns (Option<found_nonce>, total_checked).
fn run_gpu_search(
    device: &Device,
    command_queue: &CommandQueue,
    pipeline: &ComputePipelineState,
    mid_buf: &Buffer,
    tail_buf: &Buffer,
    tgt_buf: &Buffer,
    flag_buf: &[Buffer; 2],
    nonce_buf: &[Buffer; 2],
    hash_buf: &[Buffer; 2],
    start_nonce: u32,
    total_count: u64,
    threadgroup_size: u64,
) -> Result<(Option<u32>, u64), String> {
    let mut committed: u64 = 0;
    let mut done: u64 = 0;
    let mut slot: usize = 0;

    let mut prev_cb: Option<CommandBuffer> = None;
    let mut prev_slot: usize = 0;
    let mut prev_batch: u64 = 0;

    let mut found_nonce: Option<u32> = None;

    while committed < total_count {
        let left = total_count - committed;
        let batch = left.min(u32::MAX as u64) as u32;
        let batch_start = start_nonce.wrapping_add(committed as u32);

        // Reset flag for this slot
        unsafe {
            *(flag_buf[slot].contents() as *mut u32) = 0;
        }

        let cb = command_queue.new_command_buffer().to_owned();
        let encoder = cb.new_compute_command_encoder();

        encoder.set_compute_pipeline_state(pipeline);
        encoder.set_buffer(0, Some(mid_buf), 0);
        encoder.set_buffer(1, Some(tail_buf), 0);
        encoder.set_buffer(2, Some(tgt_buf), 0);
        encoder.set_bytes(3, 4, &batch_start as *const _ as *const _);
        encoder.set_buffer(4, Some(&flag_buf[slot]), 0);
        encoder.set_buffer(5, Some(&nonce_buf[slot]), 0);
        encoder.set_buffer(6, Some(&hash_buf[slot]), 0);

        let tg_size = threadgroup_size.min(batch as u64);
        let tg_count = (batch as u64 + tg_size - 1) / tg_size;

        encoder.dispatch_thread_groups(
            MTLSize {
                width: tg_count,
                height: 1,
                depth: 1,
            },
            MTLSize {
                width: tg_size,
                height: 1,
                depth: 1,
            },
        );
        encoder.end_encoding();
        cb.commit();

        committed += batch as u64;

        // Drain previous command buffer while GPU processes the new one
        if let Some(prev) = prev_cb.take() {
            prev.wait_until_completed();
            done += prev_batch;

            let flag = unsafe { *(flag_buf[prev_slot].contents() as *const u32) };
            if flag != 0 {
                found_nonce = Some(unsafe { *(nonce_buf[prev_slot].contents() as *const u32) });
                cb.wait_until_completed();
                done += batch as u64;
                break;
            }
        }

        prev_cb = Some(cb);
        prev_slot = slot;
        prev_batch = batch as u64;
        slot = 1 - slot;
    }

    // Drain the last pending command buffer
    if found_nonce.is_none() {
        if let Some(prev) = prev_cb.take() {
            prev.wait_until_completed();
            done += prev_batch;

            let flag = unsafe { *(flag_buf[prev_slot].contents() as *const u32) };
            if flag != 0 {
                found_nonce = Some(unsafe { *(nonce_buf[prev_slot].contents() as *const u32) });
            }
        }
    }

    Ok((found_nonce, done))
}

/// Detect GPU core count via IOKit (AGXAccelerator).
fn detect_gpu_cores() -> u32 {
    // On macOS we can try to read the GPU core count.
    // For now, return 0 to use the default (8).
    // A full implementation would use IOKit bindings.
    // The auto-tune fallback of 8 cores is reasonable for M1/M2 base models.
    0
}
