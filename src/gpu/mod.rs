// GPU mining module.
//
// On macOS with `--features metal-gpu`, uses Metal compute shaders (Apple Silicon).
// On Linux/Windows with `--features cuda-gpu`, uses CUDA (NVIDIA GPUs).
// Otherwise, falls back to CPU mining via the stub.

// ── Metal backend (macOS only) ──

#[cfg(all(target_os = "macos", feature = "metal-gpu"))]
mod metal_impl;

#[cfg(all(target_os = "macos", feature = "metal-gpu"))]
pub use metal_impl::GpuMiner;

// ── CUDA backend (non-macOS) ──

#[cfg(all(not(target_os = "macos"), feature = "cuda-gpu"))]
mod cuda_impl;

#[cfg(all(not(target_os = "macos"), feature = "cuda-gpu"))]
pub use cuda_impl::GpuMiner;

// ── Stub fallback ──

#[cfg(not(any(
    all(target_os = "macos", feature = "metal-gpu"),
    all(not(target_os = "macos"), feature = "cuda-gpu")
)))]
mod stub;

#[cfg(not(any(
    all(target_os = "macos", feature = "metal-gpu"),
    all(not(target_os = "macos"), feature = "cuda-gpu")
)))]
pub use stub::GpuMiner;

