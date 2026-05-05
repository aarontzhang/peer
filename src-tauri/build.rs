fn main() {
    #[cfg(target_os = "macos")]
    {
        // Embed Info.plist into the dev binary so macOS can resolve a stable
        // CFBundleIdentifier even when running the raw target/debug/Peer
        // executable. Without this, Cargo's linker-signed adhoc identity is
        // a hash that changes per build, and TCC treats every rebuild as a
        // different app — re-prompting for screen recording forever.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let plist = format!("{manifest_dir}/Info.plist");
        println!("cargo:rerun-if-changed=Info.plist");
        println!("cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{plist}");
    }
    tauri_build::build();
}
