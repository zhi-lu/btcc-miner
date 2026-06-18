# BTCC Miner

[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**中文** | [English](README.en.md)

基于 Rust 实现的 BTCC (Bitcoin-Classic) Stratum 矿池挖矿客户端，支持 **OpenCL / CUDA / Metal** 多 GPU 后端加速，以及 **CPU 多线程**回退。

## 特性

- **多 GPU 后端** — OpenCL（默认，跨平台全厂商）、CUDA（NVIDIA 最高性能）、Metal（macOS Apple Silicon/AMD）
- **多 GPU 并行** — 自动检测多张显卡，每个 GPU 一个独立挖矿线程
- **可配置** — 通过 `.config/config.toml` 配置矿池、钱包、GPU 选择、资源利用率，无需改代码
- **命令行界面** — 子命令风格 CLI：`run` / `version` / `help`
- **Stratum v1 协议** — 完整的 `mining.subscribe` / `mining.authorize` / `mining.notify` / `mining.submit` 实现
- **自动重连** — 连接断开后自动重连，GPU 线程复用不重复创建
- **实时算力统计** — 每 10 秒输出带时间戳的各 GPU 算力
- **GPU 性能优化** — Midstate 预计算 + 双缓冲流水线 + 自动调参（SM/CU 数量）
- **CPU 回退** — 无 GPU 时自动使用 CPU 多线程，可配置核心数

## 系统要求

| 平台 | GPU 后端 | 支持厂商 |
|------|---------|---------|
| Linux | OpenCL (默认) | NVIDIA / AMD / Intel |
| Linux | CUDA (可选) | NVIDIA (需 CUDA Toolkit 编译) |
| Windows | OpenCL (默认) | NVIDIA / AMD / Intel |
| Windows | CUDA (可选) | NVIDIA (需 CUDA Toolkit 编译) |
| macOS (Apple Silicon) | Metal | Apple M1/M2/M3/M4 |
| macOS (Intel + AMD GPU) | Metal / OpenCL | AMD |

- Rust 1.70+
- GPU 驱动：NVIDIA (525+)、AMD (ROCm/AMDGPU-Pro)、Apple (内置 Metal)
- CUDA 后端额外需要：CUDA Toolkit 12+ (`nvcc`)

## 快速开始

### 1. 克隆项目

```bash
git clone https://github.com/zhi-lu/btcc_miner.git
cd btcc_miner
```

### 2. 修改配置

编辑 `.config/config.toml`，填入你的钱包地址：

```toml
[miner]
server = "pool.btc-classic.org:63101"
username = "你的BTCC地址.worker1"
password = "x"

[machine]
mode = "gpu"          # "gpu" 或 "cpu"
gpu_devices = []      # [] = 所有GPU, [0] = 仅第一张
gpu_usage = 100       # GPU 使用率 1-100%
cpu_cores = 0         # CPU 线程数, 0=自动
```

### 3. 编译运行

| 平台 | 命令 |
|------|------|
| **Linux / Windows（通用）** | `cargo build --release && ./target/release/btcc_miner run` |
| **Linux / Windows（NVIDIA 高性能）** | `cargo build --release --features cuda-gpu && ./target/release/btcc_miner run` |
| **macOS（Apple Silicon / AMD）** | `cargo build --release --features metal-gpu && ./target/release/btcc_miner run` |

> 也可用 `-c` 指定配置文件：`./btcc_miner -c /path/to/my.toml run`

### 4. 停止挖矿

按 `Enter` 键退出。

## 命令行

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

## 编译选项

### Feature Flags

| Feature | 后端 | 适用平台 | GPU 厂商 |
|---------|------|---------|---------|
| `opencl-gpu` **(默认)** | OpenCL | Linux / Windows / macOS | NVIDIA / AMD / Intel |
| `cuda-gpu` | CUDA | Linux / Windows | NVIDIA |
| `metal-gpu` | Metal | macOS | Apple Silicon / AMD |

```bash
# OpenCL (默认，一行构建)
cargo build --release

# CUDA (NVIDIA 最高性能，需要 CUDA Toolkit)
cargo build --release --features cuda-gpu

# Metal (macOS，需要 Xcode)
cargo build --release --features metal-gpu
```

### CUDA 编译要求

- 安装 [CUDA Toolkit](https://developer.nvidia.com/cuda-downloads) 12+
- 确保 `nvcc` 在 PATH 中，或设置 `CUDA_HOME` / `CUDA_PATH` 环境变量
- 最低支持计算能力 5.2（Maxwell, 2014+），老卡请用 OpenCL

## 配置文件

完整配置项说明：

```toml
[miner]
server   = "pool.btc-classic.org:63101"   # 矿池地址
username = "cc1...wallet.worker1"          # 钱包地址.矿工名
password = "x"                             # 矿池密码

[machine]
mode        = "gpu"       # 挖矿模式: "gpu" 或 "cpu"
cpu_cores   = 0           # CPU 线程数, 0=自动
gpu_devices = []          # GPU 列表: [] = 全部, [0] = 仅第一张, [0,1] = 前两张
gpu_usage   = 100         # GPU 使用率 1-100%, <100 会间歇休眠降功耗
```

## 项目结构

```
btcc_miner/
├── Cargo.toml                # 项目配置与依赖
├── build.rs                  # 编译脚本（CUDA PTX 编译 + 跨平台链接）
├── config.toml               # 运行配置文件
├── src/
│   ├── main.rs               # CLI 入口：参数解析、配置加载、挖矿调度
│   ├── config.rs             # 配置文件解析（`[miner]` + `[machine]`）
│   ├── job.rs                # 挖矿作业：coinbase、Merkle 根、区块头、SHA-256d
│   ├── stratum.rs            # Stratum 协议：TCP 连接、JSON-RPC、GPU/CPU 调度
│   └── gpu/
│       ├── mod.rs            # GPU 模块路由（条件编译）
│       ├── opencl_impl.rs    # OpenCL GPU 实现（跨平台默认）
│       ├── cuda_impl.rs      # CUDA GPU 实现（NVIDIA 高性能）
│       ├── metal_impl.rs     # Metal GPU 实现（macOS）
│       ├── sha256d_kernel.cu # CUDA kernel 源码
│       └── stub.rs           # CPU 回退桩
├── docs/
│   ├── stratum_protocol.md   # Stratum 协议详解
│   └── mining_algorithm.md   # SHA-256d 挖矿核心算法详解
└── update/                   # 需求与更新文档
```

## 性能

| 硬件 | 后端 | 算力 |
|------|------|------|
| NVIDIA RTX 4090 | CUDA | ~2,600 MH/s |
| NVIDIA RTX 4090 | OpenCL | ~1,300 MH/s |
| Apple M2 (8 GPU 核) | Metal | ~188 MH/s |
| Apple M2 Pro | Metal | ~350-400 MH/s |
| Apple M2 Max | Metal | ~650-700 MH/s |
| Apple M2 (8 CPU 核) | CPU | ~5-8 MH/s |

> NVIDIA 用户优先用 CUDA（性能约为 OpenCL 的 2 倍）。AMD GPU 用户用 OpenCL。

## 后台运行

### nohup

```bash
nohup ./target/release/btcc_miner run > miner.log 2>&1 &
echo $! > miner.pid
tail -f miner.log
kill "$(cat miner.pid)"
```

### tmux（推荐）

```bash
tmux new -s miner
./target/release/btcc_miner run
# Ctrl+B, D 分离
# tmux attach -t miner  重新连接
```

## 常见问题

### 连接频繁断开

Stratum 矿池可能因空闲超时或负载均衡主动断开连接，属正常行为。程序会自动重连。

### OpenCL 找不到

确保 GPU 驱动已安装：
- NVIDIA: `nvidia-smi` 检查驱动
- AMD: `rocm-smi` 或 `clinfo` 检查 OpenCL

### CUDA 编译失败

```bash
# 检查 nvcc 是否可用
nvcc --version
# 指定 CUDA 路径
CUDA_HOME=/usr/local/cuda-12.8 cargo build --release --features cuda-gpu
```

### 授权失败

确认 `username` 格式为 `钱包地址.矿工名`，BTCC 地址以 `cc1` 开头。

## 更新日志

### v0.3.0

- OpenCL 跨平台 GPU 后端（Linux/Windows/macOS，NVIDIA/AMD/Intel）
- CUDA GPU 后端（NVIDIA 最佳性能，可选编译）
- `.config/config.toml` 配置文件支持
- 多 GPU 并行挖矿
- 命令行子命令界面（`run` / `version` / `help`）
- GPU 资源利用率可配置（`gpu_usage`）

### v0.2.1

- 所有日志添加 `[HH:MM:SS]` 时间戳
- 修复断连重连时 GPU 线程重复创建导致算力递减的问题
- 适配 `metal-rs` 0.29 API

### v0.2.0

- Metal GPU 挖矿支持（midstate 优化 + 双缓冲流水线）
- CPU 多线程回退
- Stratum v1 协议完整实现

## 免责声明

**本软件仅限学习用途，严禁用于任何商业或盈利目的。**

- **法律合规**：请务必遵守所在国家/地区的法律法规。部分国家/地区禁止或限制加密货币挖矿，任何违反法律法规的行为，后果由使用者自行承担。
- **电费成本**：GPU 挖矿功耗较高，请自行评估。
- **硬件损耗**：长时间满载运行可能加速硬件老化或导致过热降频。
- **收益风险**：挖矿收益受币价、难度、矿池政策等多因素影响，不保证盈利。

**使用本软件即表示您已了解并同意自行承担所有风险，作者不对任何直接或间接损失及法律后果负责。**

## 致谢

- 该项目从这个兄弟 Even521 的 [BTCC Rust Stratum Miner (Even521)](https://github.com/Even521/btcc_rust_stratum_miner) fork 并改进而来，感谢他的开源贡献和启发

## License

MIT
