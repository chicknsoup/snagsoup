use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_windows_sdk_rc() -> Option<PathBuf> {
    let kits_bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = fs::read_dir(kits_bin) {
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("x64").join("rc.exe"))
            .filter(|p| p.is_file())
            .collect();
        candidates.sort();
        if let Some(latest) = candidates.pop() {
            return Some(latest);
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-changed=assets/snagsoup.rc");
    println!("cargo:rerun-if-changed=assets/snagsoup.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let rc_file = manifest_dir.join("assets").join("snagsoup.rc");
    let res_file = out_dir.join("snagsoup.res");

    let rc_exe = find_windows_sdk_rc().unwrap_or_else(|| PathBuf::from("rc.exe"));
    let status = Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_file)
        .arg(&rc_file)
        .status()
        .unwrap_or_else(|e| panic!("failed to execute {:?}: {e}", rc_exe));

    assert!(status.success(), "rc.exe failed to compile snagsoup.rc");
    println!("cargo:rustc-link-arg-bins={}", res_file.display());
}
