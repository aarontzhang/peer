//! Resolve external binaries (ffmpeg/ffprobe) when launched from a macOS
//! GUI context, where `$PATH` is just `/usr/bin:/bin:/usr/sbin:/sbin` and
//! Homebrew/Anaconda installs are invisible.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Search order: explicit env override → common install prefixes → bare name
/// (lets the OS resolve via PATH if it actually contains the binary).
fn resolve(name: &str, env_var: &str) -> PathBuf {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, PathBuf>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(name) {
        return hit.clone();
    }

    let candidates: Vec<PathBuf> = std::iter::empty()
        .chain(std::env::var_os(env_var).map(PathBuf::from))
        .chain(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/opt/local/bin",
                "/usr/bin",
            ]
            .into_iter()
            .map(|d| PathBuf::from(d).join(name)),
        )
        .chain(
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .into_iter()
                .flat_map(|home| {
                    [
                        home.join("anaconda3/bin").join(name),
                        home.join("miniconda3/bin").join(name),
                        home.join(".cargo/bin").join(name),
                    ]
                }),
        )
        .collect();

    let resolved = candidates
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name));

    cache
        .lock()
        .unwrap()
        .insert(name.to_string(), resolved.clone());
    resolved
}

pub fn ffmpeg() -> PathBuf {
    resolve("ffmpeg", "PEER_FFMPEG")
}

pub fn ffprobe() -> PathBuf {
    resolve("ffprobe", "PEER_FFPROBE")
}
