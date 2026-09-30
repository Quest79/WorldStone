//! Per-instance settings the person can tune: how many bots, which realm, who may connect.
//!
//! They live in `Config/settings.json`, are created the first time the configuration is
//! generated (with defaults scaled to the machine Docker runs on) and are never replaced after
//! that, so a different machine or a later update does not quietly change a tuned server.

use crate::{builder::Engine, paths::InstancePaths};
use serde::{Deserialize, Serialize};

const FILE: &str = "settings.json";

/// Upstream's own defaults are 50 to 200 bots regardless of the machine. These are scaled.
const BOTS_PER_CPU: u64 = 8;
const BOTS_PER_GIB: u64 = 8;
const BOTS_CEILING: u64 = 200;
const BOTS_FLOOR: u64 = 40;
/// Room for real players beyond the bots.
const PLAYER_SLOTS: u32 = 100;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// The id of the realm's row in the realm list; the world server must be given the same one.
    pub realm_id: u32,
    pub bots: BotSettings,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BotSettings {
    pub enabled: bool,
    /// Bots kept online at least.
    pub min: u32,
    /// Bots kept online at most.
    pub max: u32,
}

impl Settings {
    /// Defaults for a machine: as many bots as its CPUs and memory suit, half of them the minimum.
    pub fn for_machine(engine: &Engine) -> Self {
        let by_memory = (engine.memory >> 30) * BOTS_PER_GIB;
        let by_cpu = u64::from(engine.cpus) * BOTS_PER_CPU;
        let max = by_memory.min(by_cpu).min(BOTS_CEILING).max(BOTS_FLOOR) as u32;
        Self { realm_id: 1, bots: BotSettings { enabled: true, min: max / 2, max } }
    }

    /// Accounts the bots need, one character per class on each (`classes_per_account`, which the
    /// game's classes decide), with a margin so the roster is larger than the most that is ever
    /// online and the bots have someone to swap with.
    pub fn bot_accounts(&self, classes_per_account: u32) -> u32 {
        (self.bots.max * 3).div_ceil(2 * classes_per_account).max(5)
    }

    /// The server's player cap. Bots count towards it, so it has to leave room for real players.
    pub fn player_limit(&self) -> u32 {
        self.bots.max + PLAYER_SLOTS
    }

    pub fn problem(&self) -> Option<String> {
        if self.realm_id == 0 {
            return Some("The realm id must be at least 1.".into());
        }
        if self.bots.max > 1000 {
            return Some("More than 1000 bots is not supported.".into());
        }
        if self.bots.min > self.bots.max {
            return Some("The minimum number of bots cannot be above the maximum.".into());
        }
        None
    }
}

pub fn load(paths: &InstancePaths) -> Result<Option<Settings>, String> {
    let path = paths.config.join(FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let settings: Settings = serde_json::from_str(&text)
                .map_err(|e| format!("{} cannot be read ({e}). Fix or delete it to get the defaults back.", path.display()))?;
            match settings.problem() {
                None => Ok(Some(settings)),
                Some(p) => Err(format!("{} is not usable: {p}", path.display())),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

pub fn save(paths: &InstancePaths, settings: &Settings) -> Result<(), String> {
    if let Some(problem) = settings.problem() {
        return Err(problem);
    }
    std::fs::create_dir_all(&paths.config).map_err(|e| format!("Cannot create {}: {e}", paths.config.display()))?;
    let path = paths.config.join(FILE);
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, text).map_err(|e| format!("Cannot write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

/// The saved settings, or the defaults for this machine, which are then saved.
pub fn load_or_create(paths: &InstancePaths, engine: &Engine) -> Result<Settings, String> {
    if let Some(settings) = load(paths)? {
        return Ok(settings);
    }
    let settings = Settings::for_machine(engine);
    save(paths, &settings)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(cpus: u32, gib: u64) -> Engine {
        Engine { cpus, memory: gib << 30 }
    }

    #[test]
    fn defaults_follow_the_machine_within_sane_limits() {
        assert_eq!(Settings::for_machine(&engine(12, 16)).bots, BotSettings { enabled: true, min: 48, max: 96 });
        assert_eq!(Settings::for_machine(&engine(2, 4)).bots.max, 40, "a small machine still gets a working roster");
        assert_eq!(Settings::for_machine(&engine(64, 128)).bots.max, 200, "never above upstream's own default ceiling");
        let s = Settings::for_machine(&engine(4, 64));
        assert_eq!(s.bots.max, 40, "few CPUs limit it even with plenty of memory");
        assert!(s.bots.min <= s.bots.max);
    }

    #[test]
    fn the_roster_is_larger_than_the_most_that_is_online() {
        let s = Settings::for_machine(&engine(12, 16));
        assert!(s.bot_accounts(9) * 9 > s.bots.max, "{} accounts", s.bot_accounts(9));
        assert_eq!(s.bot_accounts(9), 16);
        assert_eq!(s.bot_accounts(10), 15, "a game with ten classes needs fewer accounts");
        assert!(s.player_limit() >= s.bots.max + PLAYER_SLOTS, "bots must not use up the slots real players need");
    }

    #[test]
    fn saved_settings_are_kept_and_bad_ones_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        assert_eq!(load(&paths).unwrap(), None);
        let first = load_or_create(&paths, &engine(12, 16)).unwrap();
        // A different machine later must not change what was saved.
        assert_eq!(load_or_create(&paths, &engine(2, 4)).unwrap(), first);
        let mut tuned = first.clone();
        tuned.bots.max = 10;
        tuned.bots.min = 20;
        assert!(save(&paths, &tuned).is_err(), "minimum above maximum");
        std::fs::write(paths.config.join(FILE), "{ not json").unwrap();
        assert!(load(&paths).unwrap_err().contains("cannot be read"));
    }
}
