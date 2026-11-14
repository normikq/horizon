use crate::State;
use crate::types::{SCREENS_FILE, SavedScreen};
use horizon_client::mod_api::{Host, serde_json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where the screens are kept: the game's shared config folder
/// (`config/video_screens.json`), not wherever the game was started from.
fn screens_path(host: &dyn Host) -> PathBuf {
    host.config_dir()
        .map(|dir| dir.join(SCREENS_FILE))
        .unwrap_or_else(|_| PathBuf::from(SCREENS_FILE))
}

/// Where earlier versions kept the screens: the working directory, then
/// `%APPDATA%\horizon`. Read once while the config folder has none.
fn legacy_screens_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(SCREENS_FILE)];
    if let Ok(appdata) = std::env::var("APPDATA") {
        paths.push(PathBuf::from(appdata).join("horizon").join(SCREENS_FILE));
    }
    paths
}

/// The screens saved at `path`, or at an earlier version's place while there are none.
fn load_saved_screens(path: &Path) -> HashMap<String, SavedScreen> {
    let read = |path: &Path| std::fs::read_to_string(path).ok();
    let Some(content) = read(path).or_else(|| legacy_screens_paths().iter().find_map(|p| read(p)))
    else {
        return HashMap::new();
    };
    let content = content.trim_start_matches('\u{feff}');
    serde_json::from_str(content).unwrap_or_default()
}

/// Reads the saved screens the first time a hook can ask the game where they are.
pub(crate) fn ensure_screens(host: &dyn Host, state: &mut State) {
    if state.screens_path.is_none() {
        let path = screens_path(host);
        state.saved_screens = load_saved_screens(&path);
        state.screens_path = Some(path);
    }
}

pub(crate) fn save_saved_screens(state: &State) -> Result<(), String> {
    let path = state
        .screens_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(SCREENS_FILE));
    let json = serde_json::to_string_pretty(&state.saved_screens)
        .map_err(|e| format!("failed to serialize screens: {e}"))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, json).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(())
}
