fn main() {
    let mut attributes = tauri_build::Attributes::new();
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        attributes = attributes.windows_attributes(windows);
    }
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
