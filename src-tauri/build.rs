fn main() {
    if std::env::var("CARGO_CFG_TEST").is_err() {
        tauri_build::build();
    }
}
