fn main() {
    // Tauri validates external binary names even for unit-test builds. Release builds fail
    // closed unless the workflow staged real, target-matched binaries beforehand.
    if std::env::var("PROFILE").as_deref() != Ok("release") {
        let target = std::env::var("TARGET").expect("Cargo TARGET");
        let extension = if target.contains("windows") { ".exe" } else { "" };
        let directory = std::path::Path::new("binaries");
        std::fs::create_dir_all(directory).expect("create debug sidecar directory");
        for binary in ["commonkit", "commonkitd", "commonkit-target-helper"] {
            let path = directory.join(format!("{binary}-{target}{extension}"));
            if !path.exists() {
                std::fs::write(path, []).expect("create debug-only sidecar placeholder");
            }
        }
    }
    tauri_build::build()
}
