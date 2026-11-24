fn main() {
    #[cfg(windows)]
    {
        // tauri-build normally embeds this manifest only through generated
        // resources, which leaves the Rust unit-test executable without the
        // Common Controls v6 dependency and causes STATUS_ENTRYPOINT_NOT_FOUND.
        // Embed the same manifest through the linker for every Windows target.
        let attributes = tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        tauri_build::try_build(attributes).expect("failed to run tauri-build");

        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            let manifest = std::path::PathBuf::from(
                std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"),
            )
            .join("windows-app-manifest.xml");
            println!("cargo:rerun-if-changed={}", manifest.display());
            println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        }
    }

    #[cfg(not(windows))]
    tauri_build::build();
}
