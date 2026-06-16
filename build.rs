/// Build script: compile CUDA kernel to PTX using nvcc, and configure linker.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Only run CUDA build steps when cuda-gpu feature is enabled
    let cuda_feature = env::var("CARGO_FEATURE_CUDA_GPU").is_ok();
    if !cuda_feature {
        return;
    }

    // ── Tell rustc to link against libcuda.so ──
    // libcuda.so.1 is provided by the NVIDIA driver.
    println!("cargo:rustc-link-lib=dylib=cuda");
    // Common paths for libcuda.so
    println!("cargo:rustc-link-search=native=/lib/x86_64-linux-gnu");
    println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu");
    println!("cargo:rustc-link-search=native=/usr/lib64");

    // ── Compile .cu → PTX ──
    let kernel_path = PathBuf::from("src/gpu/sha256d_kernel.cu");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ptx_path = out_dir.join("sha256d_kernel.ptx");

    println!("cargo:rerun-if-changed={}", kernel_path.display());
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
    println!("cargo:rerun-if-env-changed=PATH");

    let nvcc = env::var("NVCC_PATH").unwrap_or_else(|_| "nvcc".to_string());

    // RTX 4090 = compute_89 / sm_89
    let status = Command::new(&nvcc)
        .args([
            "-ptx",
            "-o",
            ptx_path.to_str().unwrap(),
            kernel_path.to_str().unwrap(),
            "-arch=compute_89",
            "-code=sm_89,compute_89",
            "--use_fast_math",
            "-O2",
            "-lineinfo",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:warning=CUDA kernel compiled to PTX (sm_89)");
        }
        Ok(s) => {
            panic!(
                "nvcc failed with exit code {}. Install CUDA toolkit or set NVCC_PATH.",
                s.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            panic!("Failed to run nvcc ({}). Is CUDA toolkit installed?", e);
        }
    }
}
