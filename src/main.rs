mod config;
mod gpu;
mod job;
mod stratum;

use config::AppConfig;
use std::io;
use std::path::PathBuf;
use stratum::StratumClient;

const VERSION: &str = "0.3.0";
const DEFAULT_CONFIG: &str = ".config/config.toml";

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Help => {
            print_help();
        }
        Command::Version => {
            println!("btcc_miner v{}", VERSION);
            println!(
                "Platform: {} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            );
        }
        Command::Run => {
            run_miner(&cli.config_path);
        }
    }
}

// ─── CLI ───────────────────────────────────────────────────────────────

enum Command {
    Run,
    Help,
    Version,
}

struct Cli {
    config_path: PathBuf,
    command: Command,
}

impl Cli {
    fn parse() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mut config_path: Option<PathBuf> = None;
        let mut command: Option<Command> = None;

        let mut i = 1; // skip argv[0]
        while i < args.len() {
            match args[i].as_str() {
                "--config" | "-c" => {
                    i += 1;
                    if i < args.len() {
                        config_path = Some(PathBuf::from(&args[i]));
                    } else {
                        eprintln!("error: '--config <path>' requires a value but none was supplied");
                        eprintln!("Usage: btcc_miner [--config <path>] <command>");
                        std::process::exit(1);
                    }
                }
                "--help" | "-h" => {
                    command = Some(Command::Help);
                }
                "run" => {
                    command = Some(Command::Run);
                }
                "version" | "--version" | "-V" => {
                    command = Some(Command::Version);
                }
                "help" => {
                    command = Some(Command::Help);
                }
                other => {
                    eprintln!("error: unrecognized command '{}'", other);
                    eprintln!("       run 'btcc_miner help' for usage");
                    std::process::exit(1);
                }
            }
            i += 1;
        }

        Cli {
            config_path: config_path.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG)),
            command: command.unwrap_or(Command::Help),
        }
    }
}

// ─── Help ──────────────────────────────────────────────────────────────

fn print_help() {
    println!("btcc_miner v{}", VERSION);
    println!("BTCC Stratum Miner — GPU (OpenCL/CUDA/Metal) + CPU mining");
    println!();
    println!("Usage:");
    println!("  btcc_miner [flags] <command>");
    println!();
    println!("Available Commands:");
    println!("  run         Start the miner");
    println!("  version     Print version information");
    println!("  help        Show this help message");
    println!();
    println!("Flags:");
    println!(
        "  -c, --config <path>   Config file path (default: {})",
        DEFAULT_CONFIG
    );
    println!("  -h, --help            Show this help message");
    println!();
    println!("Examples:");
    println!("  btcc_miner run");
    println!("  btcc_miner --config /etc/miner.toml run");
    println!("  btcc_miner run -c ./my.toml");
    println!("  btcc_miner version");
}

// ─── Miner ─────────────────────────────────────────────────────────────

fn run_miner(config_path: &PathBuf) {
    println!("BTCC Rust Stratum Miner v{}", VERSION);
    println!(
        "Platform: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let cfg = AppConfig::load_from(config_path);

    println!("Server: stratum+tcp://{}", cfg.miner.server);
    println!("Username: {}", cfg.miner.username);
    println!("Mode: {}", cfg.machine.mode);

    // GPU or CPU
    let gpu_miners = if cfg.machine.mode == "gpu" {
        let devices = gpu::GpuMiner::new(&cfg.machine.gpu_devices, cfg.machine.gpu_usage);
        if devices.is_empty() {
            let cores = if cfg.machine.cpu_cores > 0 {
                cfg.machine.cpu_cores
            } else {
                num_cpus::get()
            };
            eprintln!(
                "[WARN] No GPUs found. Falling back to CPU mode with {} cores.",
                cores
            );
        }
        devices
    } else {
        vec![]
    };

    if !gpu_miners.is_empty() {
        println!(
            "Mining mode: GPU ({} device{}, usage={}%)",
            gpu_miners.len(),
            if gpu_miners.len() > 1 { "s" } else { "" },
            cfg.machine.gpu_usage
        );
    } else {
        let cores = if cfg.machine.cpu_cores > 0 {
            cfg.machine.cpu_cores
        } else {
            num_cpus::get()
        };
        println!("Mining mode: CPU ({} cores)", cores);
    }

    let cpu_cores = if cfg.machine.cpu_cores > 0 {
        cfg.machine.cpu_cores
    } else {
        num_cpus::get()
    };

    let client = StratumClient::new(
        &cfg.miner.server,
        &cfg.miner.username,
        &cfg.miner.password,
        gpu_miners,
        cpu_cores,
    );
    client.run();

    println!("Miner started. Press Enter to stop...");
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();

    println!("Shutting down...");
    client.stop();
    println!("Miner stopped.");
}
