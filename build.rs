/// Build script — cross-platform CUDA PTX compilation + linker config.
///
/// OpenCL linking is handled automatically by the `ocl` crate — no manual paths needed.
/// CUDA needs: nvcc for PTX, libcuda.so / nvcuda.dll for linking.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let cuda_feature = env::var("CARGO_FEATURE_CUDA_GPU").is_ok();
    let opencl_feature = env::var("CARGO_FEATURE_OPENCL_GPU").is_ok();

    // ── OpenCL: add common library search paths ────────────────────
    // The `ocl` crate's cl-sys tries pkg-config first, which may fail.
    // Add common fallback paths so the linker can find libOpenCL.so / OpenCL.lib.
    if opencl_feature {
        cfg_opencl_paths();
    }

    // ── CUDA: link + PTX compilation ────────────────────────────────
    if cuda_feature {
        cfg_cuda_link();
        compile_cuda_ptx();
    }
}

/// Add common OpenCL library search paths for the linker.
/// The `ocl` crate's cl-sys tries pkg-config, which may not have OpenCL registered.
fn cfg_opencl_paths() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    match target_os.as_str() {
        "linux" => {
            // Standard Linux library paths
            println!("cargo:rustc-link-search=native=/usr/lib64");
            println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu");
            println!("cargo:rustc-link-search=native=/usr/lib/aarch64-linux-gnu");
            // CUDA toolkit also ships libOpenCL.so
            for ver in &["13.0", "12.8", "12.6", "12.4", "12.2", "12.0", "11.8"] {
                let p = format!("/usr/local/cuda-{}/targets/x86_64-linux/lib", ver);
                if Path::new(&p).exists() {
                    println!("cargo:rustc-link-search=native={}", p);
                }
            }
            // AMD ROCm also ships libOpenCL.so
            for rocm in &["/opt/rocm/lib", "/opt/rocm/opencl/lib"] {
                if Path::new(rocm).exists() {
                    println!("cargo:rustc-link-search=native={}", rocm);
                }
            }
        }
        "windows" => {
            // OpenCL.dll / OpenCL.lib are typically in the GPU driver,
            // which is on the system PATH. No extra paths needed.
        }
        "macos" => {
            // OpenCL is a system framework on macOS
        }
        _ => {}
    }
}

/// Configure rustc to link against the CUDA driver library.
fn cfg_cuda_link() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    match target_os.as_str() {
        "linux" => {
            println!("cargo:rustc-link-lib=dylib=cuda");
            // Common Linux lib paths for libcuda.so
            println!("cargo:rustc-link-search=native=/usr/lib64");
            println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu");
            println!("cargo:rustc-link-search=native=/usr/lib/aarch64-linux-gnu");
        }
        "windows" => {
            println!("cargo:rustc-link-lib=dylib=nvcuda");
            // nvcuda.dll is in System32 (always on DLL search path)
        }
        "macos" => {
            println!("cargo:warning=CUDA on macOS is not supported (no NVIDIA drivers since High Sierra). Use Metal or OpenCL instead.");
        }
        _ => {}
    }

    // Also add CUDA toolkit lib dir if CUDA_HOME/CUDA_PATH is set
    if let Ok(cuda_home) = env::var("CUDA_HOME").or_else(|_| env::var("CUDA_PATH")) {
        let lib_dir = Path::new(&cuda_home).join("lib64");
        if lib_dir.exists() {
            println!("cargo:rustc-link-search=native={}", lib_dir.display());
        }
    }
}

/// Find nvcc and compile the SHA-256d kernel to PTX.
/// The resulting PTX is embedded via `include_bytes!` in cuda_impl.rs.
fn compile_cuda_ptx() {
    let kernel_path = PathBuf::from("src/gpu/sha256d_kernel.cu");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ptx_path = out_dir.join("sha256d_kernel.ptx");

    println!("cargo:rerun-if-changed={}", kernel_path.display());
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    println!("cargo:rerun-if-env-changed=NVCC_PATH");
    println!("cargo:rerun-if-env-changed=PATH");

    let nvcc = find_nvcc();

    // Minimum compute capability: 5.2 (Maxwell, 2014+)
    // Covers: GTX 9xx, 10xx, 16xx, RTX 20xx, 30xx, 40xx, 50xx
    // Older GPUs (Kepler 3.x, Fermi 2.x) should use OpenCL instead.
    // PTX is forward-compatible — newer drivers JIT it to the native ISA.
    let arch = "compute_52";
    let code = "compute_52";

    let status = Command::new(&nvcc)
        .args([
            "-ptx",
            "-o",
            ptx_path.to_str().unwrap(),
            kernel_path.to_str().unwrap(),
            &format!("-arch={}", arch),
            &format!("-code={}", code),
            "--use_fast_math",
            "-O2",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!(
                "cargo:warning=CUDA PTX compiled (arch={arch}, min cc 5.2)"
            );
        }
        Ok(s) => {
            panic!(
                "nvcc failed (exit {}). Install CUDA toolkit or set NVCC_PATH.\n\
                 Or use OpenCL: --no-default-features --features opencl-gpu",
                s.code().unwrap_or(-1)
            );
        }
        Err(_) => {
            panic!(
                "nvcc not found. Install CUDA toolkit or set NVCC_PATH.\n\
                 Or use OpenCL: --no-default-features --features opencl-gpu"
            );
        }
    }
}

/// Locate nvcc: NVCC_PATH env > CUDA_HOME/bin > CUDA_PATH/bin > common paths > PATH.
fn find_nvcc() -> PathBuf {
    // 1. NVCC_PATH environment variable
    if let Ok(path) = env::var("NVCC_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return p;
        }
    }

    // 2. CUDA_HOME / CUDA_PATH + /bin/nvcc
    for var in &["CUDA_HOME", "CUDA_PATH"] {
        if let Ok(dir) = env::var(var) {
            let nvcc = Path::new(&dir).join("bin").join(nvcc_name());
            if nvcc.exists() {
                return nvcc;
            }
        }
    }

    // 3. Common install locations (per platform)
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let candidates: &[&str] = match target_os.as_str() {
        "linux" => &[
            "/usr/local/cuda/bin/nvcc",
            "/opt/cuda/bin/nvcc",
        ],
        "windows" => &[
            r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.8\bin\nvcc.exe",
            r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.6\bin\nvcc.exe",
            r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.4\bin\nvcc.exe",
            r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v11.8\bin\nvcc.exe",
        ],
        _ => &[],
    };

    for candidate in candidates {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return p;
        }
    }

    // 4. Fallback: hope it's on PATH
    PathBuf::from(nvcc_name())
}

fn nvcc_name() -> &'static str {
    if env::var("CARGO_CFG_TARGET_OS").map_or(false, |os| os == "windows") {
        "nvcc.exe"
    } else {
        "nvcc"
    }
}
