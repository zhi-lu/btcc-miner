// GPU mining module — cross-platform backends.
//
// Backend priority (only one is compiled):
//   macOS + --features metal-gpu  → Metal (Apple Silicon / AMD)
//   any OS + --features cuda-gpu  → CUDA (NVIDIA, best perf)
//   any OS + --features opencl-gpu → OpenCL (NVIDIA / AMD / Intel, default)
//   otherwise                     → stub (CPU fallback)

// ── Metal (macOS only) ─────────────────────────────────────────────────

#[cfg(all(target_os = "macos", feature = "metal-gpu"))]
mod metal_impl;
#[cfg(all(target_os = "macos", feature = "metal-gpu"))]
pub use metal_impl::GpuMiner;

// ── CUDA (any OS, overrides OpenCL) ────────────────────────────────────

#[cfg(all(
    not(all(target_os = "macos", feature = "metal-gpu")),
    feature = "cuda-gpu"
))]
mod cuda_impl;
#[cfg(all(
    not(all(target_os = "macos", feature = "metal-gpu")),
    feature = "cuda-gpu"
))]
pub use cuda_impl::GpuMiner;

// ── OpenCL (any OS, default) ───────────────────────────────────────────

#[cfg(all(
    not(all(target_os = "macos", feature = "metal-gpu")),
    not(feature = "cuda-gpu"),
    feature = "opencl-gpu"
))]
mod opencl_impl;
#[cfg(all(
    not(all(target_os = "macos", feature = "metal-gpu")),
    not(feature = "cuda-gpu"),
    feature = "opencl-gpu"
))]
pub use opencl_impl::GpuMiner;

// ── Stub (CPU fallback) ────────────────────────────────────────────────

#[cfg(not(any(
    all(target_os = "macos", feature = "metal-gpu"),
    feature = "cuda-gpu",
    feature = "opencl-gpu"
)))]
mod stub;
#[cfg(not(any(
    all(target_os = "macos", feature = "metal-gpu"),
    feature = "cuda-gpu",
    feature = "opencl-gpu"
)))]
pub use stub::GpuMiner;
