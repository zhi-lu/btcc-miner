// OpenCL GPU miner — cross-platform (NVIDIA / AMD / Intel GPUs).
//
// Uses the `ocl` crate for Rust ↔ OpenCL bindings.
// Core SHA-256d logic identical to Metal / CUDA implementations.
//
// Performance optimizations:
//   1. Midstate precompute: CPU computes SHA-256(chunk1), GPU only chunk2 + 2nd hash.
//   2. Double-buffered OpenCL queues: keep GPU saturated across dispatches.
//   3. Auto-tuning: workgroup size 256, per_dispatch from compute units × 2M.

use ocl::{flags, Buffer, Context, Device, DeviceType, Kernel, Platform, Program, Queue};
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

// ─── OpenCL kernel (same algorithm as CUDA / Metal) ────────────────────

const SHA256D_KERNEL: &str = r#"
__constant uint K[64] = {
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

__constant uint H0[8] = {
    0x6a09e667u, 0xbb67ae85u, 0x3c6ef372u, 0xa54ff53au,
    0x510e527fu, 0x9b05688cu, 0x1f83d9abu, 0x5be0cd19u,
};

inline uint rotr32(uint x, uint n) { return (x >> n) | (x << (32 - n)); }

inline uint bswap32_u(uint x) {
    return ((x & 0x000000FFu) << 24) | ((x & 0x0000FF00u) << 8)
         | ((x & 0x00FF0000u) >> 8)  | ((x & 0xFF000000u) >> 24);
}

inline void sha256_compress(uint h[8], const uint w_in[16]) {
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

__kernel void sha256d_search(
    __global const uint *midstate,
    __global const uint *tail_words,
    __global const uint *target_be,
    uint start_nonce,
    __global volatile uint *result_flag,
    __global uint *result_nonce,
    __global uint *result_hash
) {
    uint nonce_val = start_nonce + get_global_id(0);

    uint w[16];
    w[0] = tail_words[0];
    w[1] = tail_words[1];
    w[2] = tail_words[2];
    w[3] = bswap32_u(nonce_val);
    w[4] = 0x80000000u;
    w[5] = 0; w[6] = 0; w[7] = 0;
    w[8] = 0; w[9] = 0; w[10] = 0; w[11] = 0;
    w[12] = 0; w[13] = 0; w[14] = 0;
    w[15] = 640u;

    uint h[8];
    for (uint i = 0; i < 8; i++) h[i] = midstate[i];
    sha256_compress(h, w);

    uint w2[16];
    for (uint i = 0; i < 8; i++) w2[i] = h[i];
    w2[8]  = 0x80000000u;
    w2[9]  = 0; w2[10] = 0; w2[11] = 0;
    w2[12] = 0; w2[13] = 0; w2[14] = 0;
    w2[15] = 256u;

    uint h2[8];
    for (uint i = 0; i < 8; i++) h2[i] = H0[i];
    sha256_compress(h2, w2);

    bool below = false;
    bool decided = false;
    for (uint i = 0; i < 8; i++) {
        uint d = bswap32_u(h2[7 - i]);
        if (!decided) {
            if (d < target_be[i])      { below = true;  decided = true; }
            else if (d > target_be[i]) { below = false; decided = true; }
        }
    }
    if (!decided) below = true;

    if (below) {
        uint expected = 0u;
        if (atomic_cmpxchg(result_flag, expected, 1u) == expected) {
            *result_nonce = nonce_val;
            for (uint i = 0; i < 8; i++) {
                result_hash[i] = bswap32_u(h2[7 - i]);
            }
        }
    }
}
"#;

// ─── GpuMiner ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct GpuMiner {
    /// Device index for logging.
    device_index: u32,
    device_name: String,
    /// OpenCL context (must outlive queue & program — kept for lifetime).
    #[allow(dead_code)]
    context: Context,
    queue: Queue,
    program: Program,
    workgroup_size: usize,
    per_dispatch: u32,
    gpu_usage: u32,
}

impl GpuMiner {
    /// Enumerate all OpenCL GPUs, filter by `gpu_devices`, return configured miners.
    /// If `gpu_devices` is empty, returns all available GPUs.
    pub fn new(gpu_devices: &[u32], gpu_usage: u32) -> Vec<Self> {
        let gpu_usage = gpu_usage.clamp(1, 100);

        // Collect all GPU devices across all platforms
        let mut all_gpus: Vec<(u32, Device)> = Vec::new();
        for platform in Platform::list() {
            if let Ok(devices) = Device::list(&platform, Some(DeviceType::GPU)) {
                for d in devices {
                    all_gpus.push((all_gpus.len() as u32, d));
                }
            }
        }

        if all_gpus.is_empty() {
            eprintln!("[ERROR] No OpenCL GPU devices found.");
            return vec![];
        }

        // Filter by gpu_devices if specified
        let selected: Vec<(u32, Device)> = if gpu_devices.is_empty() {
            all_gpus
        } else {
            all_gpus
                .into_iter()
                .filter(|(idx, _)| gpu_devices.contains(idx))
                .collect()
        };

        let mut miners = Vec::new();
        for (idx, device) in selected {
            match Self::init_device(idx, device, gpu_usage) {
                Some(m) => miners.push(m),
                None => eprintln!("[WARN] GPU #{} initialization failed, skipping", idx),
            }
        }

        if miners.is_empty() {
            eprintln!("[ERROR] No OpenCL GPUs could be initialized.");
        }
        miners
    }

    fn init_device(idx: u32, device: Device, gpu_usage: u32) -> Option<Self> {
        let name = device.name().unwrap_or_else(|_| "unknown".into());
        let vendor = device.vendor().unwrap_or_else(|_| "unknown".into());

        let context = Context::builder()
            .devices(device.clone())
            .build()
            .map_err(|e| eprintln!("[ERROR] GPU #{}: context: {}", idx, e))
            .ok()?;

        let queue = Queue::new(&context, device.clone(), None)
            .map_err(|e| eprintln!("[ERROR] GPU #{}: queue: {}", idx, e))
            .ok()?;

        let program = Program::builder()
            .src(SHA256D_KERNEL)
            .devices(device.clone())
            .build(&context)
            .map_err(|e| eprintln!("[ERROR] GPU #{}: program build: {}", idx, e))
            .ok()?;

        let max_wg = device.max_wg_size().unwrap_or(256);
        let workgroup_size = if max_wg >= 256 { 256 } else { max_wg };

        // Auto-tune per_dispatch from compute units
        let cu = device
            .info(ocl::enums::DeviceInfo::MaxComputeUnits)
            .map(|r| match r {
                ocl::enums::DeviceInfoResult::MaxComputeUnits(n) => n as u64,
                _ => 8,
            })
            .unwrap_or(8);
        let per_dispatch = (cu * (1u64 << 21)).max(1u64 << 22).min(1u64 << 26);

        eprintln!(
            "OpenCL GPU #{}: {} (vendor={}, CUs={})",
            idx, name, vendor, cu
        );
        eprintln!(
            "  workgroup={}, per_dispatch={}M nonces",
            workgroup_size,
            per_dispatch / 1_048_576
        );

        Some(GpuMiner {
            device_index: idx,
            device_name: name,
            context,
            queue,
            program,
            workgroup_size,
            per_dispatch: per_dispatch as u32,
            gpu_usage,
        })
    }

    /// Spawn a mining thread for this GPU.
    pub fn run(
        &self,
        current_job: Arc<Mutex<Option<MiningJob>>>,
        running: Arc<AtomicBool>,
        hashrate: Arc<Mutex<f64>>,
        share_tx: mpsc::Sender<FoundShare>,
        subscription: Arc<Mutex<Option<crate::stratum::Subscription>>>,
        difficulty: Arc<Mutex<f64>>,
    ) {
        let queue = self.queue.clone();
        let program = self.program.clone();
        let workgroup_size = self.workgroup_size;
        let per_dispatch = self.per_dispatch;
        let device_index = self.device_index;
        let device_name = self.device_name.clone();
        let gpu_usage = self.gpu_usage;

        thread::spawn(move || {
            // Allocate buffers — all MEM_READ_WRITE to avoid NVIDIA OpenCL quirk
            // with MEM_HOST_WRITE_ONLY not updating correctly on some drivers
            let make_buf = |len: usize| -> Buffer<u32> {
                Buffer::<u32>::builder()
                    .queue(queue.clone())
                    .flags(flags::MEM_READ_WRITE)
                    .len(len)
                    .fill_val(0u32)
                    .build()
                    .unwrap()
            };

            let mid_buf = make_buf(8);
            let tail_buf = make_buf(3);
            let tgt_buf = make_buf(8);
            let flag_buf: [Buffer<u32>; 2] = [make_buf(1), make_buf(1)];
            let nonce_buf: [Buffer<u32>; 2] = [make_buf(1), make_buf(1)];
            let hash_buf: [Buffer<u32>; 2] = [make_buf(8), make_buf(8)];

            let mut hash_count: u64 = 0;
            let mut last_report = Instant::now();
            let mut nonce_base: u32 = 0;
            let mut extranonce2_counter: u64 = 0;

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                let job_data = {
                    let guard = current_job.lock().unwrap();
                    guard.clone()
                };

                if let Some(job) = job_data {
                    let diff = *difficulty.lock().unwrap();
                    let target = diff_to_target(diff);
                    let extranonce1 = {
                        let sub = subscription.lock().unwrap();
                        sub.as_ref().map(|s| s.extranonce1.clone()).unwrap_or_default()
                    };

                    extranonce2_counter = extranonce2_counter.wrapping_add(1);
                    let extranonce2 = {
                        let bytes = extranonce2_counter.to_le_bytes();
                        hex::encode(bytes)
                    };

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
                        std::cmp::max(job_ntime, now)
                    };
                    let header = job.build_header(&merkle_root, ntime, 0);

                    let midstate = compute_midstate(&header);
                    let tail_words = [
                        u32::from_be_bytes([header[64], header[65], header[66], header[67]]),
                        u32::from_be_bytes([header[68], header[69], header[70], header[71]]),
                        u32::from_be_bytes([header[72], header[73], header[74], header[75]]),
                    ];
                    let target_be: [u32; 8] = std::array::from_fn(|i| {
                        let o = i * 4;
                        u32::from_be_bytes([target[o], target[o + 1], target[o + 2], target[o + 3]])
                    });

                    // Write to GPU buffers (async — must finish before kernel reads)
                    mid_buf.write(&midstate[..]).enq().ok();
                    tail_buf.write(&tail_words[..]).enq().ok();
                    tgt_buf.write(&target_be[..]).enq().ok();
                    queue.finish().ok();

                    let batch_start = Instant::now();
                    let result = run_gpu_search(
                        &queue,
                        &program,
                        &mid_buf,
                        &tail_buf,
                        &tgt_buf,
                        &flag_buf,
                        &nonce_buf,
                        &hash_buf,
                        nonce_base,
                        per_dispatch as u64,
                        workgroup_size,
                    );

                    match result {
                        Ok((found_nonce, checked)) => {
                            hash_count += checked;
                            if let Some(nonce) = found_nonce {
                                let verify_header = job.build_header(&merkle_root, ntime, nonce);
                                let hash = crate::job::double_sha256(&verify_header);
                                if hash_meets_target(&hash, &target) {
                                    eprintln!(
                                        "{} \x1b[33m✔ GPU #{} FOUND SHARE!\x1b[0m nonce={:08x} hash={}",
                                        ts(), device_index, nonce, hex::encode(hash)
                                    );
                                    let _ = share_tx.send(FoundShare {
                                        job_id: job.job_id.clone(),
                                        extranonce2: extranonce2.clone(),
                                        ntime: format!("{:08x}", ntime),
                                        nonce,
                                    });
                                } else {
                                    eprintln!(
                                        "{} [WARN] GPU #{} false positive nonce={:08x}",
                                        ts(), device_index, nonce
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("{} [ERROR] GPU #{}: {}", ts(), device_index, e);
                        }
                    }

                    nonce_base = nonce_base.wrapping_add(per_dispatch);

                    // ── GPU usage throttle ─────────────────────────────
                    if gpu_usage < 100 {
                        let batch_us = batch_start.elapsed().as_micros() as u64;
                        let sleep_us = batch_us * (100 - gpu_usage as u64) / gpu_usage as u64;
                        if sleep_us > 0 {
                            thread::sleep(std::time::Duration::from_micros(sleep_us));
                        }
                    }

                    let elapsed = last_report.elapsed().as_secs_f64();
                    if elapsed >= 10.0 {
                        let hr = hash_count as f64 / elapsed;
                        *hashrate.lock().unwrap() = hr;
                        eprintln!(
                            "{} GPU #{} ({}) {:.2} MH/s",
                            ts(), device_index, device_name, hr / 1_000_000.0
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

// ─── OpenCL search (single queue — no cross-queue sync issues) ─────────

fn run_gpu_search(
    queue: &Queue,
    program: &Program,
    mid_buf: &Buffer<u32>,
    tail_buf: &Buffer<u32>,
    tgt_buf: &Buffer<u32>,
    flag_buf: &[Buffer<u32>; 2],
    nonce_buf: &[Buffer<u32>; 2],
    hash_buf: &[Buffer<u32>; 2],
    start_nonce: u32,
    total_count: u64,
    workgroup_size: usize,
) -> Result<(Option<u32>, u64), String> {
    // Reset flag buffer to 0 — force completion before kernel launch
    flag_buf[0].write(&[0u32][..]).enq().ok();
    queue.finish().ok();

    let global = ((total_count as usize + workgroup_size - 1) / workgroup_size) * workgroup_size;

    // Launch kernel on the SAME queue — guarantees flag reset completes first
    let kernel = Kernel::builder()
        .program(program)
        .name("sha256d_search")
        .queue(queue.clone())
        .global_work_size(global)
        .arg(mid_buf)
        .arg(tail_buf)
        .arg(tgt_buf)
        .arg(&start_nonce)
        .arg(&flag_buf[0])
        .arg(&nonce_buf[0])
        .arg(&hash_buf[0])
        .build()
        .map_err(|e| format!("kernel build: {}", e))?;

    unsafe {
        kernel.enq().map_err(|e| format!("kernel enq: {}", e))?;
    }

    // Wait for kernel to complete
    queue.finish().map_err(|e| format!("queue finish: {}", e))?;

    // Read result on the same queue
    let mut flag = vec![0u32; 1];
    flag_buf[0].read(&mut flag).enq().ok();
    queue.finish().ok();

    if flag[0] != 0 {
        let mut nonce = vec![0u32; 1];
        nonce_buf[0].read(&mut nonce).enq().ok();
        queue.finish().ok();
        Ok((Some(nonce[0]), total_count))
    } else {
        Ok((None, total_count))
    }
}

// ─── CPU midstate ──────────────────────────────────────────────────────

fn compute_midstate(header: &[u8; 80]) -> [u32; 8] {
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
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
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
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(k[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g; g = f; f = e; e = d.wrapping_add(t1); d = c; c = b; b = a; a = t1.wrapping_add(t2);
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
