//! Accounts (Step 7): who can log in.
//!
//! Accounts are read from the realm database, and created or changed through the world server's
//! own console, as the emulator's documentation prescribes: the server hashes the password and
//! knows the rules. The console line travels on the standard input of the container's main
//! process; the password is never an argument of any program, never written to a file and never
//! logged. What happened is then checked in the database, not assumed: the account has to exist
//! and its stored hash has to match the password that was given. Where the database can compute
//! the hash (SHA-1) it does; where it cannot (SRP6, a salted modular exponentiation) the salt and
//! verifier are read and the app works the verifier out itself.

use crate::{
    builder, clock,
    database::{self, sql_quote, Credentials},
    paths::InstancePaths,
    process::{capture, CommandSpec},
    recipe::{AccountsSpec, PasswordHash, Recipe, ServiceSpec},
    runtime::{self, clean_log_line_to, ServiceState},
    tasks::Task,
};
use num_bigint::BigUint;
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::time::{Duration, Instant};

/// How long the world server gets to carry out a console line. It reads its console ten times a
/// second and writes accounts through its database queue.
const CONSOLE_WAIT: Duration = Duration::from_secs(20);
const POLL_EVERY: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------------------
// What may be typed
// ---------------------------------------------------------------------------------------

/// The name as the game stores it (upper case), or why it cannot be used. Letters and digits only:
/// the console splits its line on spaces and reads quotes specially, and the client's login
/// screen does not offer more.
pub fn check_name(spec: &AccountsSpec, name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("Enter an account name.".to_string());
    }
    if name.chars().count() > spec.name_max {
        return Err(format!("The account name can have at most {} characters.", spec.name_max));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("The account name can only use the letters A to Z and the digits 0 to 9.".to_string());
    }
    let upper = name.to_ascii_uppercase();
    if upper.starts_with(&spec.bot_prefix) {
        return Err(format!("Names starting with {} belong to the playerbots. Choose another.", spec.bot_prefix.to_lowercase()));
    }
    Ok(upper)
}

/// Visible ASCII, without the characters the console or the templates read specially. There is no
/// minimum length: the server accepts any password and this is the person's own server.
pub fn check_password(spec: &AccountsSpec, password: &str) -> Result<(), String> {
    let length = password.chars().count();
    if length == 0 {
        return Err("Enter a password.".to_string());
    }
    if length > spec.password_max {
        return Err(format!("The password can have at most {} characters; the game client cannot type more.", spec.password_max));
    }
    let allowed = |c: char| c.is_ascii_graphic() && !matches!(c, '"' | '\'' | '\\' | '`' | '{' | '}');
    if !password.chars().all(allowed) {
        return Err("The password can use letters, digits and the symbols ! # $ % & ( ) * + , - . / : ; < = > ? @ [ ] ^ _ | ~ (no spaces, quotes, backslashes or braces).".to_string());
    }
    Ok(())
}

/// The password a new account gets: what was typed, or, when nothing was, the account's own name
/// (the emulator's default accounts work the same way, so [`list_sql`] flags it). The console
/// cannot take an empty password. The flag is true when the name stands in, so it is not hidden
/// from the log as a secret: it is the name, which the log shows anyway.
pub fn password_for_new_account(spec: &AccountsSpec, name: &str, typed: &str) -> Result<(String, bool), String> {
    if !typed.is_empty() {
        check_password(spec, typed)?;
        return Ok((typed.to_string(), false));
    }
    if name.chars().count() > spec.password_max {
        return Err(format!("The account name is too long to be its password. Enter a password of at most {} characters.", spec.password_max));
    }
    Ok((name.to_string(), true))
}

pub fn check_level(spec: &AccountsSpec, level: u8) -> Result<(), String> {
    if level > spec.max_gm_level {
        return Err(format!("The game-master level can be at most {}.", spec.max_gm_level));
    }
    Ok(())
}

/// The console line for a command template. The tokens are replaced in one pass, so a value that
/// happens to look like a token is never replaced again.
pub fn fill_command(template: &str, name: &str, password: &str, level: u8) -> String {
    let level = level.to_string();
    let tokens: [(&str, &str); 3] = [("{name}", name), ("{password}", password), ("{level}", level.as_str())];
    let mut out = String::with_capacity(template.len() + name.len() + password.len());
    let mut rest = template;
    while let Some(at) = rest.find('{') {
        out.push_str(&rest[..at]);
        let from_brace = &rest[at..];
        match tokens.iter().find(|(token, _)| from_brace.starts_with(token)) {
            Some((token, value)) => {
                out.push_str(value);
                rest = &from_brace[token.len()..];
            }
            None => {
                out.push('{');
                rest = &from_brace[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------------------

fn table(spec: &AccountsSpec) -> String {
    format!("`{}`.`{}`", spec.schema, spec.table)
}

/// The prime of the group WoW 3.x's SRP6 works in, most significant byte first. The generator is 7.
const SRP6_N: &str = "894B645E89E1535BBDAD5B8B290650530801B18EBFBF5E8FAB3C82872A3E9BB7";
const SRP6_G: u32 = 7;

/// The verifier the game stores for an account: `g ^ H(salt, H(NAME:PASSWORD)) mod N`, with the
/// digest read as a little-endian number, written as 32 little-endian bytes. Names and passwords
/// are upper case here, as the server folds them before hashing.
pub fn srp6_verifier(name: &str, password: &str, salt: &[u8]) -> [u8; 32] {
    let credentials = Sha1::digest(format!("{}:{}", name.to_ascii_uppercase(), password.to_ascii_uppercase()));
    let mut outer = Sha1::new();
    outer.update(salt);
    outer.update(credentials);
    let x = BigUint::from_bytes_le(&outer.finalize());
    let n = BigUint::parse_bytes(SRP6_N.as_bytes(), 16).expect("a constant hexadecimal number");
    let bytes = BigUint::from(SRP6_G).modpow(&x, &n).to_bytes_le();
    let mut out = [0u8; 32];
    out[..bytes.len()].copy_from_slice(&bytes);
    out
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

/// SQL for what the database keeps of an account's password, read back by [`cell_matches`]: for
/// SHA-1 a 1 or 0 the database works out, for SRP6 the salt and verifier. `name` and `password` are
/// SQL expressions (a column or an already quoted literal).
fn password_cell(spec: &AccountsSpec, name: &str, password: &str) -> String {
    match spec.password_hash {
        PasswordHash::Sha1NamePassword => format!(
            "IF(UPPER(`{}`) = UPPER(SHA1(CONCAT(UPPER({name}), ':', UPPER({password})))), 1, 0)",
            spec.password_column
        ),
        PasswordHash::Srp6 => {
            let salt = spec.salt_column.as_deref().expect("validated on load");
            format!("CONCAT(HEX(`{salt}`), ':', HEX(`{}`))", spec.password_column)
        }
    }
}

/// Whether what [`password_cell`] printed shows `password` for `name`.
fn cell_matches(spec: &AccountsSpec, cell: &str, name: &str, password: &str) -> bool {
    match spec.password_hash {
        PasswordHash::Sha1NamePassword => cell.trim() == "1",
        PasswordHash::Srp6 => {
            let Some((salt, verifier)) = cell.trim().split_once(':') else { return false };
            match (from_hex(salt), from_hex(verifier)) {
                (Some(salt), Some(verifier)) => verifier == srp6_verifier(name, password, &salt),
                _ => false,
            }
        }
    }
}

/// SQL for an account's game-master level: its own column, or the highest level in the table that
/// keeps them apart.
fn gm_expression(spec: &AccountsSpec) -> String {
    match (&spec.gm_column, &spec.gm_table) {
        (Some(column), _) => format!("`{column}`"),
        (None, Some(gm)) => format!(
            "IFNULL((SELECT MAX(g.`{}`) FROM `{}`.`{}` g WHERE g.`{}` = {}.`{}`), 0)",
            gm.level_column,
            spec.schema,
            gm.table,
            gm.id_column,
            table(spec),
            spec.id_column
        ),
        (None, None) => unreachable!("validated on load"),
    }
}

/// SQL that is true on an account the playerbots made for themselves: it carries the flag, or, in a
/// game without one, its name starts with the bots' prefix.
fn is_bot(spec: &AccountsSpec) -> String {
    match &spec.bot_column {
        Some(column) => format!("IFNULL(`{column}`, 0) <> 0"),
        None => format!("IFNULL(LEFT(`{}`, {}) = '{}', 0) <> 0", spec.name_column, spec.bot_prefix.len(), spec.bot_prefix),
    }
}

fn literal(text: &str) -> String {
    format!("'{}'", sql_quote(text))
}

/// Everyone who can log in, and how many accounts the bots own (which are not listed).
fn list_sql(spec: &AccountsSpec) -> String {
    let name = format!("`{}`", spec.name_column);
    format!(
        "SELECT 'A', `{name_col}`, {gm}, IFNULL(UNIX_TIMESTAMP(`{created}`), 0), IFNULL(UNIX_TIMESTAMP(`{login}`), 0), {same} \
         FROM {table} WHERE NOT ({bot}) ORDER BY `{id}`;\n\
         SELECT 'B', COUNT(*) FROM {table} WHERE {bot};\n",
        name_col = spec.name_column,
        gm = gm_expression(spec),
        created = spec.created_column,
        login = spec.last_login_column,
        same = password_cell(spec, &name, &name),
        bot = is_bot(spec),
        table = table(spec),
        id = spec.id_column,
    )
}

/// What is stored for one account: nothing, a name whose password is not the one given, or a match.
fn probe_sql(spec: &AccountsSpec, name: &str, password: &str) -> String {
    format!(
        "SELECT {}, {} FROM {} WHERE `{}` = {};\n",
        password_cell(spec, &literal(name), &literal(password)),
        gm_expression(spec),
        table(spec),
        spec.name_column,
        literal(name)
    )
}

/// The one account row a probe found.
#[derive(Debug, PartialEq, Eq)]
pub struct Probe {
    pub password_matches: bool,
    pub gm_level: u8,
}

/// Reads what [`probe_sql`] printed, for the account `name` and the password that was tried.
pub fn parse_probe(spec: &AccountsSpec, text: &str, name: &str, password: &str) -> Option<Probe> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut cells = line.split('\t');
    let matches = cell_matches(spec, cells.next()?, name, password);
    let gm_level = cells.next()?.trim().parse().ok()?;
    Some(Probe { password_matches: matches, gm_level })
}

// ---------------------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    pub gm_level: u8,
    /// Seconds since the epoch; `None` when the database has no date.
    pub created: Option<u64>,
    pub last_login: Option<u64>,
    /// The password is the account's own name, which anyone who has read the emulator's notes
    /// knows. The upstream database ships accounts like this.
    pub default_password: bool,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum AccountsState {
    /// Docker or the database could not be asked.
    Unknown,
    /// The database is not there or not running, so there is nothing to list yet.
    Blocked,
    Ready,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AccountsStatus {
    pub state: AccountsState,
    pub note: Option<String>,
    pub accounts: Vec<Account>,
    /// Accounts the playerbots made for themselves; they are not listed.
    pub bot_accounts: u32,
    /// What the form may offer: the names bots use are not available, and the limits of the game.
    pub bot_prefix: String,
    pub max_gm_level: u8,
    pub name_max: usize,
    pub password_max: usize,
}

fn blank(spec: &AccountsSpec, state: AccountsState, note: Option<String>) -> AccountsStatus {
    AccountsStatus {
        state,
        note,
        accounts: Vec::new(),
        bot_accounts: 0,
        bot_prefix: spec.bot_prefix.clone(),
        max_gm_level: spec.max_gm_level,
        name_max: spec.name_max,
        password_max: spec.password_max,
    }
}

/// Reads what [`list_sql`] printed.
pub fn parse_accounts(spec: &AccountsSpec, text: &str) -> (Vec<Account>, u32) {
    let mut accounts = Vec::new();
    let mut bots = 0;
    for line in text.lines() {
        let cells: Vec<&str> = line.split('\t').collect();
        match cells.as_slice() {
            ["A", name, gm, created, login, default] => {
                let date = |cell: &str| cell.parse::<u64>().ok().filter(|t| *t > 0);
                accounts.push(Account {
                    name: name.to_string(),
                    gm_level: gm.parse().unwrap_or(0),
                    created: date(created),
                    last_login: date(login),
                    default_password: cell_matches(spec, default, name, name),
                });
            }
            ["B", count] => bots = count.parse().unwrap_or(0),
            _ => {}
        }
    }
    (accounts, bots)
}

/// Reads the accounts from the database; starts and changes nothing.
pub async fn status(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> AccountsStatus {
    let spec = &recipe.accounts;
    if let Err(reason) = builder::engine().await {
        return blank(spec, AccountsState::Unknown, Some(reason));
    }
    let creds = match Credentials::load(paths) {
        Ok(Some(creds)) => creds,
        Ok(None) => return blank(spec, AccountsState::Blocked, Some("Set up the database first.".to_string())),
        Err(e) => return blank(spec, AccountsState::Unknown, Some(e)),
    };
    if !database::is_healthy(instance).await {
        return blank(spec, AccountsState::Blocked, Some("The database is not running. Start the servers to see the accounts.".to_string()));
    }
    match database::query(instance, &creds, &list_sql(spec)).await {
        Ok(text) => {
            let (accounts, bot_accounts) = parse_accounts(spec, &text);
            AccountsStatus { accounts, bot_accounts, ..blank(spec, AccountsState::Ready, None) }
        }
        Err(e) => blank(spec, AccountsState::Unknown, Some(format!("The accounts could not be read: {}", e.trim()))),
    }
}

// ---------------------------------------------------------------------------------------
// Changing accounts
// ---------------------------------------------------------------------------------------

/// The server program that has the console, which is also the one that runs the world and its bots.
pub(crate) fn console_service(recipe: &Recipe) -> Result<&ServiceSpec, String> {
    recipe.run.services.iter().find(|s| s.console).ok_or_else(|| "This server has no console.".to_string())
}

/// Everything a change needs before the console is touched: the world server is running, the
/// database answers, and the passwords for it are known and hidden from the log.
async fn prepare(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(Credentials, String), String> {
    task.step("Checking that the world server is running");
    let service = console_service(recipe)?;
    builder::engine().await?;
    let creds = Credentials::load(paths)?.ok_or_else(|| "Set up the database first.".to_string())?;
    for secret in creds.secrets() {
        task.add_secret(secret);
    }
    let server = runtime::status(paths, instance, recipe).await;
    let world = server.services.iter().find(|s| s.id == service.id);
    if world.map(|s| s.state) == Some(ServiceState::Paused) {
        return Err(format!("The servers are paused. Resume them first: accounts are managed through the {}'s console.", service.label.to_lowercase()));
    }
    if world.map(|s| s.state) != Some(ServiceState::Running) {
        return Err(format!("Start the servers first: accounts are managed through the {}'s console.", service.label.to_lowercase()));
    }
    if !database::is_healthy(instance).await {
        return Err("The database is not answering. Restart the servers.".to_string());
    }
    Ok((creds, crate::compose::service_container(instance, &service.id)))
}

/// Types one line into the container's console: the standard input of its main process. The line
/// goes in on the input of `docker exec`, so it is never an argument of any program.
async fn send_line(container: &str, line: &str) -> Result<(), String> {
    let spec = CommandSpec::new("docker", &["exec", "-i", container, "sh", "-c", "cat > /proc/1/fd/0"])
        .stdin(format!("{line}\n").into_bytes())
        .timeout(Duration::from_secs(20));
    let out = capture(&spec).await;
    if out.success {
        Ok(())
    } else {
        Err("The world server's console could not be reached.".to_string())
    }
}

/// The server's own reply to a command, from its log, so that a refusal is explained in its words.
/// The console's prompt is not a reply.
async fn reply_since(container: &str, since: u64, prompt: &str) -> Vec<String> {
    let text = runtime::log_since(container, since).await;
    let lines: Vec<String> = text.lines().filter_map(|l| clean_log_line_to(l, 160)).filter(|l| l != prompt).collect();
    let skip = lines.len().saturating_sub(3);
    lines[skip..].to_vec()
}

/// Polls until `done` accepts what the database holds, or the wait runs out.
async fn wait_for(
    task: &mut Task,
    instance: &str,
    spec: &AccountsSpec,
    creds: &Credentials,
    account: (&str, &str),
    done: impl Fn(Option<&Probe>) -> bool,
) -> Result<Option<Probe>, String> {
    let deadline = Instant::now() + CONSOLE_WAIT;
    let sql = probe_sql(spec, account.0, account.1);
    loop {
        let text = database::query(instance, creds, &sql).await.map_err(|e| task.redact(e.trim()))?;
        let seen = parse_probe(spec, &text, account.0, account.1);
        if done(seen.as_ref()) || Instant::now() > deadline {
            return Ok(seen);
        }
        task.pause(POLL_EVERY).await?;
    }
}

async fn explain_refusal(task: &Task, container: &str, since: u64, prompt: &str, what: &str) -> String {
    let reply = reply_since(container, since, prompt).await;
    if reply.is_empty() {
        format!("{what} The world server printed nothing about it.")
    } else {
        task.redact(&format!("{what} The world server said: {}", reply.join(" / ")))
    }
}

/// Creates an account through the console and confirms it in the database.
pub async fn create(
    task: &mut Task,
    paths: &InstancePaths,
    instance: &str,
    recipe: &Recipe,
    name: &str,
    password: &str,
    level: u8,
) -> Result<(), String> {
    let spec = &recipe.accounts;
    let name = check_name(spec, name)?;
    let (password, is_name) = password_for_new_account(spec, &name, password)?;
    let password = password.as_str();
    check_level(spec, level)?;
    if !is_name {
        task.add_secret(password);
    }
    let (creds, container) = prepare(task, paths, instance, recipe).await?;
    let prompt = console_service(recipe)?.console_prompt.clone().unwrap_or_default();

    task.step("Checking that the name is free");
    let existing = database::query(instance, &creds, &probe_sql(spec, &name, password)).await.map_err(|e| task.redact(e.trim()))?;
    if parse_probe(spec, &existing, &name, password).is_some() {
        return Err(format!("An account named {name} already exists."));
    }

    task.step("Asking the world server to create the account");
    let since = clock::now().saturating_sub(1);
    send_line(&container, &fill_command(&spec.create_command, &name, password, 0)).await?;
    let seen = wait_for(task, instance, spec, &creds, (&name, password), |p| p.is_some_and(|p| p.password_matches)).await?;
    match seen {
        None => return Err(explain_refusal(task, &container, since, &prompt, &format!("The account {name} was not created.")).await),
        Some(p) if !p.password_matches => {
            return Err(format!("The account {name} exists, but its stored password is not the one given. Change the password and check it."));
        }
        Some(_) if is_name => task.info(&format!("Created the account {name}, with its name as the password")),
        Some(_) => task.info(&format!("Created the account {name}")),
    }

    if level > 0 {
        task.step("Setting the game-master level");
        let since = clock::now().saturating_sub(1);
        send_line(&container, &fill_command(&spec.gm_command, &name, "", level)).await?;
        let seen = wait_for(task, instance, spec, &creds, (&name, password), |p| p.is_some_and(|p| p.gm_level == level)).await?;
        if seen.map(|p| p.gm_level) != Some(level) {
            return Err(explain_refusal(task, &container, since, &prompt, &format!("The account {name} was created, but its game-master level was not set.")).await);
        }
        task.info(&format!("Game-master level {level} set"));
    }
    Ok(())
}

/// Changes an account's password through the console and confirms it in the database.
pub async fn change_password(
    task: &mut Task,
    paths: &InstancePaths,
    instance: &str,
    recipe: &Recipe,
    name: &str,
    password: &str,
) -> Result<(), String> {
    let spec = &recipe.accounts;
    let name = check_name(spec, name)?;
    check_password(spec, password)?;
    task.add_secret(password);
    let (creds, container) = prepare(task, paths, instance, recipe).await?;
    let prompt = console_service(recipe)?.console_prompt.clone().unwrap_or_default();

    task.step("Checking the account");
    let sql = probe_sql(spec, &name, password);
    let existing = database::query(instance, &creds, &sql).await.map_err(|e| task.redact(e.trim()))?;
    match parse_probe(spec, &existing, &name, password) {
        None => return Err(format!("There is no account named {name}.")),
        Some(p) if p.password_matches => return Err("That is already the password.".to_string()),
        Some(_) => {}
    }

    task.step("Asking the world server to change the password");
    let since = clock::now().saturating_sub(1);
    send_line(&container, &fill_command(&spec.password_command, &name, password, 0)).await?;
    let seen = wait_for(task, instance, spec, &creds, (&name, password), |p| p.is_some_and(|p| p.password_matches)).await?;
    if !seen.is_some_and(|p| p.password_matches) {
        return Err(explain_refusal(task, &container, since, &prompt, &format!("The password of {name} was not changed.")).await);
    }
    task.info(&format!("Changed the password of {name}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AccountsSpec {
        crate::recipe::builtin().unwrap().remove(0).accounts
    }

    fn acore() -> AccountsSpec {
        crate::recipe::builtin().unwrap().into_iter().find(|r| r.id == "azerothcore").unwrap().accounts
    }

    #[test]
    fn names_are_letters_and_digits_stored_in_upper_case() {
        let spec = spec();
        assert_eq!(check_name(&spec, "Paul01").unwrap(), "PAUL01");
        assert!(check_name(&spec, "").unwrap_err().contains("Enter"));
        assert!(check_name(&spec, "a b").unwrap_err().contains("letters"));
        assert!(check_name(&spec, "x;DROP").unwrap_err().contains("letters"));
        assert!(check_name(&spec, "élan").unwrap_err().contains("letters"));
        assert!(check_name(&spec, &"a".repeat(17)).unwrap_err().contains("at most 16"));
        assert!(check_name(&spec, &"a".repeat(16)).is_ok());
    }

    #[test]
    fn the_names_the_bots_use_are_not_available() {
        let spec = spec();
        assert!(check_name(&spec, "rndbot77").unwrap_err().contains("playerbots"));
        assert!(check_name(&spec, "RNDBOT").unwrap_err().contains("playerbots"));
        assert!(check_name(&spec, "RndBotFan").is_err());
        assert!(check_name(&spec, "robot").is_ok());
    }

    #[test]
    fn passwords_keep_out_of_the_console_and_template_syntax() {
        let spec = spec();
        assert!(check_password(&spec, "Str0ng!pw").is_ok());
        assert!(check_password(&spec, "x").is_ok(), "there is no minimum length");
        assert!(check_password(&spec, "").unwrap_err().contains("Enter a password"));
        assert!(check_password(&spec, &"p".repeat(17)).unwrap_err().contains("at most 16"));
        for bad in ["has space1", "quote\"ed1", "it's-one", "back\\slash", "brace{1}xx", "tick`xx1", "tab\tulator", "línea123"] {
            assert!(check_password(&spec, bad).unwrap_err().contains("letters, digits"), "{bad}");
        }
    }

    #[test]
    fn a_new_account_may_be_made_without_a_password_and_then_uses_its_name() {
        let spec = spec();
        assert_eq!(password_for_new_account(&spec, "PAUL", "").unwrap(), ("PAUL".to_string(), true));
        assert_eq!(password_for_new_account(&spec, "PAUL", "123123").unwrap(), ("123123".to_string(), false));
        assert_eq!(password_for_new_account(&spec, "PAUL", "1").unwrap(), ("1".to_string(), false));
        assert!(password_for_new_account(&spec, "PAUL", "bad pw").unwrap_err().contains("letters, digits"), "a typed password is still checked");
        let long = "A".repeat(spec.password_max + 1);
        assert!(password_for_new_account(&spec, &long, "").unwrap_err().contains("too long"));
    }

    #[test]
    fn the_game_master_level_stops_at_the_recipes_highest() {
        let spec = spec();
        assert!(check_level(&spec, 0).is_ok() && check_level(&spec, 3).is_ok());
        assert!(check_level(&spec, 4).is_err());
    }

    #[test]
    fn a_console_line_is_filled_in_one_pass() {
        assert_eq!(fill_command("account create {name} {password}", "PAUL", "pw{level}x", 0), "account create PAUL pw{level}x");
        assert_eq!(fill_command("account set gmlevel {name} {level}", "PAUL", "", 3), "account set gmlevel PAUL 3");
        assert_eq!(fill_command("account set password {name} {password} {password}", "A", "b", 0), "account set password A b b");
        assert_eq!(fill_command("plain {unknown} text", "A", "b", 0), "plain {unknown} text");
    }

    #[test]
    fn a_real_recipe_produces_the_upstream_console_commands() {
        let spec = spec();
        assert_eq!(fill_command(&spec.create_command, "ANN", "secret1", 0), "account create ANN secret1");
        assert_eq!(fill_command(&spec.password_command, "ANN", "secret2", 0), "account set password ANN secret2 secret2");
        assert_eq!(fill_command(&spec.gm_command, "ANN", "", 2), "account set gmlevel ANN 2");
    }

    #[test]
    fn the_sql_quotes_what_a_person_typed_and_checks_the_hash_in_the_database() {
        let spec = spec();
        let sql = probe_sql(&spec, "PAUL", "it's;--");
        assert!(sql.contains("'it''s;--'"), "{sql}");
        assert!(sql.contains("SHA1(CONCAT(UPPER('PAUL'), ':', UPPER('it''s;--')))"), "{sql}");
        assert!(sql.contains("FROM `realmd`.`account` WHERE `username` = 'PAUL'"), "{sql}");
        let list = list_sql(&spec);
        assert!(list.contains("WHERE NOT (IFNULL(`playerBot`, 0) <> 0)"), "{list}");
        assert!(list.contains("SHA1(CONCAT(UPPER(`username`), ':', UPPER(`username`)))"), "{list}");
        assert!(list.contains("SELECT 'B', COUNT(*) FROM `realmd`.`account` WHERE IFNULL(`playerBot`, 0) <> 0;"), "{list}");
    }

    #[test]
    fn a_probe_tells_a_missing_account_from_a_wrong_password_from_a_match() {
        let spec = spec();
        let probe = |text: &str| parse_probe(&spec, text, "A", "b");
        assert_eq!(probe(""), None);
        assert_eq!(probe("\n"), None);
        assert_eq!(probe("0\t0\n"), Some(Probe { password_matches: false, gm_level: 0 }));
        assert_eq!(probe("1\t3\n"), Some(Probe { password_matches: true, gm_level: 3 }));
        assert_eq!(probe("1\tx\n"), None);
    }

    #[test]
    fn the_account_list_is_read_with_dates_defaults_and_the_bot_count() {
        let text = "A\tADMINISTRATOR\t3\t1609507136\t0\t1\nA\tPAUL\t0\t1790670072\t1790670100\t0\nB\t16\n";
        let (accounts, bots) = parse_accounts(&spec(), text);
        assert_eq!(bots, 16);
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0], Account { name: "ADMINISTRATOR".into(), gm_level: 3, created: Some(1_609_507_136), last_login: None, default_password: true });
        assert!(!accounts[1].default_password && accounts[1].last_login == Some(1_790_670_100));
        assert_eq!(parse_accounts(&spec(), ""), (Vec::new(), 0));
    }

    /// The verifier is a 32-byte number that changes with the name, the password (case does not
    /// matter) and the salt. That it is the number the game stores is settled against a real
    /// account the server made, when the app checks one.
    #[test]
    fn an_srp6_verifier_follows_the_name_the_password_and_the_salt() {
        let salt = [7u8; 32];
        let v = srp6_verifier("Ann", "Secret1", &salt);
        assert_eq!(v, srp6_verifier("ANN", "SECRET1", &salt), "the server folds both to upper case");
        assert_ne!(v, srp6_verifier("ANN", "SECRET2", &salt));
        assert_ne!(v, srp6_verifier("BOB", "SECRET1", &salt));
        assert_ne!(v, srp6_verifier("ANN", "SECRET1", &[8u8; 32]));
        let n = BigUint::parse_bytes(SRP6_N.as_bytes(), 16).unwrap();
        assert!(BigUint::from_bytes_le(&v) < n, "a residue modulo N");
        assert_eq!(v, srp6_verifier("ANN", "SECRET1", &salt), "and repeatable");
    }

    #[test]
    fn the_prime_of_the_group_is_the_one_the_game_uses() {
        let n = BigUint::parse_bytes(SRP6_N.as_bytes(), 16).unwrap();
        assert_eq!(n.bits(), 256);
        assert_eq!(SRP6_G, 7);
    }

    #[test]
    fn hexadecimal_from_the_database_is_read_exactly_or_refused() {
        assert_eq!(from_hex("00FFa1"), Some(vec![0, 255, 161]));
        assert_eq!(from_hex(""), Some(vec![]));
        assert_eq!(from_hex("0"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("é1"), None);
    }

    #[test]
    fn a_game_with_salted_passwords_is_checked_in_the_app_not_the_database() {
        let spec = acore();
        let salt = [3u8; 32];
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02X}")).collect::<String>();
        let stored = format!("{}:{}", hex(&salt), hex(&srp6_verifier("ANN", "secret1", &salt)));
        assert!(cell_matches(&spec, &stored, "ANN", "secret1"));
        assert!(cell_matches(&spec, &format!("{stored}\n"), "ANN", "SECRET1"));
        assert!(!cell_matches(&spec, &stored, "ANN", "secret2"));
        assert!(!cell_matches(&spec, "no colon", "ANN", "secret1"));
        assert!(!cell_matches(&spec, "12:34", "ANN", "secret1"));
        let text = format!("{stored}\t2\n");
        assert_eq!(parse_probe(&spec, &text, "ANN", "secret1"), Some(Probe { password_matches: true, gm_level: 2 }));
        assert_eq!(parse_probe(&spec, &text, "ANN", "other"), Some(Probe { password_matches: false, gm_level: 2 }));
        let listed = format!("A\tANN\t0\t0\t0\t{}:{}\n", hex(&salt), hex(&srp6_verifier("ANN", "ANN", &salt)));
        assert!(parse_accounts(&spec, &listed).0[0].default_password, "the account whose password is its name is flagged");
    }

    #[test]
    fn a_game_that_keeps_levels_apart_and_flags_no_bots_asks_for_them_differently() {
        let spec = acore();
        let sql = probe_sql(&spec, "ANN", "pw");
        assert!(sql.contains("CONCAT(HEX(`salt`), ':', HEX(`verifier`))"), "{sql}");
        assert!(
            sql.contains("IFNULL((SELECT MAX(g.`gmlevel`) FROM `acore_auth`.`account_access` g WHERE g.`id` = `acore_auth`.`account`.`id`), 0)"),
            "{sql}"
        );
        assert!(!sql.contains("'pw'"), "the password is never sent to the database when the app checks it");
        let list = list_sql(&spec);
        assert!(list.contains("WHERE NOT (IFNULL(LEFT(`username`, 6) = 'RNDBOT', 0) <> 0) ORDER BY `id`"), "{list}");
        assert!(list.contains("WHERE IFNULL(LEFT(`username`, 6) = 'RNDBOT', 0) <> 0;"), "{list}");
    }

    #[test]
    fn the_console_commands_of_the_second_game_carry_the_realm() {
        let spec = acore();
        assert_eq!(fill_command(&spec.create_command, "ANN", "secret1", 0), "account create ANN secret1");
        assert_eq!(fill_command(&spec.gm_command, "ANN", "", 3), "account set gmlevel ANN 3 -1");
    }
}
