fn main() {
    #[cfg(all(windows, feature = "gui"))]
    {
        use sha2::{Digest, Sha256};
        use std::{env, fs, path::PathBuf};
        println!("cargo:rerun-if-env-changed=DEEPAGENT_INSTALLER_PAYLOAD");
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        let code = if let Some(folder) = env::var_os("DEEPAGENT_INSTALLER_PAYLOAD") {
            let folder = fs::canonicalize(folder).expect("resolve installer payload directory");
            let exe = folder.join("setup.exe");
            let manifest = folder.join("manifest.json");
            println!("cargo:rerun-if-changed={}", exe.display());
            println!("cargo:rerun-if-changed={}", manifest.display());
            let bytes = fs::read(&exe).expect("read original NSIS payload");
            let meta: serde_json::Value =
                serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
            assert_eq!(
                meta["setup_sha256"].as_str().unwrap(),
                format!("{:x}", Sha256::digest(&bytes)),
                "NSIS payload digest mismatch"
            );
            format!("pub static SETUP: &[u8] = include_bytes!({:?});\npub const MANIFEST: &str = include_str!({:?});\n", exe, manifest)
        } else {
            assert_ne!(env::var("PROFILE").unwrap(), "release", "release installer requires DEEPAGENT_INSTALLER_PAYLOAD; refusing an empty installer");
            "pub static SETUP: &[u8] = &[];\npub const MANIFEST: &str = \"\";\n".into()
        };
        fs::write(out.join("payload.rs"), code).unwrap();
        tauri_build::build();
    }
}
