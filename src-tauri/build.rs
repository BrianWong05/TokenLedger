fn main() {
    for (key, value) in std::env::vars() {
        if key.starts_with("CARGO") {
            println!("cargo:warning={}={}", key, value);
        }
    }

    if std::env::var("CARGO_CFG_TEST").is_ok() {
        return;
    }

    tauri_build::build();
}
