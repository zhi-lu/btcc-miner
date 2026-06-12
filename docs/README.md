# BTCC Rust Stratum Miner

[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

基于 Rust 实现的 BTCC (Bitcoin-Classic) Stratum 矿池挖矿客户端，支持 **Apple Silicon GPU (Metal)** 加速和 **多线程 CPU** 回退。

## 特性

- **Metal GPU 挖矿** — 在 Apple M1/M2/M3/M4 系列芯片上使用 GPU 进行 SHA-256d 哈希计算
- **CPU 多线程回退** — 非 macOS 平台或无 GPU 时自动使用 CPU 多线程挖矿
- **Stratum v1 协议** — 完整的 `mining.subscribe` / `mining.authorize` / `mining.notify` / `mining.submit` 实现
- **自动重连** — 连接断开后自动重连，GPU 线程复用不重复创建
- **实时算力统计** — 每 10 秒输出带时间戳的当前算力
- **GPU 性能优化** — Midstate 预计算 + 双缓冲命令流水线 + 自动调参
- **单一二进制** — 纯 Rust 实现，无外部运行时依赖

## 系统要求

| 平台 | GPU 支持 | CPU 支持 |
|------|---------|---------|
| macOS (Apple Silicon) | ✅ Metal GPU | ✅ |
| macOS (Intel) | ❌ | ✅ |
| Linux | ❌ | ✅ |
| Windows | ❌ | ✅ |

- Rust 1.70+
- macOS 12+（GPU 挖矿需要）
- Xcode Command Line Tools（GPU 挖矿需要，`xcode-select --install`）

## 快速开始

### 1. 克隆项目

```bash
git clone <repo-url>
cd btcc_rust_stratum_miner
```

### 2. 修改钱包地址

编辑 `src/main.rs`，将 `username` 改为你的 BTCC 钱包地址：

```rust
let username = "你的BTCC地址.worker1";
```

### 3. 编译运行

| 平台 | 命令 | 挖矿模式 |
|------|------|----------|
| macOS (Apple Silicon) | `cargo run --release --features metal-gpu` | GPU Metal |
| macOS (Intel) | `cargo run --release` | CPU 多线程 |
| Linux | `cargo run --release` | CPU 多线程 |
| Windows | `cargo run --release` | CPU 多线程 |

> **注意**：`--features metal-gpu` 仅在 macOS 上有效。非 macOS 平台即使加上也会被忽略，自动回退 CPU。
>
> 也可以先编译再运行：`cargo build --release --features metal-gpu && ./target/release/btcc_rust_stratum_miner`

### 4. 停止挖矿

按 `Enter` 键优雅退出。

## 命令行参数

当前版本通过修改 `src/main.rs` 中的常量来配置：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `server` | `pool.btc-classic.org:63101` | 矿池地址 |
| `username` | `your_btcc_address.worker1` | BTCC 钱包地址.矿工名 |
| `password` | `x` | 矿池密码（通常填 `x`） |

## 编译选项

### Feature Flags

| Feature | 说明 |
|---------|------|
| `metal-gpu` | 启用 Metal GPU 挖矿（仅 macOS） |

### Release 优化

`Cargo.toml` 中预配置了针对 Apple M2 的编译优化：

```toml
[profile.release]
opt-level = 3      # 最高优化级别
lto = true         # 链接时优化
codegen-units = 1  # 单代码生成单元（更好的内联）

[target.aarch64-apple-darwin]
rustflags = ["-C", "target-cpu=apple-m2"]  # M2 专用指令优化
```

## 项目结构

```
btcc_rust_stratum_miner/
├── Cargo.toml              # 项目配置与依赖
├── src/
│   ├── main.rs             # 入口：初始化、连接矿池、启动挖矿
│   ├── job.rs              # 挖矿作业：coinbase 构建、Merkle 根、区块头、SHA-256d
│   ├── stratum.rs          # Stratum 协议：TCP 连接、JSON-RPC、GPU/CPU 挖矿调度
│   └── gpu/
│       ├── mod.rs          # GPU 模块入口（条件编译）
│       ├── metal_impl.rs   # Metal GPU 实现（SHA-256d kernel + 双缓冲流水线）
│       └── stub.rs         # 非 macOS 平台的 GPU 桩实现
└── docs/
    └── README.md           # 本文件
```

## 工作原理

### Stratum 协议流程

```
┌──────────┐                    ┌──────────┐
│  Miner   │                    │   Pool   │
└────┬─────┘                    └────┬─────┘
     │                               │
     │  mining.subscribe ──────────► │
     │  mining.authorize ──────────► │
     │                               │
     │  ◄────────── mining.notify    │  (新作业)
     │                               │
     │  [GPU/CPU 搜索 nonce]         │
     │                               │
     │  mining.submit ─────────────► │  (提交 share)
     │                               │
     │  ◄────────── result (accept)  │
```

### GPU 挖矿流程

```
┌─────────────────────────────────────────────────────┐
│                    CPU (Rust)                        │
│                                                     │
│  1. 接收 mining.notify → 解析 job                   │
│  2. 构建 80 字节区块头                               │
│  3. 预计算 midstate (chunk1 的 SHA-256 中间状态)     │
│  4. 将 midstate + tail_words + target 写入 GPU buffer│
│  5. 提交 GPU compute dispatch                       │
│  6. 等待 GPU 完成 → 读取结果                         │
│  7. CPU 复核 hash → 提交 share                      │
│                                                     │
└────────────────────┬────────────────────────────────┘
                     │
┌────────────────────▼────────────────────────────────┐
│                  GPU (Metal)                         │
│                                                     │
│  每个线程处理一个 nonce:                              │
│    SHA256_compress(chunk2, midstate) → hash1         │
│    SHA256_compress(hash1, 初始状态)  → 最终 hash      │
│    比较 hash ≤ target → 原子 CAS 写入结果             │
│                                                     │
└─────────────────────────────────────────────────────┘
```

### 断连重连与 GPU 线程复用

每次 TCP 连接断开后，程序自动等待 5 秒重连。**GPU 挖矿线程只在首次连接时启动一次**，后续重连复用已有线程，避免多个 GPU 线程争抢资源导致算力下降。

## 性能

| 硬件 | 模式 | 算力 |
|------|------|------|
| Apple M2 (8 GPU 核) | GPU (Metal) | ~60-90 MH/s |
| Apple M2 Pro | GPU (Metal) | ~350-400 MH/s |
| Apple M2 Max | GPU (Metal) | ~650-700 MH/s |
| Apple M2 (8 CPU 核) | CPU | ~5-8 MH/s |

> 注：M2 基础款实测约 60-90 MH/s（受矿池 vardiff 难度影响），Pro/Max 款需实现 `detect_gpu_cores()` 获取真实核心数才能达到标称算力。

## 日志格式

所有日志带 `[HH:MM:SS]` 时间戳，输出到 stderr：

```
[14:32:05] Connecting to pool.btc-classic.org:63101...
[14:32:06] Connected to pool.btc-classic.org:63101
[14:32:06] GPU: Apple M2 (registry_id=...)
[14:32:06] Metal GPU miner: threadgroup=576, per_dispatch=16M, gpu_cores=0
[14:32:06] Starting GPU miner...
[14:32:06] Subscribed: extranonce1=30005acb, extranonce2_size=8
[14:32:06] Authorized successfully
[14:32:07] New job: id=00000eda, prev_hash=7c7f8948, nbits=1902ee94
[14:32:17] GPU Hashrate: 187976622.83 H/s (187.98 MH/s)
[14:45:30] [WARN] Server closed connection
[14:45:35] Reconnecting in 5 seconds...
[14:45:40] Connected to pool.btc-classic.org:63101
[14:45:40] GPU miner already running, reusing existing thread
```

## 后台运行

### nohup（最简单）

```bash
nohup ./target/release/btcc_rust_stratum_miner > miner.log 2>&1 &
echo $! > miner.pid

# 查看日志
tail -f miner.log

# 停止
kill "$(cat miner.pid)"
```

### caffeinate（防止休眠）

```bash
nohup caffeinate -i ./target/release/btcc_rust_stratum_miner > miner.log 2>&1 &
```

### tmux（SSH 远程推荐）

```bash
tmux new -s miner
caffeinate -i ./target/release/btcc_rust_stratum_miner
# Ctrl+B, D 分离
# tmux attach -t miner  重新连接
```

## 常见问题

### 连接频繁断开

Stratum 矿池可能因空闲超时或负载均衡主动断开连接，属正常行为。程序会自动重连，GPU 线程复用不重复创建。观察日志中 `Server closed connection` 的时间间隔可判断是否有规律。

### 算力逐渐下降

如果每次重连后算力递减，说明 GPU 线程被重复创建。v0.2.1 已修复此问题，重连时输出 `GPU miner already running, reusing existing thread`。

### 连接失败

```bash
# 测试矿池连通性
nc -vz pool.btc-classic.org 63101
```

### GPU 不可用

非 macOS 平台或未启用 `--features metal-gpu` 时，程序会自动回退到 CPU 挖矿。

### 授权失败

确认 `username` 格式为 `钱包地址.矿工名`，BTCC 地址以 `cc1` 开头。

## 更新日志

### v0.2.1

- 所有日志添加 `[HH:MM:SS]` 时间戳
- 修复断连重连时 GPU 线程重复创建导致算力递减的问题
- 适配 `metal-rs` 0.29 API（`ComputePipelineDescriptor`、`CommandBuffer`）

### v0.2.0

- Metal GPU 挖矿支持（midstate 优化 + 双缓冲流水线）
- CPU 多线程回退
- Stratum v1 协议完整实现

## 致谢

- Metal SHA-256d kernel 参考了 [BTCC_apple-gpu-miner](https://github.com/wendell1224/BTCC_apple-gpu-miner)
- Midstate 优化技术源自 cgminer/bfgminer 的成熟方案

## License

MIT