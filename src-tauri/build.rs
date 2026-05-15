fn main() {
    #[cfg(target_os = "macos")]
    {
        // Embed usage-description keys into the dev binary so macOS can show
        // the standard mic/screen prompts before dev-runner applies a stable
        // ad-hoc signing identifier.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let plist = format!("{manifest_dir}/Info.plist");
        println!("cargo:rerun-if-changed=Info.plist");
        println!("cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{plist}");
    }
    tauri_build::build();
}
