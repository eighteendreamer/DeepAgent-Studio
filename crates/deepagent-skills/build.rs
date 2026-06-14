//! Build script for `deepagent-skills`.
//!
//! On Windows + MSVC we embed an `asInvoker` UAC manifest into every test
//! binary so the OS does not flag tests like `e2e_market_install` as
//! installers and demand elevation. Without this, `cargo test` fails with
//! `os error 740` ("requires elevation"). The link-arg is a no-op
//! everywhere else.
//!
//! Implementation: we compile a `.manifest` file and ask `link.exe` to merge
//! it into the test binary's manifest via `/MANIFESTINPUT:`. The
//! `/MANIFESTUAC:level=asInvoker` short form alone proved unreliable in
//! local testing (the rustc-supplied default manifest seemed to win).

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        let manifest_dir =
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during build");
        let manifest_path = std::path::Path::new(&manifest_dir).join("asInvoker.manifest");
        // Tell cargo to rerun if the manifest changes.
        println!("cargo:rerun-if-changed={}", manifest_path.display());
        // Force-enable manifest generation, then merge our trustInfo block
        // declaring `requestedExecutionLevel level="asInvoker"`.
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
            manifest_path.display()
        );
    }
}
