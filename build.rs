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
            // On Windows, the ocl crate's cl-sys requires OpenCL.lib to link.
            // GPU drivers ship OpenCL.dll (in System32) but not the .lib import library.
            // We search common SDK paths first, then auto-generate the .lib from the DLL
            // using Visual Studio tools (dumpbin + lib).
            if let Some(lib_dir) = find_or_generate_opencl_lib_windows() {
                println!("cargo:rustc-link-search=native={}", lib_dir.display());
            }
        }
        "macos" => {
            // OpenCL is a system framework on macOS
        }
        _ => {}
    }
}

/// On Windows: find OpenCL.lib from installed SDKs, or auto-generate it
/// from the system OpenCL.dll using Visual Studio tools.
/// Returns the directory containing OpenCL.lib so the linker can find it.
fn find_or_generate_opencl_lib_windows() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // 1. Check if we already generated it in OUT_DIR
    let cached_lib = out_dir.join("OpenCL.lib");
    if cached_lib.exists() {
        return Some(out_dir);
    }

    // 2. Search common SDK paths (env vars)
    let sdk_envs = [
        ("INTELOCLSDKROOT", "x64"),
        ("CUDA_PATH", "x64"),
        ("AMDAPPSDKROOT", "x86_64"),
    ];
    for (env_var, subdir) in &sdk_envs {
        if let Ok(root) = env::var(env_var) {
            let lib_path = Path::new(&root).join("lib").join(subdir).join("OpenCL.lib");
            if lib_path.exists() {
                println!("cargo:warning=Found OpenCL.lib via {env_var}: {lib}", lib = lib_path.display());
                return Some(lib_path.parent().unwrap().to_path_buf());
            }
        }
    }

    // 3. Search common hard-coded paths
    let common_paths = [
        r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA",
        r"C:\Program Files (x86)\Intel\OpenCL SDK\lib\x64",
        r"C:\Program Files (x86)\AMD APP SDK",
        r"C:\Program Files (x86)\OCL_SDK_Light\lib\x86_64",
    ];
    // CUDA versions
    if Path::new(common_paths[0]).exists() {
        if let Ok(entries) = std::fs::read_dir(common_paths[0]) {
            for entry in entries.flatten() {
                let lib = entry.path().join("lib").join("x64").join("OpenCL.lib");
                if lib.exists() {
                    println!("cargo:warning=Found OpenCL.lib at: {}", lib.display());
                    return Some(lib.parent().unwrap().to_path_buf());
                }
            }
        }
    }
    for path in &common_paths[1..] {
        let lib = Path::new(path).join("OpenCL.lib");
        if lib.exists() {
            println!("cargo:warning=Found OpenCL.lib at: {}", lib.display());
            return Some(lib.parent().unwrap().to_path_buf());
        }
    }

    // 4. Not found — auto-generate from the system OpenCL.dll
    println!("cargo:warning=OpenCL.lib not found in SDK paths, generating from system OpenCL.dll...");
    match generate_opencl_lib_from_dll(&out_dir) {
        Ok(()) => Some(out_dir),
        Err(e) => {
            println!("cargo:warning=Failed to generate OpenCL.lib: {e}");
            println!(
                "cargo:warning=Please install an OpenCL SDK (Intel, AMD, or NVIDIA CUDA Toolkit)"
            );
            println!(
                "cargo:warning=Download OCL_SDK_Light from: https://github.com/GPUOpen-LibrariesAndSDKs/OCL-SDK/releases"
            );
            None
        }
    }
}

/// Generate OpenCL.lib from C:\Windows\System32\OpenCL.dll using VS tools.
fn generate_opencl_lib_from_dll(out_dir: &Path) -> Result<(), String> {
    let system32 = Path::new(r"C:\Windows\System32");
    let dll_path = system32.join("OpenCL.dll");
    if !dll_path.exists() {
        return Err(format!("OpenCL.dll not found at {}", dll_path.display()));
    }

    // Find VS tools: prefer the path we can detect from the linker,
    // fall back to common VS install paths.
    let (dumpbin, lib) = find_vs_tools()?;

    let def_path = out_dir.join("OpenCL.def");
    let lib_path = out_dir.join("OpenCL.lib");

    // Step 1: dumpbin /exports to get exported symbols
    let output = Command::new(&dumpbin)
        .args(["/exports", dll_path.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to run dumpbin: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("dumpbin failed with status {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Step 2: Parse exports and write .def file
    let exports = parse_dumpbin_exports(&stdout);
    if exports.is_empty() {
        return Err("No exports found in OpenCL.dll".to_string());
    }

    let mut def_content = String::from("EXPORTS\n");
    for export in &exports {
        def_content.push_str(&format!("    {}\n", export));
    }
    std::fs::write(&def_path, &def_content)
        .map_err(|e| format!("Failed to write .def file: {e}"))?;

    // Step 3: Run lib.exe to generate the import library
    let output = Command::new(&lib)
        .args([
            &format!("/def:{}", def_path.display()),
            "/machine:x64",
            &format!("/out:{}", lib_path.display()),
        ])
        .output()
        .map_err(|e| format!("Failed to run lib.exe: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("lib.exe failed: {stderr}"));
    }

    println!("cargo:warning=Generated OpenCL.lib successfully from system OpenCL.dll");
    Ok(())
}

/// Parse the output of `dumpbin /exports` and extract function names.
fn parse_dumpbin_exports(output: &str) -> Vec<String> {
    let mut exports = Vec::new();
    let mut in_exports_section = false;

    for line in output.lines() {
        // Start of exports section
        if line.contains("ordinal hint RVA") {
            in_exports_section = true;
            continue;
        }
        // End of exports section
        if in_exports_section && line.starts_with("  Summary") {
            break;
        }
        if in_exports_section && line.trim().is_empty() {
            continue;  // skip blank lines between header and first export
        }
        if in_exports_section {
            // Lines look like: "          1    0 00002820 clBuildProgram"
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Split by whitespace, the last part is the function name
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let name = parts[parts.len() - 1];
                // Skip forwarded exports and odd entries
                if !name.contains('.') && !name.starts_with('?') {
                    exports.push(name.to_string());
                }
            }
        }
    }
    exports
}

/// Locate dumpbin.exe and lib.exe from Visual Studio installation.
fn find_vs_tools() -> Result<(PathBuf, PathBuf), String> {
    // Strategy 1: Check common VS install paths
    let vs_editions = ["Community", "Professional", "Enterprise"];

    for program_files in &[
        Path::new(r"C:\Program Files\Microsoft Visual Studio"),
        Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio"),
    ] {
        if let Ok(entries) = std::fs::read_dir(program_files) {
            for entry in entries.flatten() {
                let year_dir = entry.path();
                if !year_dir.is_dir() {
                    continue;
                }
                for edition in &vs_editions {
                    let vc_tools = year_dir
                        .join(edition)
                        .join("VC")
                        .join("Tools")
                        .join("MSVC");
                    if !vc_tools.exists() {
                        continue;
                    }
                    // Find the latest MSVC version
                    if let Ok(versions) = std::fs::read_dir(&vc_tools) {
                        let mut version_dirs: Vec<_> = versions
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().is_dir())
                            .collect();
                        // Sort by version (newest first)
                        version_dirs.sort_by(|a, b| b.file_name().cmp(&a.file_name()));

                        for ver_dir in &version_dirs {
                            let host_dir = ver_dir.path().join("bin").join("HostX64").join("x64");
                            let dumpbin = host_dir.join("dumpbin.exe");
                            let lib = host_dir.join("lib.exe");
                            if dumpbin.exists() && lib.exists() {
                                return Ok((dumpbin, lib));
                            }
                        }
                    }
                }
            }
        }
    }

    // Strategy 2: Try vswhere (ships with VS 2017+)
    if let Ok(output) = Command::new("vswhere")
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
    {
        if output.status.success() {
            let install_path = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_string();
            if !install_path.is_empty() {
                let vs_path = Path::new(&install_path);
                let vc_tools = vs_path.join("VC").join("Tools").join("MSVC");
                if let Ok(versions) = std::fs::read_dir(&vc_tools) {
                    let mut version_dirs: Vec<_> = versions
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .collect();
                    version_dirs.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
                    for ver_dir in &version_dirs {
                        let host_dir = ver_dir.path().join("bin").join("HostX64").join("x64");
                        let dumpbin = host_dir.join("dumpbin.exe");
                        let lib = host_dir.join("lib.exe");
                        if dumpbin.exists() && lib.exists() {
                            return Ok((dumpbin, lib));
                        }
                    }
                }
            }
        }
    }

    Err(
        "Visual Studio with C++ tools not found. Install Visual Studio Build Tools or set up an OpenCL SDK.".to_string(),
    )
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
