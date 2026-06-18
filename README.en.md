# BTCC Miner

[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

[中文文档](README.md) | **English**

A Rust-based BTCC (Bitcoin-Classic) Stratum mining client with **OpenCL / CUDA / Metal** multi-GPU backend acceleration and **CPU multi-threaded** fallback.

## Features

- **Multi-GPU Backends** — OpenCL (default, cross-platform, all vendors), CUDA (NVIDIA, best performance), Metal (macOS Apple Silicon/AMD)
- **Multi-GPU Parallelism** — Auto-detects all GPUs, spawns one mining thread per device
- **Configuration File** — Edit `.config/config.toml` for pool, wallet, GPU selection, and resource usage — no code changes needed
- **CLI Interface** — Subcommand-style CLI: `run` / `version` / `help`
- **Stratum v1 Protocol** — Full `mining.subscribe` / `mining.authorize` / `mining.notify` / `mining.submit` implementation
- **Auto-Reconnect** — Automatically reconnects on disconnect; GPU threads are reused, not re-created
- **Real-time Hashrate** — Per-GPU hashrate reported every 10 seconds with timestamps
- **GPU Optimizations** — Midstate precompute + double-buffered pipeline + auto-tuning (SM/CU count)
- **CPU Fallback** — Automatically falls back to CPU multi-threading when no GPU is available; configurable core count

## System Requirements

| Platform | GPU Backend | Supported Vendors |
|----------|-------------|-------------------|
| Linux | OpenCL (default) | NVIDIA / AMD / Intel |
| Linux | CUDA (optional) | NVIDIA (requires CUDA Toolkit) |
| Windows | OpenCL (default) | NVIDIA / AMD / Intel |
| Windows | CUDA (optional) | NVIDIA (requires CUDA Toolkit) |
| macOS (Apple Silicon) | Metal | Apple M1/M2/M3/M4 |
| macOS (Intel + AMD GPU) | Metal / OpenCL | AMD |

- Rust 1.70+
- GPU drivers: NVIDIA (525+), AMD (ROCm/AMDGPU-Pro), Apple (built-in Metal)
- CUDA backend additionally requires: CUDA Toolkit 12+ (`nvcc`)

## Quick Start

### 1. Clone

```bash
git clone https://github.com/zhi-lu/btcc_miner.git
cd btcc_miner
```

### 2. Configure

Edit `.config/config.toml` with your wallet address:

```toml
[miner]
server = "pool.btc-classic.org:63101"
username = "your_btcc_address.worker1"
password = "x"

[machine]
mode = "gpu"          # "gpu" or "cpu"
gpu_devices = []      # [] = all GPUs, [0] = first GPU only
gpu_usage = 100       # GPU usage 1–100%
cpu_cores = 0         # CPU thread count, 0 = auto
```

### 3. Build & Run

| Platform | Command |
|----------|---------|
| **Linux / Windows (general)** | `cargo build --release && ./target/release/btcc_miner run` |
| **Linux / Windows (NVIDIA max perf)** | `cargo build --release --features cuda-gpu && ./target/release/btcc_miner run` |
| **macOS (Apple Silicon / AMD)** | `cargo build --release --features metal-gpu && ./target/release/btcc_miner run` |

> Use `-c` to specify a custom config path: `./btcc_miner -c /path/to/my.toml run`

### 4. Stop Mining

Press `Enter` to gracefully shut down.

## Command Line

```bash
btcc_miner [flags] <command>

Available Commands:
  run         Start the miner
  version     Print version information
  help        Show this help message

Flags:
  -c, --config <path>   Config file path (default: .config/config.toml)
  -h, --help            Show this help message

Examples:
  btcc_miner run
  btcc_miner --config /etc/miner.toml run
  btcc_miner run -c ./my.toml
  btcc_miner version
```

## Build Options

### Feature Flags

| Feature | Backend | Platform | GPU Vendors |
|---------|---------|----------|-------------|
| `opencl-gpu` **(default)** | OpenCL | Linux / Windows / macOS | NVIDIA / AMD / Intel |
| `cuda-gpu` | CUDA | Linux / Windows | NVIDIA |
| `metal-gpu` | Metal | macOS | Apple Silicon / AMD |

```bash
# OpenCL (default, single command)
cargo build --release

# CUDA (NVIDIA best performance, requires CUDA Toolkit)
cargo build --release --features cuda-gpu

# Metal (macOS, requires Xcode)
cargo build --release --features metal-gpu
```

### CUDA Build Requirements

- Install [CUDA Toolkit](https://developer.nvidia.com/cuda-downloads) 12+
- Ensure `nvcc` is in PATH, or set `CUDA_HOME` / `CUDA_PATH` environment variable
- Minimum supported compute capability: 5.2 (Maxwell, 2014+). Older GPUs should use OpenCL.

## Configuration File

Full configuration reference:

```toml
[miner]
server   = "pool.btc-classic.org:63101"   # Pool address
username = "cc1...wallet.worker1"          # Wallet address . worker name
password = "x"                             # Pool password

[machine]
mode        = "gpu"       # Mining mode: "gpu" or "cpu"
cpu_cores   = 0           # CPU thread count, 0 = auto
gpu_devices = []          # GPU device list: [] = all, [0] = first only, [0,1] = first two
gpu_usage   = 100         # GPU usage 1–100%, <100 sleeps between batches to reduce power
```

## Project Structure

```
btcc_miner/
├── Cargo.toml                # Project config & dependencies
├── build.rs                  # Build script (CUDA PTX compilation + cross-platform linking)
├── config.toml               # Runtime configuration
├── src/
│   ├── main.rs               # CLI entry: arg parsing, config loading, mining orchestration
│   ├── config.rs             # Config file parsing (`[miner]` + `[machine]`)
│   ├── job.rs                # Mining jobs: coinbase, Merkle root, block header, SHA-256d
│   ├── stratum.rs            # Stratum protocol: TCP, JSON-RPC, GPU/CPU scheduling
│   └── gpu/
│       ├── mod.rs            # GPU module routing (conditional compilation)
│       ├── opencl_impl.rs    # OpenCL GPU backend (cross-platform default)
│       ├── cuda_impl.rs      # CUDA GPU backend (NVIDIA high performance)
│       ├── metal_impl.rs     # Metal GPU backend (macOS)
│       ├── sha256d_kernel.cu # CUDA kernel source
│       └── stub.rs           # CPU fallback stub
├── docs/
│   ├── stratum_protocol.md   # Stratum protocol details
│   └── mining_algorithm.md   # SHA-256d mining algorithm deep dive
└── update/                   # Changelog & requirements docs
```

## Performance

| Hardware | Backend | Hashrate |
|----------|---------|----------|
| NVIDIA RTX 4090 | CUDA | ~2,600 MH/s |
| NVIDIA RTX 4090 | OpenCL | ~1,300 MH/s |
| Apple M2 (8 GPU cores) | Metal | ~188 MH/s |
| Apple M2 Pro | Metal | ~350–400 MH/s |
| Apple M2 Max | Metal | ~650–700 MH/s |
| Apple M2 (8 CPU cores) | CPU | ~5–8 MH/s |

> NVIDIA users should prefer CUDA (~2× OpenCL performance). AMD GPU users use OpenCL.

## Background Running

### nohup

```bash
nohup ./target/release/btcc_miner run > miner.log 2>&1 &
echo $! > miner.pid
tail -f miner.log
kill "$(cat miner.pid)"
```

### tmux (recommended)

```bash
tmux new -s miner
./target/release/btcc_miner run
# Ctrl+B, D to detach
# tmux attach -t miner  to reattach
```

## FAQ

### Frequent Disconnects

Stratum pools may disconnect due to idle timeout or load balancing. This is normal. The miner auto-reconnects.

### OpenCL Not Found

Ensure GPU drivers are installed:
- NVIDIA: check with `nvidia-smi`
- AMD: check with `rocm-smi` or `clinfo`

### CUDA Build Fails

```bash
# Check if nvcc is available
nvcc --version
# Specify CUDA path
CUDA_HOME=/usr/local/cuda-12.8 cargo build --release --features cuda-gpu
```

### Authorization Failed

Ensure `username` is formatted as `wallet_address.worker_name`. BTCC addresses start with `cc1`.

## Changelog

### v0.3.0

- OpenCL cross-platform GPU backend (Linux/Windows/macOS, NVIDIA/AMD/Intel)
- CUDA GPU backend (NVIDIA best performance, optional build)
- `.config/config.toml` configuration file support
- Multi-GPU parallel mining
- CLI subcommand interface (`run` / `version` / `help`)
- Configurable GPU resource usage (`gpu_usage`)

### v0.2.1

- `[HH:MM:SS]` timestamps on all log lines
- Fixed GPU thread duplication on reconnect causing hashrate degradation
- Adapted to `metal-rs` 0.29 API

### v0.2.0

- Metal GPU mining support (midstate optimization + double-buffered pipeline)
- CPU multi-threaded fallback
- Full Stratum v1 protocol implementation

## Disclaimer

**This software is for educational purposes only. Commercial or profit-making use is strictly prohibited.**

- **Legal Compliance**: You must comply with the laws and regulations of your jurisdiction. Some jurisdictions prohibit or restrict cryptocurrency mining. You are solely responsible for any consequences of violating applicable laws.
- **Electricity Cost**: GPU mining consumes significant power. Evaluate costs yourself.
- **Hardware Wear**: Prolonged full-load operation may accelerate hardware aging or cause thermal throttling.
- **Profit Risk**: Mining revenue is affected by coin price, difficulty, pool policies, and other factors. Profitability is not guaranteed.

**By using this software, you acknowledge and agree to assume all risks. The author is not liable for any direct or indirect damages or legal consequences.**

## Acknowledgements

- This project was forked and improved from [Even521's BTCC Rust Stratum Miner](https://github.com/Even521/btcc_rust_stratum_miner). Many thanks for his open-source contribution and inspiration.

## License

MIT
