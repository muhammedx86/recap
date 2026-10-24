//! Where everything lives on disk.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

#[derive(Debug, Clone)]
pub struct Paths {
    /// %APPDATA%/com.recap.app
    pub data_dir: PathBuf,
    pub settings_file: PathBuf,
    pub library_db: PathBuf,
    /// User game/detector folders (override bundled ones).
    pub user_games: PathBuf,
    /// Bundled game folders shipped with the app.
    pub bundled_games: PathBuf,
    pub thumbnails: PathBuf,
    /// recap-recorder executable (next to the OBS binaries).
    pub recorder_exe: PathBuf,
    /// Folders searched for ffmpeg/ffprobe.
    pub ffmpeg_dirs: Vec<PathBuf>,
}

impl Paths {
    pub fn resolve(app: &AppHandle) -> anyhow::Result<Self> {
        // RECAP_DATA_DIR isolates a run (used by the end-to-end test).
        let data_dir = match std::env::var_os("RECAP_DATA_DIR").filter(|v| !v.is_empty()) {
            Some(d) => PathBuf::from(d),
            None => app.path().app_data_dir()?,
        };
        let resources = app.path().resource_dir().unwrap_or_else(|_| exe_dir());
        // In `tauri dev` resources are not copied; fall back to the repo layout.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
        let pick = |candidates: Vec<PathBuf>| {
            candidates
                .iter()
                .find(|p| p.exists())
                .cloned()
                .unwrap_or_else(|| candidates[0].clone())
        };

        let recorder_name = if cfg!(windows) { "recap-recorder.exe" } else { "recap-recorder" };
        let recorder_exe = pick(vec![
            resources.join("obs").join(recorder_name),
            repo.join("obs").join(recorder_name),
            repo.join("..")
                .join("target")
                .join(if cfg!(debug_assertions) { "debug" } else { "release" })
                .join(recorder_name),
        ]);
        let bundled_games = pick(vec![resources.join("games"), repo.join("..").join("games")]);
        let ffmpeg_dirs = vec![resources.join("binaries"), repo.join("binaries"), exe_dir()];

        Ok(Self {
            settings_file: data_dir.join("settings.json"),
            library_db: data_dir.join("library.db"),
            user_games: data_dir.join("games"),
            thumbnails: data_dir.join("thumbnails"),
            data_dir,
            bundled_games,
            recorder_exe,
            ffmpeg_dirs,
        })
    }

    /// Bundled games, then RECAP_EXTRA_GAMES (test fixtures), then the user's folder (wins).
    pub fn game_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.bundled_games.clone()];
        if let Some(extra) = std::env::var_os("RECAP_EXTRA_GAMES").filter(|v| !v.is_empty()) {
            roots.push(PathBuf::from(extra));
        }
        roots.push(self.user_games.clone());
        roots
    }
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// Default recordings folder: <Videos>/Recap.
pub fn default_recordings_dir() -> PathBuf {
    dirs::video_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Recap")
}

/// Make a string safe to use as a file or folder name.
pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() {
        "untitled".into()
    } else {
        s.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    /// Tauri flattens glob resources (`dir/**/*`), which broke the per-game folders and the
    /// OBS plugin layout in the installer. Resources must be listed as whole directories.
    #[test]
    fn bundle_resources_keep_folder_structure() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json parses");
        let res = conf["bundle"]["resources"].as_object().expect("resources map");
        for key in ["obs", "binaries", "../games"] {
            assert!(res.contains_key(key), "missing resource dir {key}");
        }
        assert!(
            res.keys().all(|k| !k.contains('*')),
            "glob resources get flattened: {:?}",
            res.keys()
        );
    }

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize("League of Legends"), "League of Legends");
        assert_eq!(sanitize("a/b:c*?"), "a_b_c__");
        assert_eq!(sanitize("..."), "untitled");
    }
}
