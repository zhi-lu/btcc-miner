use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::gpu::GpuMiner;
use crate::job::{hash_meets_target, MiningJob};

/// Format a timestamp for log output: [HH:MM:SS]
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

/// Represents a stratum mining subscription.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub extranonce1: String,
}

/// A share found by a mining worker, to be submitted to the pool.
#[derive(Debug, Clone)]
pub struct FoundShare {
    pub job_id: String,
    pub extranonce2: String,
    pub ntime: String,
    pub nonce: u32,
}

/// The stratum client manages the TCP connection and mining loop.
pub struct StratumClient {
    server: String,
    username: String,
    password: String,
    running: Arc<AtomicBool>,
    gpu_miners: Vec<GpuMiner>,
    cpu_cores: usize,
    /// Shared mining job state.
    pub current_job: Arc<Mutex<Option<MiningJob>>>,
    pub subscription: Arc<Mutex<Option<Subscription>>>,
    pub hashrate: Arc<Mutex<f64>>,
    /// Current share difficulty (from mining.set_difficulty).
    pub difficulty: Arc<Mutex<f64>>,
}

impl StratumClient {
    pub fn new(
        server: &str,
        username: &str,
        password: &str,
        gpu_miners: Vec<GpuMiner>,
        cpu_cores: usize,
    ) -> Self {
        StratumClient {
            server: server.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            running: Arc::new(AtomicBool::new(true)),
            gpu_miners,
            cpu_cores,
            current_job: Arc::new(Mutex::new(None)),
            subscription: Arc::new(Mutex::new(None)),
            hashrate: Arc::new(Mutex::new(0.0)),
            difficulty: Arc::new(Mutex::new(1.0)),
        }
    }

    /// Connect to the stratum server and start the mining loop.
    pub fn run(&self) {
        let running = self.running.clone();
        let current_job = self.current_job.clone();
        let subscription = self.subscription.clone();
        let hashrate = self.hashrate.clone();
        let difficulty = self.difficulty.clone();
        let server = self.server.clone();
        let username = self.username.clone();
        let password = self.password.clone();
        let gpu_miners = self.gpu_miners.clone();
        let cpu_cores = self.cpu_cores;

        let gpu_started = Arc::new(AtomicBool::new(false));

        thread::spawn(move || loop {
            if !running.load(Ordering::Relaxed) {
                break;
            }

            eprintln!("{} Connecting to {}...", ts(), server);
            match TcpStream::connect(&server) {
                Ok(stream) => {
                    eprintln!("{} Connected to {}", ts(), server);
                    if let Err(e) = handle_connection(
                        stream,
                        &username,
                        &password,
                        &running,
                        &current_job,
                        &subscription,
                        &hashrate,
                        &difficulty,
                        &gpu_miners,
                        &gpu_started,
                        cpu_cores,
                    ) {
                        eprintln!("{} [ERROR] Connection error: {}", ts(), e);
                    }
                }
                Err(e) => {
                    eprintln!("{} [ERROR] Failed to connect: {}", ts(), e);
                }
            }

            if running.load(Ordering::Relaxed) {
                eprintln!("{} Reconnecting in 5 seconds...", ts());
                thread::sleep(Duration::from_secs(5));
            }
        });
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

fn handle_connection(
    mut stream: TcpStream,
    username: &str,
    password: &str,
    running: &Arc<AtomicBool>,
    current_job: &Arc<Mutex<Option<MiningJob>>>,
    subscription: &Arc<Mutex<Option<Subscription>>>,
    hashrate: &Arc<Mutex<f64>>,
    difficulty: &Arc<Mutex<f64>>,
    gpu_miners: &Vec<GpuMiner>,
    gpu_started: &Arc<AtomicBool>,
    cpu_cores: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    // Subscribe to mining
    let sub_msg = serde_json::json!({
        "id": 1,
        "method": "mining.subscribe",
        "params": ["btcc-rust-miner/0.2.0"]
    });
    send_json(&mut stream, &sub_msg)?;

    // Authorize
    let auth_msg = serde_json::json!({
        "id": 2,
        "method": "mining.authorize",
        "params": [username, password]
    });
    send_json(&mut stream, &auth_msg)?;

    let reader = BufReader::new(stream.try_clone()?);

    // Channel for workers to send found shares
    let (share_tx, share_rx) = mpsc::channel::<FoundShare>();

    let job_for_workers = current_job.clone();
    let running_for_workers = running.clone();
    let hashrate_for_workers = hashrate.clone();
    let sub_for_workers = subscription.clone();
    let diff_for_workers = difficulty.clone();

    // Start all GPU miners, or fall back to CPU
    if !gpu_miners.is_empty() {
        if !gpu_started.swap(true, Ordering::Relaxed) {
            eprintln!("{} Starting {} GPU miner(s)...", ts(), gpu_miners.len());
            for gpu in gpu_miners {
                gpu.run(
                    job_for_workers.clone(),
                    running_for_workers.clone(),
                    hashrate_for_workers.clone(),
                    share_tx.clone(),
                    sub_for_workers.clone(),
                    diff_for_workers.clone(),
                );
            }
        } else {
            eprintln!("{} GPU miners already running, reusing threads", ts());
        }
    } else {
        // CPU fallback — use configured core count
        let num_threads = if cpu_cores > 0 { cpu_cores } else { num_cpus::get() };
        eprintln!("{} Starting {} CPU mining threads", ts(), num_threads);

        for worker_idx in 0..num_threads {
            let job = job_for_workers.clone();
            let run = running_for_workers.clone();
            let hr = hashrate_for_workers.clone();
            let sub = sub_for_workers.clone();
            let diff = diff_for_workers.clone();
            let tx = share_tx.clone();
            thread::spawn(move || {
                cpu_mining_worker(job, run, hr, sub, diff, worker_idx as u32, tx);
            });
        }
    }

    // Read stratum messages (non-blocking with share submission)
    let mut reader = reader;
    let mut submit_id: u64 = 100;
    let mut line = String::new();

    while running.load(Ordering::Relaxed) {
        // Check for shares to submit
        while let Ok(share) = share_rx.try_recv() {
            submit_id += 1;
            let submit_msg = serde_json::json!({
                "id": submit_id,
                "method": "mining.submit",
                "params": [
                    username,
                    share.job_id,
                    share.extranonce2,
                    share.ntime,
                    format!("{:08x}", share.nonce)
                ]
            });
            eprintln!(
                "{} \x1b[36m→ Submitting share: job={}, nonce={:08x}\x1b[0m",
                ts(),
                share.job_id,
                share.nonce
            );
            if let Err(e) = send_json(&mut stream, &submit_msg) {
                eprintln!("{} [ERROR] Failed to submit share: {}", ts(), e);
            }
        }

        // Read server messages
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                eprintln!("{} [WARN] Server closed connection", ts());
                break;
            }
            Ok(_) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<Value>(&line) {
                    Ok(msg) => {
                        let method = msg["method"].as_str().unwrap_or("");

                        match method {
                            "mining.notify" => {
                                if let Some(params) = msg["params"].as_array() {
                                    if params.len() >= 9 {
                                        let job = MiningJob {
                                            job_id: params[0].as_str().unwrap_or("").to_string(),
                                            prev_hash: params[1].as_str().unwrap_or("").to_string(),
                                            coinb1: params[2].as_str().unwrap_or("").to_string(),
                                            coinb2: params[3].as_str().unwrap_or("").to_string(),
                                            merkle_branch: params[4]
                                                .as_array()
                                                .map(|a| {
                                                    a.iter()
                                                        .map(|v| {
                                                            v.as_str().unwrap_or("").to_string()
                                                        })
                                                        .collect()
                                                })
                                                .unwrap_or_default(),
                                            version: params[5].as_str().unwrap_or("").to_string(),
                                            nbits: params[6].as_str().unwrap_or("").to_string(),
                                            ntime: params[7].as_str().unwrap_or("").to_string(),
                                            clean_jobs: params[8].as_bool(),
                                        };

                                        eprintln!(
                                            "{} New job: id={}, prev_hash={}, nbits={}",
                                            ts(),
                                            job.job_id,
                                            &job.prev_hash[..16],
                                            job.nbits
                                        );

                                        *current_job.lock().unwrap() = Some(job);
                                    }
                                }
                            }
                            "mining.set_difficulty" => {
                                if let Some(params) = msg["params"].as_array() {
                                    if let Some(diff) = params[0].as_f64() {
                                        *difficulty.lock().unwrap() = diff;
                                        eprintln!(
                                            "{} \x1b[33mDifficulty set to: {}\x1b[0m",
                                            ts(),
                                            diff
                                        );
                                    }
                                }
                            }
                            _ => {
                                // Handle subscription response
                                if let Some(id) = msg["id"].as_u64() {
                                    if id == 1 {
                                        if let Some(result) = msg["result"].as_array() {
                                            if result.len() >= 3 {
                                                let extranonce1 =
                                                    result[1].as_str().unwrap_or("").to_string();
                                                let extranonce2_size =
                                                    result[2].as_u64().unwrap_or(4) as usize;
                                                eprintln!(
                                                    "{} Subscribed: extranonce1={}, extranonce2_size={}",
                                                    ts(),
                                                    extranonce1,
                                                    extranonce2_size
                                                );
                                                *subscription.lock().unwrap() =
                                                    Some(Subscription {
                                                        extranonce1: extranonce1.clone(),
                                                    });
                                            }
                                        }
                                    } else if id == 2 {
                                        let success = msg["result"].as_bool().unwrap_or(false);
                                        if success {
                                            eprintln!("{} Authorized successfully", ts());
                                        } else {
                                            eprintln!("{} [ERROR] Authorization failed", ts());
                                        }
                                    } else if id >= 100 {
                                        // Share submission response
                                        let accepted = msg["result"].as_bool().unwrap_or(false);
                                        if accepted {
                                            eprintln!(
                                                "{} \x1b[32m✔ Share accepted by pool! YES\x1b[0m",
                                                ts()
                                            );
                                        } else {
                                            let reason = msg["error"]
                                                .as_array()
                                                .and_then(|a| a.get(1))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("unknown reason");
                                            eprintln!(
                                                "{} \x1b[31m[WARN] Share rejected: {}\x1b[0m",
                                                ts(),
                                                reason
                                            );
                                        }
                                    } else {
                                        // Debug: log unexpected id responses
                                        eprintln!("{} [DEBUG] response id={}: {:?}", ts(), id, msg);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // Silently skip unparseable messages (common with stratum)
                        let _ = (e, line);
                    }
                }
            }
            Err(e) => {
                eprintln!("{} [ERROR] Read error: {}", ts(), e);
                break;
            }
        }
    }

    Ok(())
}

fn cpu_mining_worker(
    current_job: Arc<Mutex<Option<MiningJob>>>,
    running: Arc<AtomicBool>,
    hashrate: Arc<Mutex<f64>>,
    subscription: Arc<Mutex<Option<Subscription>>>,
    difficulty: Arc<Mutex<f64>>,
    worker_idx: u32,
    share_tx: mpsc::Sender<FoundShare>,
) {
    use crate::job::{diff_to_target, double_sha256};
    use std::time::Instant;

    let mut nonce: u32 = 0;
    let mut hash_count: u64 = 0;
    let mut last_report = Instant::now();

    // Each worker gets a unique extranonce2 prefix
    let worker_prefix = worker_idx << 16;

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
            let diff = *difficulty.lock().unwrap();
            let target = diff_to_target(diff);
            let sub = subscription.lock().unwrap().clone();
            let extranonce1 = sub
                .as_ref()
                .map(|s| s.extranonce1.clone())
                .unwrap_or_default();

            // Build extranonce2
            let extranonce2 = format!("{:08x}", worker_prefix.wrapping_add(nonce));

            // Build coinbase and merkle root
            let coinbase = job.build_coinbase(&extranonce1, &extranonce2);
            let merkle_root = job.compute_merkle_root(&coinbase);

            // Parse ntime
            let ntime = u32::from_str_radix(&job.ntime, 16).unwrap_or(0);

            // Try nonces
            for _ in 0..100_000 {
                let header = job.build_header(&merkle_root, ntime, nonce);
                let hash = double_sha256(&header);

                hash_count += 1;

                if hash_meets_target(&hash, &target) {
                    eprintln!(
                        "\x1b[33m✔ CPU FOUND SHARE!\x1b[0m worker={}, nonce={:08x}, hash={}",
                        worker_idx,
                        nonce,
                        hex::encode(hash)
                    );
                    let share = FoundShare {
                        job_id: job.job_id.clone(),
                        extranonce2: extranonce2.clone(),
                        ntime: job.ntime.clone(),
                        nonce,
                    };
                    let _ = share_tx.send(share);
                }

                nonce = nonce.wrapping_add(1);
            }

            // Update hashrate periodically
            let elapsed = last_report.elapsed().as_secs_f64();
            if elapsed >= 10.0 {
                let hr = hash_count as f64 / elapsed;
                *hashrate.lock().unwrap() = hr;
                eprintln!("CPU Hashrate: {:.2} H/s", hr);
                hash_count = 0;
                last_report = Instant::now();
            }
        } else {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn send_json(stream: &mut TcpStream, msg: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut data = serde_json::to_string(msg)?;
    data.push('\n');
    stream.write_all(data.as_bytes())?;
    stream.flush()?;
    Ok(())
}
