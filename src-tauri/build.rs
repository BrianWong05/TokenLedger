fn main() {
    let mut attributes = tauri_build::Attributes::new();

    let windows = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows";
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() == "msvc";
    if windows && msvc {
        // tauri-plugin-dialog imports entry points that only exist in Common
        // Controls v6, which Windows activates solely from the executable's
        // application manifest. tauri-build embeds that manifest into binaries
        // only (rustc-link-arg-bins), so a test executable loads v5 and dies
        // with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) before running any
        // test — tauri-apps/tauri#13419. Tauri's own workaround gates on
        // __TAURI_WORKSPACE__ and resolves the manifest relative to its own
        // monorepo, so it cannot work downstream. Do what it does instead:
        // suppress the per-bin manifest and embed ours into every link target
        // (binaries and tests alike) via /MANIFEST:EMBED.
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        let manifest = std::env::current_dir()
            .expect("build script runs with the crate root as cwd")
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }

    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
