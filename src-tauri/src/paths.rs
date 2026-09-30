//! Where WorldStone keeps things. Nothing here is hard-coded to a user's machine.
//!
//! ```text
//! <root>/Logs/                     launcher log
//! <root>/Settings/appearance.json  fonts and their outline and shadow (app-wide, not per server)
//! <root>/Fonts/                    Google fonts downloaded on request, and the list they were picked from
//! <root>/Instances/<id>/Repos      cloned emulator + database repositories
//!                       /Data      MariaDB files and extracted client data
//!                       /Config    generated credentials and server configuration
//!                       /Logs      server logs
//!                       /Backups   database dumps and replaced data
//!                       /Temp      partial clones and staging directories
//!                       /Docker    generated Compose topology
//! ```
//!
//! Development uses the project directory as the root; a packaged build uses the folder
//! the executable lives in. `WORLDSTONE_HOME` overrides both.

use std::path::{Path, PathBuf};

const INSTANCE_FOLDERS: [&str; 7] = ["Repos", "Data", "Config", "Logs", "Backups", "Temp", "Docker"];

#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct InstancePaths {
    pub root: PathBuf,
    pub repos: PathBuf,
    pub data: PathBuf,
    pub config: PathBuf,
    pub logs: PathBuf,
    pub backups: PathBuf,
    pub temp: PathBuf,
    pub docker: PathBuf,
}

impl Paths {
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn resolve() -> Result<Self, String> {
        if let Some(home) = std::env::var_os("WORLDSTONE_HOME") {
            return Ok(Self::at(PathBuf::from(home)));
        }
        Ok(Self::at(default_root()?))
    }

    /// Launcher-wide logs, outside any instance.
    pub fn logs(&self) -> PathBuf {
        self.root.join("Logs")
    }

    /// The person's appearance choices (fonts, outline, shadow).
    pub fn appearance_file(&self) -> PathBuf {
        self.root.join("Settings").join("appearance.json")
    }

    /// Fonts the app downloaded, and the cached Google Fonts list.
    pub fn fonts(&self) -> PathBuf {
        self.root.join("Fonts")
    }

    /// The launcher's own log: every operation it has run, across restarts.
    pub fn launcher_log(&self) -> PathBuf {
        self.logs().join("worldstone.log")
    }

    /// Paths for one server instance. The id becomes a directory name, so it is validated.
    pub fn instance(&self, id: &str) -> Result<InstancePaths, String> {
        if !valid_id(id) {
            return Err(format!("'{id}' is not a valid instance id"));
        }
        let root = self.root.join("Instances").join(id);
        Ok(InstancePaths {
            repos: root.join("Repos"),
            data: root.join("Data"),
            config: root.join("Config"),
            logs: root.join("Logs"),
            backups: root.join("Backups"),
            temp: root.join("Temp"),
            docker: root.join("Docker"),
            root,
        })
    }
}

impl InstancePaths {
    pub fn ensure(&self) -> Result<(), String> {
        for folder in INSTANCE_FOLDERS {
            std::fs::create_dir_all(self.root.join(folder))
                .map_err(|e| format!("Cannot create {}: {e}", self.root.join(folder).display()))?;
        }
        Ok(())
    }
}

/// Lowercase letters, digits and hyphens; no separators, dots or empty names.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A single path component with no traversal, for names taken from recipe files.
pub fn valid_component(name: &str) -> bool {
    let p = Path::new(name);
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && p.components().count() == 1
}

#[cfg(debug_assertions)]
fn default_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Cannot locate the project directory".to_string())
}

#[cfg(not(debug_assertions))]
fn default_root() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Cannot locate the application directory".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_ids_cannot_escape_the_instances_folder() {
        for bad in ["", "..", "../x", "a/b", "a\\b", "A", "-x", "a b", "a.b"] {
            assert!(!valid_id(bad), "{bad:?} should be rejected");
        }
        assert!(valid_id("mangos-zero"));
        assert!(Paths::at("C:/x".into()).instance("../evil").is_err());
    }

    #[test]
    fn instance_layout_is_under_the_root_and_handles_spaces() {
        let paths = Paths::at(PathBuf::from("C:/Launcher With Spaces"));
        let i = paths.instance("mangos-zero").unwrap();
        assert_eq!(i.repos, PathBuf::from("C:/Launcher With Spaces/Instances/mangos-zero/Repos"));
        assert_eq!(i.temp, PathBuf::from("C:/Launcher With Spaces/Instances/mangos-zero/Temp"));
    }

    #[test]
    fn components_reject_traversal() {
        for bad in ["", ".", "..", "a/b", "a\\b", "C:", "..\\x"] {
            assert!(!valid_component(bad), "{bad:?} should be rejected");
        }
        assert!(valid_component("mangoszero-database"));
    }

    #[test]
    fn ensure_creates_every_folder() {
        let dir = tempfile::tempdir().unwrap();
        let i = Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        i.ensure().unwrap();
        for folder in INSTANCE_FOLDERS {
            assert!(i.root.join(folder).is_dir(), "{folder} missing");
        }
    }
}
