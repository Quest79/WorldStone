//! Playerbots (Steps 5 and 7): how many there should be, and whether they really are in the world.
//!
//! "Working" is never assumed from the configuration. It is read from the database: how many bot
//! characters exist and how many of them are online right now, next to what the server is doing.
//! The state is worked out from those facts each time it is asked, so it is right after the app
//! is restarted, too.

use crate::{
    accounts::console_service,
    builder, compose,
    config,
    database::{self, Credentials},
    paths::InstancePaths,
    recipe::{AccountsSpec, PlayerbotsSpec, Recipe},
    runtime::{self, clean_log_line_to, ServiceState},
    settings::{self, BotSettings},
    tasks::Task,
};
use serde::{Deserialize, Serialize};

/// Bots are created and log in gradually, several a second at best. A machine that has not
/// brought its bots online this long after the world server started is called stalled.
const GRACE_SECS: u64 = 10 * 60;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum BotsState {
    /// Docker, the database or the settings could not be read.
    Unknown,
    /// Switched off in the settings.
    Off,
    /// The world server is not running.
    Stopped,
    /// The world server is loading.
    Starting,
    /// The world is up and the bots' characters are still being created.
    Creating,
    /// The world is up and the bots are logging in.
    LoggingIn,
    /// At least the minimum are in the world.
    Online,
    /// The world has been up long enough and the bots are not there.
    Stalled,
}

/// What was read, before it is judged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub enabled: bool,
    pub min: u32,
    pub max: u32,
    /// Bot characters wanted, from the number of accounts the settings ask for.
    pub wanted_characters: u32,
    pub player_limit: u32,
    pub world: Option<ServiceState>,
    pub uptime_secs: Option<u64>,
    pub characters: u32,
    pub online: u32,
    pub players: u32,
    /// The world server's last error line since it started, when it printed one.
    pub last_error: Option<String>,
}

/// Judges the facts: the state, and a sentence that says what is going on.
pub fn classify(f: &Facts) -> (BotsState, String) {
    if !f.enabled || f.max == 0 {
        return (BotsState::Off, "Playerbots are switched off in the settings.".to_string());
    }
    match f.world {
        None | Some(ServiceState::Missing | ServiceState::Stopped) => {
            return (BotsState::Stopped, "The world server is not running, so no bots are in the world.".to_string());
        }
        Some(ServiceState::Crashed | ServiceState::Unresponsive) => {
            return (BotsState::Stopped, "The world server is not working, so the bots are not either.".to_string());
        }
        Some(ServiceState::Paused) => {
            return (BotsState::Stopped, "The world server is paused, so the bots are frozen with it.".to_string());
        }
        Some(ServiceState::Starting) => {
            return (BotsState::Starting, "The world server is still loading.".to_string());
        }
        Some(ServiceState::Running) => {}
    }
    if f.online >= f.min && f.online > 0 {
        return (BotsState::Online, format!("{} bots are in the world.", f.online));
    }
    let grace_over = f.uptime_secs.is_some_and(|up| up >= GRACE_SECS);
    if !grace_over {
        if f.characters < f.wanted_characters {
            return (BotsState::Creating, format!("Creating the bots' characters: {} of {}.", f.characters, f.wanted_characters));
        }
        return (BotsState::LoggingIn, format!("The bots are logging in: {} so far, {} wanted at least.", f.online, f.min));
    }
    let cause = if f.characters == 0 {
        "No bot characters exist. The bots' accounts may not have been created; check the world server's log.".to_string()
    } else if f.characters < f.min {
        format!("Only {} bot characters exist, fewer than the {} wanted online.", f.characters, f.min)
    } else if f.online + f.players >= f.player_limit {
        format!("The server's player limit of {} is full.", f.player_limit)
    } else if let Some(error) = &f.last_error {
        format!("The world server's last error was: {error}")
    } else {
        "The world server printed no error; check its log.".to_string()
    };
    (BotsState::Stalled, format!("Only {} of at least {} bots are online. {cause}", f.online, f.min))
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BotsStatus {
    pub state: BotsState,
    pub note: String,
    pub enabled: bool,
    pub min: u32,
    pub max: u32,
    pub player_limit: u32,
    /// `None` when the database could not be asked.
    pub bot_accounts: Option<u32>,
    pub characters: Option<u32>,
    pub online: Option<u32>,
    /// Real players in the world, not counting bots.
    pub players: Option<u32>,
    /// The saved settings differ from what the running world server was started with.
    pub restart_needed: bool,
}

fn unknown(note: String, settings: Option<&settings::Settings>) -> BotsStatus {
    BotsStatus {
        state: BotsState::Unknown,
        note,
        enabled: settings.is_some_and(|s| s.bots.enabled),
        min: settings.map_or(0, |s| s.bots.min),
        max: settings.map_or(0, |s| s.bots.max),
        player_limit: settings.map_or(0, |s| s.player_limit()),
        bot_accounts: None,
        characters: None,
        online: None,
        players: None,
        restart_needed: false,
    }
}

// ---------------------------------------------------------------------------------------
// Counting
// ---------------------------------------------------------------------------------------

/// Bot accounts, bot characters, bot characters in the world, and real characters in the world,
/// each on its own tagged row.
fn count_sql(accounts: &AccountsSpec, bots: &PlayerbotsSpec) -> String {
    let account_table = format!("`{}`.`{}`", accounts.schema, accounts.table);
    let character_table = format!("`{}`.`{}`", bots.characters_schema, bots.characters_table);
    let join = format!("{character_table} c LEFT JOIN {account_table} a ON a.`{}` = c.`{}`", accounts.id_column, bots.characters_account_column);
    // A bot account is flagged by a column, or else recognised by the prefix of its name.
    let is_bot = |table: &str| match &accounts.bot_column {
        Some(column) => format!("IFNULL({table}`{column}`, 0) <> 0"),
        None => format!("IFNULL(LEFT({table}`{}`, {}) = '{}', 0) <> 0", accounts.name_column, accounts.bot_prefix.len(), accounts.bot_prefix),
    };
    let (account_is_bot, bot, online) = (is_bot(""), is_bot("a."), format!("c.`{}` <> 0", bots.characters_online_column));
    format!(
        "SELECT 'accounts', COUNT(*) FROM {account_table} WHERE {account_is_bot};\n\
         SELECT 'characters', COUNT(*) FROM {join} WHERE {bot};\n\
         SELECT 'online', COUNT(*) FROM {join} WHERE {bot} AND {online};\n\
         SELECT 'players', COUNT(*) FROM {join} WHERE NOT ({bot}) AND {online};\n"
    )
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub accounts: u32,
    pub characters: u32,
    pub online: u32,
    pub players: u32,
}

pub fn parse_counts(text: &str) -> Option<Counts> {
    let mut counts = Counts::default();
    let mut seen = 0;
    for line in text.lines() {
        let Some((tag, value)) = line.split_once('\t') else { continue };
        let Ok(value) = value.trim().parse::<u32>() else { continue };
        match tag {
            "accounts" => counts.accounts = value,
            "characters" => counts.characters = value,
            "online" => counts.online = value,
            "players" => counts.players = value,
            _ => continue,
        }
        seen += 1;
    }
    (seen == 4).then_some(counts)
}

/// The last line of a log that reports an error, cleaned for showing.
pub fn last_error_line(log: &str) -> Option<String> {
    log.lines().filter_map(|l| clean_log_line_to(l, 140)).filter(|l| l.starts_with("ERROR") || l.starts_with("FATAL")).last()
}

/// Reads the settings, the servers and the database; starts and changes nothing.
pub async fn status(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> BotsStatus {
    let settings = match settings::load(paths) {
        Ok(Some(settings)) => settings,
        Ok(None) => return unknown("Generate the configuration first; that also chooses how many bots suit this machine.".to_string(), None),
        Err(e) => return unknown(e, None),
    };
    if let Err(reason) = builder::engine().await {
        return unknown(reason, Some(&settings));
    }
    let world_spec = match console_service(recipe) {
        Ok(spec) => spec,
        Err(e) => return unknown(e, Some(&settings)),
    };
    let server = runtime::status(paths, instance, recipe).await;
    let world = server.services.iter().find(|s| s.id == world_spec.id);
    let mut facts = Facts {
        enabled: settings.bots.enabled,
        min: settings.bots.min,
        max: settings.bots.max,
        wanted_characters: settings.bot_accounts(recipe.playerbots.classes_per_account) * recipe.playerbots.classes_per_account,
        player_limit: settings.player_limit(),
        world: world.map(|s| s.state),
        uptime_secs: world.and_then(|s| s.uptime_secs),
        ..Facts::default()
    };

    // Counts are only read from a database that answers. They are only meaningful while the world
    // runs (it clears the online flags when it starts), but the characters that exist are worth
    // showing either way.
    let mut counts = None;
    let mut trouble = None;
    match Credentials::load(paths) {
        Ok(Some(creds)) if database::is_healthy(instance).await => {
            match database::query(instance, &creds, &count_sql(&recipe.accounts, &recipe.playerbots)).await {
                Ok(text) => match parse_counts(&text) {
                    Some(c) => counts = Some(c),
                    None => trouble = Some("The database gave an answer the app could not read.".to_string()),
                },
                Err(e) => trouble = Some(format!("The bots could not be counted: {}", e.trim())),
            }
        }
        Ok(_) => {}
        Err(e) => trouble = Some(e),
    }
    let running = facts.world == Some(ServiceState::Running);
    if let Some(c) = &counts {
        facts.characters = c.characters;
        if running {
            facts.online = c.online;
            facts.players = c.players;
        }
    } else if running && facts.enabled {
        // The world runs but the bots cannot be counted: say so instead of guessing.
        let note = trouble.unwrap_or_else(|| "The database is not answering, so the bots cannot be counted.".to_string());
        let mut out = unknown(note, Some(&settings));
        out.restart_needed = server.restart_needed;
        return out;
    }

    let (mut state, mut note) = classify(&facts);
    if state == BotsState::Stalled {
        let log = runtime::current_log(&compose::service_container(instance, &world_spec.id), 300).await;
        facts.last_error = last_error_line(&log);
        (state, note) = classify(&facts);
    }
    BotsStatus {
        state,
        note,
        enabled: facts.enabled,
        min: facts.min,
        max: facts.max,
        player_limit: facts.player_limit,
        bot_accounts: counts.as_ref().map(|c| c.accounts),
        characters: counts.as_ref().map(|c| c.characters),
        online: (counts.is_some() && running).then_some(facts.online),
        players: (counts.is_some() && running).then_some(facts.players),
        restart_needed: server.restart_needed,
    }
}

// ---------------------------------------------------------------------------------------
// Changing the settings
// ---------------------------------------------------------------------------------------

/// What the person chose for the bots.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BotChoice {
    pub enabled: bool,
    pub min: u32,
    pub max: u32,
}

/// The settings with the choice applied, or why it cannot be.
pub fn apply_choice(current: &settings::Settings, choice: BotChoice) -> Result<settings::Settings, String> {
    let mut next = current.clone();
    next.bots = BotSettings { enabled: choice.enabled, min: choice.min, max: choice.max };
    match next.problem() {
        Some(problem) => Err(problem),
        None => Ok(next),
    }
}

/// Saves the bot settings and regenerates the configuration from them. The servers keep working from
/// the old configuration until they are restarted; the status says so.
pub async fn save_settings(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, choice: BotChoice) -> Result<(), String> {
    task.step("Checking the settings");
    let current = settings::load(paths)?.ok_or_else(|| "Generate the configuration first.".to_string())?;
    let next = apply_choice(&current, choice)?;
    if next == current {
        task.info("The bot settings are already like that");
        return Ok(());
    }
    task.step("Saving the settings");
    settings::save(paths, &next)?;
    task.info(&format!(
        "Bots {}: {} to {} online, {} accounts, player limit {}",
        if next.bots.enabled { "on" } else { "off" },
        next.bots.min,
        next.bots.max,
        next.bot_accounts(recipe.playerbots.classes_per_account),
        next.player_limit()
    ));
    config::configure(task, paths, instance, recipe).await.map_err(|e| {
        format!("The settings are saved, but the configuration could not be generated from them: {e} Use Update config to try again.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            enabled: true,
            min: 48,
            max: 96,
            wanted_characters: 144,
            player_limit: 196,
            world: Some(ServiceState::Running),
            uptime_secs: Some(60),
            characters: 144,
            online: 0,
            players: 0,
            last_error: None,
        }
    }

    #[test]
    fn switched_off_beats_everything() {
        let (state, note) = classify(&Facts { enabled: false, ..facts() });
        assert_eq!(state, BotsState::Off);
        assert!(note.contains("switched off"));
        assert_eq!(classify(&Facts { max: 0, ..facts() }).0, BotsState::Off);
    }

    #[test]
    fn a_world_that_is_not_up_has_no_bots_in_it() {
        for world in [None, Some(ServiceState::Missing), Some(ServiceState::Stopped), Some(ServiceState::Crashed), Some(ServiceState::Unresponsive)] {
            assert_eq!(classify(&Facts { world, online: 77, ..facts() }).0, BotsState::Stopped, "{world:?}");
        }
        assert_eq!(classify(&Facts { world: Some(ServiceState::Starting), ..facts() }).0, BotsState::Starting);
    }

    #[test]
    fn enough_bots_online_is_online() {
        let (state, note) = classify(&Facts { online: 77, uptime_secs: Some(3600), ..facts() });
        assert_eq!(state, BotsState::Online);
        assert_eq!(note, "77 bots are in the world.");
        assert_eq!(classify(&Facts { online: 48, ..facts() }).0, BotsState::Online);
        assert_ne!(classify(&Facts { online: 47, ..facts() }).0, BotsState::Online);
    }

    #[test]
    fn a_first_start_creates_characters_before_they_log_in() {
        let (state, note) = classify(&Facts { characters: 45, ..facts() });
        assert_eq!(state, BotsState::Creating);
        assert!(note.contains("45 of 144"), "{note}");
        let (state, note) = classify(&Facts { online: 10, ..facts() });
        assert_eq!(state, BotsState::LoggingIn);
        assert!(note.contains("10 so far"), "{note}");
    }

    #[test]
    fn the_grace_period_ends_with_a_reason() {
        let late = Facts { uptime_secs: Some(GRACE_SECS), ..facts() };
        let (state, note) = classify(&Facts { characters: 0, ..late.clone() });
        assert_eq!(state, BotsState::Stalled);
        assert!(note.contains("No bot characters exist"), "{note}");
        assert!(classify(&Facts { characters: 20, ..late.clone() }).1.contains("Only 20 bot characters"));
        assert!(classify(&Facts { online: 30, players: 166, ..late.clone() }).1.contains("player limit of 196 is full"));
        let with_error = Facts { last_error: Some("ERROR: cannot load map".into()), ..late.clone() };
        assert!(classify(&with_error).1.contains("ERROR: cannot load map"));
        assert!(classify(&late).1.contains("printed no error"));
        // An unknown uptime never counts as time passed.
        assert_eq!(classify(&Facts { uptime_secs: None, ..facts() }).0, BotsState::LoggingIn);
    }

    #[test]
    fn the_counting_sql_uses_the_recipes_names_and_keeps_bots_out_of_the_players() {
        let recipe = crate::recipe::builtin().unwrap().remove(0);
        let sql = count_sql(&recipe.accounts, &recipe.playerbots);
        assert!(sql.contains("`character0`.`characters` c LEFT JOIN `realmd`.`account` a ON a.`id` = c.`account`"), "{sql}");
        assert!(sql.contains("SELECT 'accounts', COUNT(*) FROM `realmd`.`account` WHERE IFNULL(`playerBot`, 0) <> 0;"), "{sql}");
        assert!(sql.contains("NOT (IFNULL(a.`playerBot`, 0) <> 0) AND c.`online` <> 0"), "{sql}");
    }

    #[test]
    fn a_game_without_a_bot_flag_recognises_its_bot_accounts_by_name() {
        let recipe = crate::recipe::builtin().unwrap().into_iter().find(|r| r.id == "azerothcore").unwrap();
        let sql = count_sql(&recipe.accounts, &recipe.playerbots);
        assert!(sql.contains("FROM `acore_auth`.`account` WHERE IFNULL(LEFT(`username`, 6) = 'RNDBOT', 0) <> 0;"), "{sql}");
        assert!(sql.contains("NOT (IFNULL(LEFT(a.`username`, 6) = 'RNDBOT', 0) <> 0) AND c.`online` <> 0"), "{sql}");
        assert!(sql.contains("`acore_characters`.`characters` c LEFT JOIN `acore_auth`.`account` a ON a.`id` = c.`account`"), "{sql}");
    }

    #[test]
    fn counts_are_only_believed_when_every_number_is_there() {
        let text = "accounts\t16\ncharacters\t144\nonline\t77\nplayers\t1\n";
        assert_eq!(parse_counts(text), Some(Counts { accounts: 16, characters: 144, online: 77, players: 1 }));
        assert_eq!(parse_counts("accounts\t16\ncharacters\t144\n"), None);
        assert_eq!(parse_counts("accounts\tx\n"), None);
        assert_eq!(parse_counts(""), None);
    }

    #[test]
    fn the_worlds_last_error_is_found_and_cleaned() {
        let log = "Loading maps\n\u{1b}[31mERROR: Table `creature` missing\u{1b}[0m\nINFO ok\nERROR: second one\n";
        assert_eq!(last_error_line(log).as_deref(), Some("ERROR: second one"));
        assert_eq!(last_error_line("all fine\n"), None);
    }

    #[test]
    fn a_choice_is_checked_like_the_saved_settings() {
        let current = settings::Settings { realm_id: 1, bots: BotSettings { enabled: true, min: 48, max: 96 } };
        let next = apply_choice(&current, BotChoice { enabled: false, min: 10, max: 20 }).unwrap();
        assert_eq!(next.bots, BotSettings { enabled: false, min: 10, max: 20 });
        assert_eq!(next.realm_id, 1);
        assert!(apply_choice(&current, BotChoice { enabled: true, min: 50, max: 40 }).unwrap_err().contains("minimum"));
        assert!(apply_choice(&current, BotChoice { enabled: true, min: 0, max: 1001 }).unwrap_err().contains("1000"));
    }
}
