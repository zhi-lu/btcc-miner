mod gpu;
mod job;
mod stratum;

use std::io;
use stratum::StratumClient;

fn main() {
    println!("BTCC Rust Stratum Miner v0.2.0 (GPU)");
    println!(
        "Platform: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    // Try to initialize GPU miners (all available GPUs)
    let gpu_miners = gpu::GpuMiner::new();
    let use_gpu = !gpu_miners.is_empty();

    let server = "pool.btc-classic.org:63101";
    let username = "cc1q6qmx0kgdf94xe8046ee9tnvn6l20hk8nm8naw8.worker1";
    let password = "x";

    println!("Server: stratum+tcp://{}", server);
    println!("Username: {}", username);
    if use_gpu {
        println!(
            "Mining mode: GPU ({} device{})",
            gpu_miners.len(),
            if gpu_miners.len() > 1 { "s" } else { "" }
        );
    } else {
        println!("Mining mode: CPU");
    }

    let client = StratumClient::new(server, username, password, gpu_miners);
    client.run();

    println!("Miner started. Press Enter to stop...");
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();

    println!("Shutting down...");
    client.stop();
    println!("Miner stopped.");
}
