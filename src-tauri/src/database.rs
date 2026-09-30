//! The database (Step 4): a MariaDB container, the game's schemas loaded from the emulator's own
//! SQL repository, brought up to date, with an account the game servers use.
//!
//! Everything runs inside the database container, which sees the SQL repository read-only, so
//! large files are never streamed through the app. State is read from the database itself: the
//! tables that exist, the `db_version` rows the update scripts write, and a small ledger of what
//! this app has done, kept in a schema of its own. Nothing is decided from a separate flag file.
//!
//! Safety rules the steps follow:
//! - A schema is only ever (re)created by this app when its ledger says it started that setup and
//!   never finished it. A schema that already holds tables this app did not create is refused.
//! - Data files load once each and are recorded, so an interrupted load resumes and a finished
//!   one is never repeated (their base `db_version` would undo applied updates).
//! - A database that already holds data is backed up before any update script touches it.
//! - Updates are trusted only once their own row appears in `db_version`; the scripts exit 0
//!   even when they fail.
//!
//! Some emulators bring their own importer (a program built with the server that creates the
//! tables and applies the updates). For those the app creates the empty schemas, backs up what is
//! there, runs the importer in a throwaway container with the server's own configuration, and
//! checks the tables afterwards. It never repeats a finished import: the ledger records which
//! build it ran for, and a rebuilt server is the only reason to run it again.

use crate::{
    backups, builder,
    compose::{self, DATABASE_ENV_FILE, SQL_MOUNT},
    docker::{self, ContainerState},
    extractor::failure_detail,
    config,
    paths::InstancePaths,
    process::{capture, probe, CommandSpec, Probe},
    recipe::{DatabaseEngine, DatabaseMethod, Recipe, VERSION_TABLE},
    tasks::{Task, CANCELLED},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

/// The schema this app keeps its own notes in.
pub(crate) const LEDGER_SCHEMA: &str = "worldstone";
const LEDGER_TABLE: &str = "ledger";
pub(crate) const CREDENTIALS_FILE: &str = "database.json";
/// The client used for every statement; the password arrives through `MYSQL_PWD`, never an argument.
/// Its character set only has to be wide enough for whatever the schemas hold.
fn client_args(engine: DatabaseEngine) -> [&'static str; 5] {
    [engine.client(), "-uroot", "--batch", "--skip-column-names", "--default-character-set=utf8mb4"]
}

/// Ledger entries for an import tool run: one when it starts (it stays, so a run that never
/// finished is recognised) and one for each build it finished for.
const IMPORT_STARTED: &str = "import:started";
const IMPORT_DONE: &str = "import:done:";

// ---------------------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------------------

/// Passwords generated for one instance, kept in its `Config` folder. They are created once and
/// never replaced: the database volume holds the root password from the first start on.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub root_password: String,
    pub user: String,
    pub user_password: String,
    /// The server the database container runs, so every statement uses that server's programs.
    /// Files from before this was recorded are MariaDB's.
    #[serde(default)]
    pub engine: DatabaseEngine,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credentials {{ user: {:?}, passwords: *** }}", self.user)
    }
}

fn new_secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

impl Credentials {
    pub fn load(paths: &InstancePaths) -> Result<Option<Self>, String> {
        let path = paths.config.join(CREDENTIALS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| format!("{} cannot be read ({e}). It holds the database passwords; restore it from a backup.", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
        }
    }

    /// The saved credentials, or new ones when there are none. Saved passwords are never replaced.
    pub fn load_or_create(paths: &InstancePaths, user: &str, engine: DatabaseEngine) -> Result<Self, String> {
        std::fs::create_dir_all(&paths.config).map_err(|e| format!("Cannot create {}: {e}", paths.config.display()))?;
        let mut creds = match Self::load(paths)? {
            Some(saved) => saved,
            None => Self { root_password: new_secret(), user: user.to_string(), user_password: new_secret(), engine },
        };
        creds.user = user.to_string();
        creds.engine = engine;
        let json = serde_json::to_string_pretty(&creds).map_err(|e| e.to_string())?;
        write_if_changed(&paths.config.join(CREDENTIALS_FILE), &json)?;
        // What the database container reads on its first start. Root may only connect from
        // inside the container; the game servers use their own account.
        let (password_var, host_var) = engine.root_env();
        let env = format!("{password_var}={}\n{host_var}=localhost\n", creds.root_password);
        write_if_changed(&paths.config.join(DATABASE_ENV_FILE), &env)?;
        Ok(creds)
    }

    pub fn secrets(&self) -> [&str; 2] {
        [&self.root_password, &self.user_password]
    }
}

fn write_if_changed(path: &std::path::Path, text: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// What the SQL repository offers
// ---------------------------------------------------------------------------------------

/// One update script and the `db_version` row it writes when it succeeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub file: String,
    pub folder: String,
    pub version: (u32, u32, u32),
}

/// `Rel22_04_030_Some_Name.sql` writes the row version 22, structure 4, content 30.
pub fn parse_update_name(file: &str) -> Option<(u32, u32, u32)> {
    let mut parts = file.strip_suffix(".sql")?.split('_');
    let release = parts.next()?.strip_prefix("Rel")?.parse().ok()?;
    let structure = parts.next()?.parse().ok()?;
    let content = parts.next()?.parse().ok()?;
    Some((release, structure, content))
}

#[derive(Clone, Debug)]
pub struct SchemaPlan {
    pub name: String,
    /// Empty for a schema the import tool loads.
    pub dir: String,
    pub setup: String,
    /// Created and granted by the app, filled by the server itself: nothing is expected in it yet.
    pub self_managed: bool,
    pub populate_dir: Option<String>,
    /// Data files, in the order they load.
    pub populate: Vec<String>,
    /// Update scripts, in the order they must run.
    pub updates: Vec<Update>,
    pub required_tables: Vec<String>,
}

impl SchemaPlan {
    fn setup_path(&self) -> String {
        format!("{SQL_MOUNT}/{}/Setup/{}", self.dir, self.setup)
    }

    fn populate_path(&self) -> Option<String> {
        self.populate_dir.as_ref().map(|d| format!("{SQL_MOUNT}/{}/Setup/{d}", self.dir))
    }

    fn update_path(&self, folder: &str) -> String {
        format!("{SQL_MOUNT}/{}/Updates/{folder}", self.dir)
    }

    fn key(&self, what: &str) -> String {
        format!("{}:{what}", self.name)
    }
}

/// The emulator's own importer, as it is started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportPlan {
    /// The server whose image holds the tool.
    pub service: String,
    /// The program, inside that image.
    pub tool: String,
    pub args: Vec<String>,
    /// The build the tool comes from (the image's fingerprint). Unknown until the image is read.
    pub build: Option<String>,
}

/// Everything that will be done to the databases: the scripts in the SQL repository, read from the
/// repository itself, or the import tool to run.
#[derive(Clone, Debug)]
pub struct Plan {
    pub schemas: Vec<SchemaPlan>,
    /// Path of the script that adds the realm entry, inside the container. Left out when the
    /// emulator's own data already holds the entry.
    pub realm_script: Option<String>,
    pub import: Option<ImportPlan>,
}

/// File names end up in shell arguments and ledger keys, so only plain ones are accepted.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

fn sql_files(dir: &std::path::Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("Cannot read {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".sql") || !entry.path().is_file() {
            continue;
        }
        if !plain_name(&name) {
            return Err(format!("{} holds a script with an unexpected name: {name}", dir.display()));
        }
        files.push(name);
    }
    files.sort();
    Ok(files)
}

impl Plan {
    pub fn read(paths: &InstancePaths, recipe: &Recipe) -> Result<Plan, String> {
        match recipe.database.method {
            DatabaseMethod::Scripts => Self::read_scripts(paths, recipe),
            DatabaseMethod::ImportTool => Ok(Self::import_tool(recipe)),
        }
    }

    /// The import tool loads everything, so there is nothing to read from disk; what the plan lists
    /// is what must exist afterwards.
    fn import_tool(recipe: &Recipe) -> Plan {
        let spec = recipe.database.import.as_ref().expect("validated on load");
        let file = recipe.config.files.iter().find(|f| f.id == spec.config).expect("validated on load");
        let config = format!("{}/{}", recipe.run.config_mount, file.stored_at());
        let schemas = recipe
            .database
            .schemas
            .iter()
            .map(|s| SchemaPlan {
                name: s.name.clone(),
                dir: String::new(),
                setup: String::new(),
                self_managed: s.self_managed,
                populate_dir: None,
                populate: Vec::new(),
                updates: Vec::new(),
                required_tables: s.required_tables.clone(),
            })
            .collect();
        let import = ImportPlan {
            service: spec.service.clone(),
            tool: format!("{}/{}", recipe.build.install_prefix, spec.tool),
            args: spec.args.iter().map(|a| a.replace("{config}", &config)).collect(),
            build: None,
        };
        Plan { schemas, realm_script: None, import: Some(import) }
    }

    fn read_scripts(paths: &InstancePaths, recipe: &Recipe) -> Result<Plan, String> {
        let repo = recipe.database_repo();
        let root = paths.repos.join(recipe.repo_folder(repo));
        if !root.is_dir() {
            return Err("Download the source first: the database scripts come from its repositories.".to_string());
        }
        let lacks = |what: &str| {
            format!("The {} repository has no {what}. Update the source (part of it lives in a submodule).", repo.id)
        };
        let mut schemas = Vec::new();
        for spec in &recipe.database.schemas {
            let (dir, setup) = (spec.dir.as_deref().expect("validated on load"), spec.setup.as_deref().expect("validated on load"));
            let base: PathBuf = root.join(dir);
            if !base.join("Setup").join(setup).is_file() {
                return Err(lacks(&format!("{dir}/Setup/{setup}")));
            }
            let populate = match &spec.populate {
                None => Vec::new(),
                Some(folder) => {
                    let files = sql_files(&base.join("Setup").join(folder)).map_err(|_| lacks(&format!("{dir}/Setup/{folder}")))?;
                    if files.is_empty() {
                        return Err(lacks(&format!("data files in {dir}/Setup/{folder}")));
                    }
                    files
                }
            };
            let mut updates = Vec::new();
            for folder in &spec.updates {
                let folder_path = base.join("Updates").join(folder);
                if !folder_path.is_dir() {
                    continue;
                }
                for file in sql_files(&folder_path)? {
                    let version = parse_update_name(&file)
                        .ok_or_else(|| format!("{dir}/Updates/{folder} holds a script whose name does not give its version: {file}"))?;
                    updates.push(Update { file, folder: folder.clone(), version });
                }
            }
            updates.sort_by(|a, b| a.version.cmp(&b.version).then_with(|| a.file.cmp(&b.file)));
            schemas.push(SchemaPlan {
                name: spec.name.clone(),
                dir: dir.to_string(),
                setup: setup.to_string(),
                self_managed: spec.self_managed,
                populate_dir: spec.populate.clone(),
                populate,
                updates,
                required_tables: spec.required_tables.clone(),
            });
        }
        let sql = recipe.database.realm.sql.as_deref().expect("validated on load");
        if !root.join(sql).is_file() {
            return Err(lacks(sql));
        }
        Ok(Plan { schemas, realm_script: Some(format!("{SQL_MOUNT}/{sql}")), import: None })
    }
}

/// [`Plan::read`], and for an import tool the build it comes from, which decides whether it is due.
async fn read_plan(instance: &str, paths: &InstancePaths, recipe: &Recipe) -> Result<Plan, String> {
    let mut plan = Plan::read(paths, recipe)?;
    if let Some(import) = &mut plan.import {
        let service = recipe.run.services.iter().find(|s| s.id == import.service).expect("validated on load");
        let image = builder::image_ref(instance, recipe, service);
        match builder::inspect_image(&image).await {
            Some(info) if !info.fingerprint.is_empty() => import.build = Some(info.fingerprint),
            Some(_) => return Err(format!("The server image {image} does not say which build it is. Build the server again.")),
            None => return Err("Build the server first: the database importer comes with it.".to_string()),
        }
    }
    Ok(plan)
}

// ---------------------------------------------------------------------------------------
// What the database holds
// ---------------------------------------------------------------------------------------

#[derive(Default, Debug)]
struct Facts {
    schemas: HashSet<String>,
    tables: HashMap<String, BTreeSet<String>>,
    ledger: HashSet<String>,
    versions: HashMap<String, BTreeSet<(u32, u32, u32)>>,
    realms: Option<u32>,
}

/// Reads the tab-separated lines the queries below print. Lines that are not one of ours (a
/// client warning, say) are ignored.
fn parse_facts(text: &str) -> Facts {
    let mut facts = Facts::default();
    for line in text.lines() {
        let f: Vec<&str> = line.trim_end().split('\t').collect();
        match f.as_slice() {
            ["S", schema] => {
                facts.schemas.insert(schema.to_string());
            }
            ["T", schema, table] => {
                facts.tables.entry(schema.to_string()).or_default().insert(table.to_string());
            }
            ["L", name] => {
                facts.ledger.insert(name.to_string());
            }
            ["V", schema, v, s, c] => {
                if let (Ok(v), Ok(s), Ok(c)) = (v.parse(), s.parse(), c.parse()) {
                    facts.versions.entry(schema.to_string()).or_default().insert((v, s, c));
                }
            }
            ["R", count] => facts.realms = count.parse().ok(),
            _ => {}
        }
    }
    facts
}

fn quoted_schemas(plan: &Plan) -> String {
    plan.schemas
        .iter()
        .map(|s| s.name.as_str())
        .chain([LEDGER_SCHEMA])
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(",")
}

fn overview_sql(plan: &Plan) -> String {
    let list = quoted_schemas(plan);
    format!(
        "SELECT 'S', schema_name FROM information_schema.schemata WHERE schema_name IN ({list});\n\
         SELECT 'T', table_schema, table_name FROM information_schema.tables WHERE table_schema IN ({list});\n"
    )
}

/// The queries that need a table to exist, built from what the overview found.
fn details_sql(plan: &Plan, recipe: &Recipe, seen: &Facts) -> String {
    let has = |schema: &str, table: &str| seen.tables.get(schema).is_some_and(|t| t.contains(table));
    let mut sql = String::new();
    for schema in &plan.schemas {
        if has(&schema.name, VERSION_TABLE) {
            let n = &schema.name;
            sql += &format!("SELECT 'V', '{n}', version, structure, content FROM `{n}`.`{VERSION_TABLE}`;\n");
        }
    }
    if has(LEDGER_SCHEMA, LEDGER_TABLE) {
        sql += &format!("SELECT 'L', name FROM `{LEDGER_SCHEMA}`.`{LEDGER_TABLE}`;\n");
    }
    let realm = &recipe.database.realm;
    if has(&realm.schema, &realm.table) {
        sql += &format!("SELECT 'R', COUNT(*) FROM `{}`.`{}`;\n", realm.schema, realm.table);
    }
    sql
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaStatus {
    pub name: String,
    pub exists: bool,
    pub tables: u32,
    /// The table setup ran to the end.
    pub setup_done: bool,
    /// Holds tables this app did not create, so it will not touch them.
    pub unmanaged: bool,
    pub populate_total: u32,
    pub populate_pending: u32,
    /// The newest `db_version` row, such as `22.6.4`.
    pub version: Option<String>,
    pub updates_total: u32,
    pub updates_pending: u32,
    pub missing_tables: Vec<String>,
}

/// What the ledger says about the import tool's runs.
struct ImportLedger {
    /// A run started at some point.
    started: bool,
    /// Some run finished (for this build or an earlier one).
    finished: bool,
    /// The run for the current build finished.
    current: bool,
}

impl ImportLedger {
    fn read(plan: &Plan, facts: &Facts) -> Option<ImportLedger> {
        let import = plan.import.as_ref()?;
        Some(ImportLedger {
            started: facts.ledger.contains(IMPORT_STARTED),
            finished: facts.ledger.iter().any(|k| k.starts_with(IMPORT_DONE)),
            current: import.build.as_ref().is_some_and(|b| facts.ledger.contains(&format!("{IMPORT_DONE}{b}"))),
        })
    }
}

fn classify(plan: &Plan, facts: &Facts) -> Vec<SchemaStatus> {
    let import = ImportLedger::read(plan, facts);
    plan.schemas
        .iter()
        .map(|schema| {
            let tables = facts.tables.get(&schema.name);
            let table_count = tables.map_or(0, BTreeSet::len);
            let versions = facts.versions.get(&schema.name);
            // The import tool loads all its schemas at once; a schema the server fills itself is
            // never expected to hold anything yet.
            let (setup_done, started, updates_total, updates_pending) = match &import {
                Some(run) => (
                    schema.self_managed || run.finished,
                    run.started && !schema.self_managed,
                    u32::from(!schema.self_managed),
                    // A rebuilt server may bring updates; whether it does is the tool's to say, so
                    // running it again is what is due.
                    u32::from(!schema.self_managed && run.finished && !run.current && plan.import.as_ref().is_some_and(|i| i.build.is_some())),
                ),
                None => (
                    facts.ledger.contains(&schema.key("setup:done")),
                    facts.ledger.contains(&schema.key("setup:started")),
                    schema.updates.len() as u32,
                    schema.updates.iter().filter(|u| !versions.is_some_and(|v| v.contains(&u.version))).count() as u32,
                ),
            };
            SchemaStatus {
                name: schema.name.clone(),
                exists: facts.schemas.contains(&schema.name),
                tables: table_count as u32,
                setup_done,
                unmanaged: !schema.self_managed && table_count > 0 && !started && !setup_done,
                populate_total: schema.populate.len() as u32,
                populate_pending: schema.populate.iter().filter(|f| !facts.ledger.contains(&schema.key(&format!("populate:{f}")))).count() as u32,
                version: versions.and_then(|v| v.iter().next_back()).map(|(a, b, c)| format!("{a}.{b}.{c}")),
                updates_total,
                updates_pending,
                missing_tables: schema
                    .required_tables
                    .iter()
                    .filter(|t| !tables.is_some_and(|have| have.contains(t.as_str())))
                    .cloned()
                    .collect(),
            }
        })
        .collect()
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum DatabaseState {
    /// Docker, the scripts or the database could not be asked.
    Unknown,
    /// Nothing has been set up yet.
    NotProvisioned,
    /// The container is not running, so its contents cannot be read.
    Stopped,
    Starting,
    /// Something is missing that setting up again will supply.
    Incomplete,
    /// Complete, and newer update scripts are waiting.
    Outdated,
    Ready,
}

fn overall(schemas: &[SchemaStatus], realms: Option<u32>) -> DatabaseState {
    let incomplete = schemas
        .iter()
        .any(|s| !s.setup_done || s.unmanaged || s.populate_pending > 0 || !s.missing_tables.is_empty())
        || realms.unwrap_or(0) == 0;
    if incomplete {
        DatabaseState::Incomplete
    } else if schemas.iter().any(|s| s.updates_pending > 0) {
        DatabaseState::Outdated
    } else {
        DatabaseState::Ready
    }
}

/// What is missing, in a sentence, for the failure message and the UI.
pub fn describe_gaps(schemas: &[SchemaStatus], realms: Option<u32>) -> String {
    let mut gaps = Vec::new();
    for s in schemas {
        if s.unmanaged {
            gaps.push(format!("{} already holds tables this app did not create", s.name));
        } else if !s.setup_done {
            gaps.push(format!("{} is not set up", s.name));
        } else if s.populate_pending > 0 {
            gaps.push(format!("{} still has {} data files to load", s.name, s.populate_pending));
        } else if !s.missing_tables.is_empty() {
            gaps.push(format!("{} lacks the table{} {}", s.name, if s.missing_tables.len() == 1 { "" } else { "s" }, s.missing_tables.join(", ")));
        }
        if s.updates_pending > 0 {
            gaps.push(format!("{} has {} update{} to apply", s.name, s.updates_pending, if s.updates_pending == 1 { "" } else { "s" }));
        }
    }
    if realms.unwrap_or(0) == 0 {
        gaps.push("no realm is listed for players to connect to".to_string());
    }
    gaps.join("; ")
}

// ---------------------------------------------------------------------------------------
// Talking to the container
// ---------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Container {
    Missing,
    Stopped,
    Starting,
    Healthy,
}

/// What Docker says about the database container, as the state that matters here.
fn container_of(seen: Option<&ContainerState>) -> Container {
    match seen.map(|c| (c.status.as_str(), c.health.as_deref())) {
        Some(("running", Some("healthy"))) => Container::Healthy,
        Some(("running" | "restarting", _)) => Container::Starting,
        Some(_) => Container::Stopped,
        None => Container::Missing,
    }
}

async fn container_state(instance: &str) -> Container {
    container_of(docker::container(&compose::database_container(instance)).await.as_ref())
}

/// Whether the database container is up and answering, so it can be asked something.
pub(crate) async fn is_healthy(instance: &str) -> bool {
    container_state(instance).await == Container::Healthy
}

pub(crate) async fn volume_exists(instance: &str) -> bool {
    docker::volume_exists(&compose::database_volume(instance)).await
}

/// `docker exec` into the database container as the database's root. The password travels in the
/// environment of the Docker client (`-e MYSQL_PWD` with no value), not in the arguments.
fn exec_spec(instance: &str, creds: &Credentials, stdin: Option<Vec<u8>>, args: &[&str]) -> CommandSpec {
    let mut all: Vec<String> = vec!["exec".into()];
    if stdin.is_some() {
        all.push("-i".into());
    }
    all.extend(["-e".into(), "MYSQL_PWD".into(), compose::database_container(instance)]);
    all.extend(args.iter().map(|a| a.to_string()));
    let mut spec = CommandSpec { program: "docker".into(), args: all, ..CommandSpec::default() }.env("MYSQL_PWD", &creds.root_password);
    spec.stdin = stdin;
    spec
}

fn sql_spec(instance: &str, creds: &Credentials, sql: &str) -> CommandSpec {
    exec_spec(instance, creds, Some(sql.as_bytes().to_vec()), &client_args(creds.engine)).timeout(Duration::from_secs(120))
}

/// The most statement text handed to the client as an argument: Linux limits one argument to 128 KiB.
const MAX_ARGUMENT: usize = 100 * 1024;

/// Asks the engine to run the client in the database container: no program is started on this
/// machine, and the password reaches the client through the engine. `None` when the engine cannot
/// be reached that way, or the statements are too long for an argument.
async fn ask_engine(instance: &str, creds: &Credentials, sql: &str) -> Option<Probe> {
    if sql.len() > MAX_ARGUMENT {
        return None;
    }
    let mut command: Vec<String> = client_args(creds.engine).iter().map(|a| a.to_string()).collect();
    command.extend(["-e".to_string(), sql.to_string()]);
    docker::exec(&compose::database_container(instance), &[("MYSQL_PWD", creds.root_password.as_str())], &command, Duration::from_secs(120)).await
}

/// Runs statements and returns what they print. A failure carries the client's message.
pub(crate) async fn query(instance: &str, creds: &Credentials, sql: &str) -> Result<String, String> {
    let out = match ask_engine(instance, creds, sql).await {
        Some(answer) => answer,
        None => capture(&sql_spec(instance, creds, sql)).await,
    };
    if out.success {
        Ok(out.text)
    } else {
        Err(out.text)
    }
}

async fn read_facts(instance: &str, creds: &Credentials, plan: &Plan, recipe: &Recipe) -> Result<Facts, String> {
    let overview = query(instance, creds, &overview_sql(plan)).await?;
    let details = details_sql(plan, recipe, &parse_facts(&overview));
    let more = if details.is_empty() { String::new() } else { query(instance, creds, &details).await? };
    Ok(parse_facts(&format!("{overview}\n{more}")))
}

pub(crate) fn sql_quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\'', "''")
}

/// Statements that create the ledger, the game's schemas and the game servers' account. Safe to
/// repeat: it also keeps the account's password in step with the saved credentials.
fn accounts_sql(recipe: &Recipe, creds: &Credentials) -> String {
    let mut sql = String::new();
    let charset = &recipe.database.charset;
    let names = std::iter::once(LEDGER_SCHEMA).chain(recipe.database.schemas.iter().map(|s| s.name.as_str()));
    for name in names {
        sql += &format!("CREATE DATABASE IF NOT EXISTS `{name}` DEFAULT CHARACTER SET {charset} COLLATE {charset}_general_ci;\n");
    }
    sql += &format!(
        "CREATE TABLE IF NOT EXISTS `{LEDGER_SCHEMA}`.`{LEDGER_TABLE}` (`name` VARCHAR(190) NOT NULL, \
         `at` TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY (`name`)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3;\n"
    );
    let (user, password) = (sql_quote(&creds.user), sql_quote(&creds.user_password));
    sql += &format!("CREATE USER IF NOT EXISTS '{user}'@'%' IDENTIFIED BY '{password}';\n");
    sql += &format!("ALTER USER '{user}'@'%' IDENTIFIED BY '{password}';\n");
    let grants = recipe.database.grants.join(", ");
    for schema in &recipe.database.schemas {
        sql += &format!("GRANT {grants} ON `{}`.* TO '{user}'@'%';\n", schema.name);
    }
    sql
}

/// Empties a schema so it can be filled again. Grants on it stay, because they belong to the
/// account, not to the schema.
pub(crate) fn recreate_sql(charset: &str, schema: &str) -> String {
    format!("DROP DATABASE IF EXISTS `{schema}`;\nCREATE DATABASE `{schema}` DEFAULT CHARACTER SET {charset} COLLATE {charset}_general_ci;\n")
}

fn ledger_insert(key: &str) -> String {
    format!("INSERT IGNORE INTO `{LEDGER_SCHEMA}`.`{LEDGER_TABLE}` (`name`) VALUES ('{}');\n", sql_quote(key))
}

// The scripts below take everything variable as positional arguments, so a file or schema name
// is never pasted into shell text. `@LEDGER@` and `@VERSION@` are fixed constants of this module.

/// `sh -c <script> sh <schema> <sql file>`.
const RUN_FILE_SCRIPT: &str = r#"@CLIENT@ -uroot --default-character-set=utf8mb3 "$1" < "$2""#;

/// `sh -c <script> sh <schema> <dir> <already loaded> <all files> <files still to load...>`
const POPULATE_SCRIPT: &str = r#"schema="$1"; dir="$2"; i="$3"; all="$4"; shift 4
for f in "$@"; do
  i=$((i+1))
  echo "FILE $i $all $f"
  @CLIENT@ -uroot --default-character-set=utf8mb3 "$schema" < "$dir/$f" || { echo "FAILED $f"; exit 1; }
  @CLIENT@ -uroot --default-character-set=utf8mb3 -e "INSERT IGNORE INTO @LEDGER@.ledger (name) VALUES ('$schema:populate:$f')" || exit 1
done"#;

/// `sh -c <script> sh <schema> <dir> <file:version:structure:content...>`
const UPDATE_SCRIPT: &str = r#"schema="$1"; dir="$2"; shift 2
total=$#; i=0
for spec in "$@"; do
  i=$((i+1))
  f=${spec%%:*}; rest=${spec#*:}; v=${rest%%:*}; rest=${rest#*:}; s=${rest%%:*}; c=${rest#*:}
  echo "UPDATE $i $total $f"
  @CLIENT@ -uroot --default-character-set=utf8mb3 "$schema" < "$dir/$f" || echo "The client reported an error in $f"
  n=$(@CLIENT@ -uroot -N -e "SELECT COUNT(*) FROM \`$schema\`.\`@VERSION@\` WHERE version=$v AND structure=$s AND content=$c")
  [ "$n" = "1" ] || { echo "FAILED $f was not recorded in @VERSION@"; exit 1; }
done"#;

pub(crate) fn script(text: &str) -> String {
    text.replace("@LEDGER@", LEDGER_SCHEMA).replace("@VERSION@", VERSION_TABLE)
}

/// Runs a script in the database container. `@CLIENT@` and `@DUMP@` become the programs of the
/// server the container runs.
pub(crate) fn script_spec(instance: &str, creds: &Credentials, script_text: &str, args: &[String]) -> CommandSpec {
    let text = script_text.replace("@CLIENT@", creds.engine.client()).replace("@DUMP@", creds.engine.dump());
    let mut all: Vec<&str> = vec!["sh", "-c", &text, "sh"];
    all.extend(args.iter().map(String::as_str));
    exec_spec(instance, creds, None, &all)
}

/// `FILE 3 120 name.sql` and `UPDATE 2 20 name.sql`: the kind, how many are done, how many there are.
fn script_progress(line: &str) -> Option<(&str, u32, u32)> {
    let mut parts = line.split_whitespace();
    let kind = parts.next().filter(|k| ["FILE", "UPDATE"].contains(k))?;
    Some((kind, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Turns a script's progress lines into a live status such as `Loading mangos0 · 37 of 120 files`.
fn tracker(label: String, kind: &'static str, unit: &'static str) -> impl FnMut(&str) -> Option<String> + Send {
    move |line| {
        let (k, done, total) = script_progress(line)?;
        (k == kind).then(|| format!("{label} · {done} of {total} {unit}"))
    }
}

pub(crate) fn explain(what: &str, error: &str, tail: &[String]) -> String {
    match failure_detail(tail) {
        Some(detail) => format!("{what}: {error} Last output: {detail}"),
        None => format!("{what}: {error}"),
    }
}

// ---------------------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseStatus {
    pub state: DatabaseState,
    /// Why setting up cannot start right now, if it cannot.
    pub blocked: Option<String>,
    /// Something worth knowing that does not stop setting up.
    pub note: Option<String>,
    pub container: String,
    pub volume: String,
    pub image: String,
    pub schemas: Vec<SchemaStatus>,
    /// How many realms players can pick, when it could be read.
    pub realms: Option<u32>,
}

/// Makes sure the database image exists under this server's own name (`worldstone/vanilla-database`):
/// downloads the recipe's public image if it is missing, gives it the name, and drops the public name
/// so Docker Desktop lists one image that says what it belongs to. The public name is kept when any
/// other container uses it, and dropping a name never removes the image itself.
async fn ensure_image(task: &mut Task, instance: &str, recipe: &Recipe) -> Result<(), String> {
    let (source, alias) = (recipe.database.image.as_str(), builder::database_image(instance, recipe));
    if docker::image(&alias).await.is_some() {
        return Ok(());
    }
    // Pulled on its own: Compose's pull progress is a line per few kilobytes, which buries the
    // activity log, while `docker pull` prints one line per layer step.
    if docker::image(source).await.is_none() {
        task.step("Downloading the database server (first time only)");
        let pull = CommandSpec::new("docker", &["pull", source]).timeout(Duration::from_secs(30 * 60));
        task.run(pull).await.map_err(|e| explain("The database server could not be downloaded", &e, &task.recent_output()))?;
    }
    task.step("Naming the database image");
    let tag = probe("docker", &["tag", source, &alias]).await;
    if !tag.success {
        return Err(format!("Docker could not name the database image {alias}: {}", tag.text.trim()));
    }
    task.info(&format!("The database image is now {alias}."));
    let id = docker::image(&alias).await.map(|image| image.id).unwrap_or_default();
    let others = crate::images::container_users()
        .await?
        .into_iter()
        .filter(|u| u.image_id == id && u.container != compose::database_container(instance))
        .count();
    if others > 0 {
        task.info(&format!("{source} is kept: {others} other container{} use it.", if others == 1 { "" } else { "s" }));
    } else if probe("docker", &["image", "rm", source]).await.success {
        task.info(&format!("Dropped the public name {source}; the image is still there under {alias}."));
    }
    Ok(())
}

/// Reads the database's state without starting anything.
pub async fn status(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> DatabaseStatus {
    let base = DatabaseStatus {
        state: DatabaseState::Unknown,
        blocked: None,
        note: None,
        container: compose::database_container(instance),
        volume: compose::database_volume(instance),
        image: builder::database_image(instance, recipe),
        schemas: Vec::new(),
        realms: None,
    };
    if let Err(reason) = builder::engine().await {
        return DatabaseStatus { blocked: Some(reason), ..base };
    }
    let plan = read_plan(instance, paths, recipe).await;
    let blocked = plan.as_ref().err().cloned();
    let container = container_state(instance).await;
    let state = match container {
        Container::Missing if !volume_exists(instance).await => {
            return DatabaseStatus { state: DatabaseState::NotProvisioned, blocked, ..base };
        }
        Container::Missing | Container::Stopped => {
            return DatabaseStatus { state: DatabaseState::Stopped, blocked, ..base };
        }
        Container::Starting => {
            return DatabaseStatus { state: DatabaseState::Starting, blocked, ..base };
        }
        Container::Healthy => (),
    };
    let _ = state;
    let plan = match plan {
        Ok(plan) => plan,
        Err(why) => return DatabaseStatus { note: Some(why), ..base },
    };
    let creds = match Credentials::load(paths) {
        Ok(Some(creds)) => creds,
        Ok(None) => return DatabaseStatus { note: Some("The saved database passwords are missing, so the database cannot be read.".into()), ..base },
        Err(why) => return DatabaseStatus { note: Some(why), ..base },
    };
    match read_facts(instance, &creds, &plan, recipe).await {
        Ok(facts) => {
            let schemas = classify(&plan, &facts);
            DatabaseStatus { state: overall(&schemas, facts.realms), schemas, realms: facts.realms, ..base }
        }
        Err(why) => DatabaseStatus { note: Some(format!("Could not read the database: {why}")), ..base },
    }
}

// ---------------------------------------------------------------------------------------
// Setting up
// ---------------------------------------------------------------------------------------

/// Sets the database up, or brings it up to date, and checks the result. Every step is safe to
/// repeat and to resume after an interruption.
pub async fn provision(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    provision_after_backup(task, paths, instance, recipe, false).await
}

/// [`provision`]. `backed_up` says that the caller has just made a backup of everything, so the one
/// taken before applying updates would only repeat it.
pub async fn provision_after_backup(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, backed_up: bool) -> Result<(), String> {
    task.step("Checking Docker and the database scripts");
    builder::engine().await?;
    if !probe("docker", &["compose", "version", "--short"]).await.success {
        return Err("Docker Compose is not available. Update Docker Desktop.".to_string());
    }
    let plan = read_plan(instance, paths, recipe).await?;
    paths.ensure()?;
    if Credentials::load(paths)?.is_none() && volume_exists(instance).await {
        return Err(format!(
            "The database volume {} already exists but its saved passwords are gone, so it cannot be opened. \
             Restore Config/{CREDENTIALS_FILE} from a backup, or remove that Docker volume (this deletes its data) and set up again.",
            compose::database_volume(instance)
        ));
    }
    let creds = Credentials::load_or_create(paths, &recipe.database.user, recipe.database.engine)?;
    for secret in creds.secrets() {
        task.add_secret(secret);
    }
    let compose_file = compose::write(paths, recipe, instance)?;

    ensure_image(task, instance, recipe).await?;

    task.step("Starting the database");
    let up = CommandSpec::new(
        "docker",
        &[
            "compose", "--progress", "quiet", "-p", &compose::project_name(instance), "-f", &compose_file.to_string_lossy(),
            "up", "-d", "--wait", compose::DATABASE_SERVICE,
        ],
    )
    .cwd(&paths.docker)
    .timeout(Duration::from_secs(30 * 60));
    task.run(up).await.map_err(|e| explain("The database did not start", &e, &task.recent_output()))?;
    query(instance, &creds, "SELECT 1;")
        .await
        .map_err(|e| format!("The database is running but refused the saved root password: {e}"))?;

    task.step("Creating the databases and the game account");
    query(instance, &creds, &accounts_sql(recipe, &creds)).await.map_err(|e| format!("Could not create the databases: {e}"))?;

    // What existed before this run changes, to decide what needs a backup.
    let before = classify(&plan, &read_facts(instance, &creds, &plan, recipe).await?);
    for (schema, status) in plan.schemas.iter().zip(&before) {
        if status.unmanaged {
            return Err(format!(
                "The database {} already holds tables that this app did not create. To protect them it will not load anything \
                 into it. Back them up and drop the database (or remove the Docker volume {}) if you want a fresh setup.",
                schema.name,
                compose::database_volume(instance)
            ));
        }
    }

    match &plan.import {
        Some(import) => import_schemas(task, paths, instance, recipe, &plan, import, &creds, &before, backed_up).await?,
        None => load_scripts(task, instance, recipe, &plan, &creds, &before, backed_up).await?,
    }

    let facts = read_facts(instance, &creds, &plan, recipe).await?;
    if let (Some(script), Some(0)) = (&plan.realm_script, facts.realms) {
        task.step("Adding the realm players connect to");
        let realm = &recipe.database.realm;
        let args = [realm.schema.clone(), script.clone()];
        task.run(script_spec(instance, &creds, RUN_FILE_SCRIPT, &args))
            .await
            .map_err(|e| explain("Adding the realm failed", &e, &task.recent_output()))?;
    }

    task.step("Checking the result");
    let facts = read_facts(instance, &creds, &plan, recipe).await?;
    let schemas = classify(&plan, &facts);
    if overall(&schemas, facts.realms) != DatabaseState::Ready {
        return Err(format!("The database is not complete: {}.", describe_gaps(&schemas, facts.realms)));
    }
    for s in &schemas {
        task.info(&format!("{}: {} tables, version {}", s.name, s.tables, s.version.as_deref().unwrap_or("unknown")));
    }
    task.info(&format!("{} realm{} listed. The database is ready.", facts.realms.unwrap_or(0), if facts.realms == Some(1) { "" } else { "s" }));
    Ok(())
}

/// The scripts method: the repository's setup scripts, data files and updates, run in order.
async fn load_scripts(
    task: &mut Task,
    instance: &str,
    recipe: &Recipe,
    plan: &Plan,
    creds: &Credentials,
    before: &[SchemaStatus],
    backed_up: bool,
) -> Result<(), String> {
    for schema in &plan.schemas {
        let facts = read_facts(instance, &creds, &plan, recipe).await?;
        let status = classify(&plan, &facts).into_iter().find(|s| s.name == schema.name).expect("classified with the plan");
        if !status.setup_done {
            task.step(&format!("Creating the tables of {}", schema.name));
            query(instance, &creds, &ledger_insert(&schema.key("setup:started"))).await?;
            let args = [schema.name.clone(), schema.setup_path()];
            task.run(script_spec(instance, &creds, RUN_FILE_SCRIPT, &args))
                .await
                .map_err(|e| explain(&format!("Creating the tables of {} failed", schema.name), &e, &task.recent_output()))?;
            query(instance, &creds, &ledger_insert(&schema.key("setup:done"))).await?;
        }
        if let Some(dir) = schema.populate_path() {
            let pending: Vec<String> = schema
                .populate
                .iter()
                .filter(|f| !facts.ledger.contains(&schema.key(&format!("populate:{f}"))))
                .cloned()
                .collect();
            // A schema that was just created has nothing loaded, whatever an older ledger said.
            if !pending.is_empty() {
                let total = schema.populate.len();
                let label = format!("Loading the {} data", schema.name);
                task.step(&label);
                let mut args = vec![schema.name.clone(), dir, (total - pending.len()).to_string(), total.to_string()];
                args.extend(pending);
                let mut track = tracker(label, "FILE", "files");
                task.run_tracked(script_spec(instance, &creds, &script(POPULATE_SCRIPT), &args), &mut track)
                    .await
                    .map_err(|e| explain(&format!("Loading the {} data failed; run this again to continue", schema.name), &e, &task.recent_output()))?;
            }
        }
    }

    let facts = read_facts(instance, &creds, &plan, recipe).await?;
    let now = classify(&plan, &facts);
    let gaps: Vec<String> = now.iter().filter(|s| !s.missing_tables.is_empty()).map(|s| format!("{} lacks {}", s.name, s.missing_tables.join(", "))).collect();
    if !gaps.is_empty() {
        return Err(format!("The tables were loaded but some are missing: {}. The scripts may be from a different version.", gaps.join("; ")));
    }

    // Only schemas that held data before this run are worth a backup; a new one has nothing to lose.
    let to_back_up: Vec<&str> = plan
        .schemas
        .iter()
        .zip(before)
        .zip(&now)
        .filter(|((_, was), is)| was.setup_done && was.populate_pending == 0 && is.updates_pending > 0)
        .map(|((schema, _), _)| schema.name.as_str())
        .collect();
    if !to_back_up.is_empty() && !backed_up {
        backups::backup(task, instance, &creds, &to_back_up, backups::BEFORE_UPDATE).await?;
    }

    for (schema, status) in plan.schemas.iter().zip(&now) {
        if status.updates_pending == 0 {
            continue;
        }
        let versions = facts.versions.get(&schema.name);
        let pending: Vec<String> = schema
            .updates
            .iter()
            .filter(|u| !versions.is_some_and(|v| v.contains(&u.version)))
            .map(|u| format!("{}:{}:{}:{}", u.file, u.version.0, u.version.1, u.version.2))
            .collect();
        // Update scripts sit in release folders; a schema's scripts all live in one folder per release.
        for folder in schema.updates.iter().map(|u| u.folder.as_str()).collect::<BTreeSet<_>>() {
            let in_folder: Vec<String> = schema
                .updates
                .iter()
                .filter(|u| u.folder == folder)
                .filter_map(|u| pending.iter().find(|p| p.starts_with(&format!("{}:", u.file))).cloned())
                .collect();
            if in_folder.is_empty() {
                continue;
            }
            let label = format!("Updating {}", schema.name);
            task.step(&label);
            let mut args = vec![schema.name.clone(), schema.update_path(folder)];
            args.extend(in_folder);
            let mut track = tracker(label, "UPDATE", "updates");
            task.run_tracked(script_spec(instance, &creds, &script(UPDATE_SCRIPT), &args), &mut track)
                .await
                .map_err(|e| explain(&format!("Updating {} failed; a backup was taken first", schema.name), &e, &task.recent_output()))?;
        }
    }
    Ok(())
}

/// The import method: the emulator's own importer creates the tables and applies the updates.
/// Everything around it is safe to repeat; the importer itself only applies what is missing.
#[allow(clippy::too_many_arguments)]
async fn import_schemas(
    task: &mut Task,
    paths: &InstancePaths,
    instance: &str,
    recipe: &Recipe,
    plan: &Plan,
    import: &ImportPlan,
    creds: &Credentials,
    before: &[SchemaStatus],
    backed_up: bool,
) -> Result<(), String> {
    let build = import.build.as_deref().expect("read with the plan");
    let facts = read_facts(instance, creds, plan, recipe).await?;
    let ledger = ImportLedger::read(plan, &facts).expect("an import plan has a ledger");
    if ledger.current && before.iter().all(|s| s.setup_done && s.missing_tables.is_empty()) {
        task.info("The database importer has already run for this build, so it is not run again.");
        return Ok(());
    }

    // The importer reads the server's own configuration, so it is written first.
    config::configure(task, paths, instance, recipe).await?;

    // Managed schemas that hold data are backed up before the importer touches them.
    let managed = |was: &&SchemaStatus, schema: &&SchemaPlan| !schema.self_managed && was.tables > 0;
    let holding: Vec<&str> = plan.schemas.iter().zip(before).filter(|(s, w)| managed(w, s)).map(|(s, _)| s.name.as_str()).collect();
    if ledger.finished && !holding.is_empty() && !backed_up {
        backups::backup(task, instance, creds, &holding, backups::BEFORE_UPDATE).await?;
    }
    // A first import that never finished left partial tables and no game data. The importer only
    // fills an empty database, so those are emptied and it starts over.
    if !ledger.finished && !holding.is_empty() {
        task.step("Clearing the unfinished import");
        task.info(&format!("An earlier import into {} did not finish. Those databases are emptied so it can start over.", holding.join(", ")));
        for name in &holding {
            query(instance, creds, &recreate_sql(&recipe.database.charset, name))
                .await
                .map_err(|e| format!("Could not empty {name}: {e}"))?;
        }
    }

    clear_import_container(instance).await?;
    task.step("Importing the databases");
    query(instance, creds, &ledger_insert(IMPORT_STARTED)).await?;
    let mut applied = 0u32;
    let mut track = move |line: &str| {
        (line.contains("Applying") || line.contains("Applied")).then(|| {
            applied += 1;
            format!("Importing the databases · {applied} steps reported")
        })
    };
    let ran = task.run_tracked(import_command(paths, instance, recipe, import)?, &mut track).await;
    if let Err(e) = ran {
        // Stopping the Docker client does not stop the container, and an importer that has failed
        // may sit there without exiting, so it is removed here rather than left to be found later.
        remove_import_container(instance).await;
        if e == CANCELLED {
            return Err(e);
        }
        return Err(explain("The database import failed; run this again to continue", &e, &task.recent_output()));
    }

    task.step("Checking the imported tables");
    let facts = read_facts(instance, creds, plan, recipe).await?;
    let missing: Vec<String> = classify(plan, &facts)
        .iter()
        .filter(|s| !s.missing_tables.is_empty())
        .map(|s| format!("{} lacks {}", s.name, s.missing_tables.join(", ")))
        .collect();
    if !missing.is_empty() {
        return Err(format!("The importer finished but some tables are missing: {}.", missing.join("; ")));
    }
    query(instance, creds, &ledger_insert(&format!("{IMPORT_DONE}{build}"))).await?;
    Ok(())
}

/// `docker run` for the importer: the server's image with its configuration folders and logs, on
/// the project's network so the database is reachable as its service name. The compose file is not
/// used because it declares the game data volume, which may not exist yet.
fn import_command(paths: &InstancePaths, instance: &str, recipe: &Recipe, import: &ImportPlan) -> Result<CommandSpec, String> {
    let service = recipe
        .run
        .services
        .iter()
        .find(|s| s.id == import.service)
        .ok_or_else(|| format!("The recipe has no {} server to run the importer with.", import.service))?;
    let mut args: Vec<String> = vec![
        "run".into(), "--rm".into(), "--init".into(),
        "--name".into(), compose::import_container(instance),
        "--network".into(), compose::network_name(instance),
    ];
    let config = config::config_dir(paths);
    for (folder, target) in compose::config_folders(recipe, service) {
        let source = if folder.is_empty() { config.clone() } else { config.join(&folder) };
        args.extend(["-v".into(), format!("{}:{target}:ro", source.display())]);
    }
    args.extend(["-v".into(), format!("{}:{}", config::logs_dir(paths).display(), recipe.run.logs_mount)]);
    args.extend(["-w".into(), recipe.run.config_mount.clone(), "--entrypoint".into(), import.tool.clone()]);
    args.push(builder::image_ref(instance, recipe, service));
    args.extend(import.args.iter().cloned());
    Ok(CommandSpec { program: "docker".into(), args, ..CommandSpec::default() }.timeout(Duration::from_secs(60 * 60)))
}

/// Stops and removes the importer's container, whatever it is doing. A container that is already
/// gone is not a problem.
async fn remove_import_container(instance: &str) {
    probe("docker", &["rm", "-f", &compose::import_container(instance)]).await;
}

/// Removes the importer's container from an earlier run, unless that run is still going.
async fn clear_import_container(instance: &str) -> Result<(), String> {
    let name = compose::import_container(instance);
    let Some(state) = docker::container(&name).await else {
        return Ok(());
    };
    if matches!(state.status.as_str(), "running" | "restarting") {
        return Err(format!("The database import from an earlier run is still going ({name}). Wait for it to finish, then try again."));
    }
    let removed = probe("docker", &["rm", &name]).await;
    if removed.success {
        Ok(())
    } else {
        Err(format!("Docker could not remove the old import container {name}: {}", removed.text.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero() -> Recipe {
        crate::recipe::builtin().unwrap().remove(0)
    }

    fn creds() -> Credentials {
        Credentials { root_password: "root-secret-value".into(), user: "mangos".into(), user_password: "user-secret-value".into(), engine: Default::default() }
    }

    fn schema(name: &str, populate: &[&str], updates: &[(u32, u32, u32)], tables: &[&str]) -> SchemaPlan {
        SchemaPlan {
            name: name.into(),
            dir: "Dir".into(),
            setup: "load.sql".into(),
            self_managed: false,
            populate_dir: (!populate.is_empty()).then(|| "Full".to_string()),
            populate: populate.iter().map(|s| s.to_string()).collect(),
            updates: updates.iter().map(|&version| Update { file: format!("Rel{}_{:02}_{:03}_x.sql", version.0, version.1, version.2), folder: "Rel22".into(), version }).collect(),
            required_tables: tables.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn plan() -> Plan {
        Plan {
            schemas: vec![
                schema("realmd", &[], &[(22, 1, 1), (22, 5, 1)], &["db_version", "realmlist"]),
                schema("mangos0", &["a.sql", "b.sql"], &[(22, 4, 30)], &["db_version", "creature_template"]),
            ],
            realm_script: Some("/worldstone/sql/Tools/updateRealm.sql".into()),
            import: None,
        }
    }

    fn azerothcore() -> Recipe {
        crate::recipe::builtin().unwrap().into_iter().find(|r| r.id == "azerothcore").unwrap()
    }

    /// The plan for a game whose own importer fills the databases, for a server image of `build`.
    fn import_plan(build: Option<&str>) -> Plan {
        let mut plan = Plan::read(&crate::paths::Paths::at(std::env::temp_dir()).instance("azerothcore").unwrap(), &azerothcore()).unwrap();
        plan.import.as_mut().unwrap().build = build.map(String::from);
        plan
    }

    const IMPORTED: &str = "S\tacore_auth\nS\tacore_characters\nS\tacore_world\nS\tacore_playerbots\n\
        T\tacore_auth\tupdates\nT\tacore_auth\trealmlist\nT\tacore_auth\taccount\nT\tacore_auth\taccount_access\n\
        T\tacore_characters\tupdates\nT\tacore_characters\tcharacters\nT\tacore_characters\tcharacter_inventory\n\
        T\tacore_world\tupdates\nT\tacore_world\tcreature_template\nT\tacore_world\titem_template\nR\t1\n";

    fn facts(text: &str) -> Facts {
        parse_facts(text)
    }

    #[test]
    fn update_names_give_the_row_they_write() {
        assert_eq!(parse_update_name("Rel22_04_030_PygmySurfCrawler_Level_Range.sql"), Some((22, 4, 30)));
        assert_eq!(parse_update_name("Rel22_05_001_Cata_Warden_Identity.sql"), Some((22, 5, 1)));
        for bad in ["readme.txt", "Rel22_04.sql", "Update_22_04_030.sql", "Rel22_xx_030_a.sql", "Rel22_04_030_a.zip"] {
            assert_eq!(parse_update_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_plan_reads_the_repository_and_orders_updates_by_version() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        let recipe = zero();
        let repo = paths.repos.join(&recipe.database_repo().path);
        for s in &recipe.database.schemas {
            let (dir, file) = (s.dir.as_deref().unwrap(), s.setup.as_deref().unwrap());
            let setup = repo.join(dir).join("Setup");
            std::fs::create_dir_all(&setup).unwrap();
            std::fs::write(setup.join(file), "-- tables").unwrap();
            if let Some(p) = &s.populate {
                std::fs::create_dir_all(setup.join(p)).unwrap();
                std::fs::write(setup.join(p).join("b.sql"), "").unwrap();
                std::fs::write(setup.join(p).join("a.sql"), "").unwrap();
            }
            let rel = repo.join(dir).join("Updates").join("Rel22");
            std::fs::create_dir_all(rel.join("Rel21_to_BaseRel22_Updates")).unwrap();
            std::fs::write(rel.join("Rel22_05_001_b.sql"), "").unwrap();
            std::fs::write(rel.join("Rel22_04_030_a.sql"), "").unwrap();
            std::fs::write(rel.join("readme.txt"), "").unwrap();
        }
        std::fs::create_dir_all(repo.join("Tools")).unwrap();
        std::fs::write(repo.join("Tools").join("updateRealm.sql"), "").unwrap();

        let plan = Plan::read(&paths, &recipe).unwrap();
        let world = plan.schemas.iter().find(|s| s.name == "mangos0").unwrap();
        assert_eq!(world.populate, ["a.sql", "b.sql"]);
        assert_eq!(world.updates.iter().map(|u| u.version).collect::<Vec<_>>(), [(22, 4, 30), (22, 5, 1)]);
        assert_eq!(world.setup_path(), "/worldstone/sql/World/Setup/mangosdLoadDB.sql");
        assert_eq!(world.update_path("Rel22"), "/worldstone/sql/World/Updates/Rel22");
        assert_eq!(plan.realm_script.as_deref(), Some("/worldstone/sql/Tools/updateRealm.sql"));
        assert!(plan.import.is_none(), "the scripts method has no importer");
    }

    #[test]
    fn a_missing_or_incomplete_repository_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        let recipe = zero();
        assert!(Plan::read(&paths, &recipe).unwrap_err().contains("Download the source"));
        std::fs::create_dir_all(paths.repos.join(&recipe.database_repo().path).join("Realm")).unwrap();
        let why = Plan::read(&paths, &recipe).unwrap_err();
        assert!(why.contains("Realm/Setup/realmdLoadDB.sql") && why.contains("submodule"), "{why}");
    }

    #[test]
    fn scripts_with_odd_names_are_refused_rather_than_passed_to_a_shell() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a b.sql"), "").unwrap();
        assert!(sql_files(dir.path()).unwrap_err().contains("unexpected name"));
    }

    #[test]
    fn credentials_are_created_once_and_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        assert!(Credentials::load(&paths).unwrap().is_none());
        let first = Credentials::load_or_create(&paths, "mangos", Default::default()).unwrap();
        assert_eq!(first.root_password.len(), 64);
        assert_ne!(first.root_password, first.user_password);
        let second = Credentials::load_or_create(&paths, "mangos", Default::default()).unwrap();
        assert_eq!(first, second, "the volume holds the first root password");
        let env = std::fs::read_to_string(paths.config.join("database.env")).unwrap();
        assert!(env.contains(&format!("MARIADB_ROOT_PASSWORD={}", first.root_password)));
        assert!(env.contains("MARIADB_ROOT_HOST=localhost"), "root only works from inside the container");
    }

    #[test]
    fn a_mysql_database_gets_its_own_variables_programs_and_saved_engine() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("azerothcore").unwrap();
        let creds = Credentials::load_or_create(&paths, "acore", DatabaseEngine::Mysql).unwrap();
        let env = std::fs::read_to_string(paths.config.join("database.env")).unwrap();
        assert!(env.contains(&format!("MYSQL_ROOT_PASSWORD={}", creds.root_password)) && env.contains("MYSQL_ROOT_HOST=localhost"), "{env}");
        assert!(!env.contains("MARIADB"));
        assert_eq!(Credentials::load(&paths).unwrap().unwrap().engine, DatabaseEngine::Mysql, "kept for every later statement");

        let statement = sql_spec("azerothcore", &creds, "SELECT 1");
        assert!(statement.args.iter().any(|a| a == "mysql") && !statement.args.iter().any(|a| a == "mariadb"), "{:?}", statement.args);

        let backup = script_spec("azerothcore", &creds, &script(crate::backups::BACKUP_SCRIPT), &[]);
        let text = backup.args.join(" ");
        assert!(text.contains("mysqldump -uroot") && !text.contains("@DUMP@") && !text.contains("mariadb"), "{text}");
        let load = script_spec("azerothcore", &creds, crate::backups::LOAD_SCRIPT, &[]);
        assert!(load.args.join(" ").contains("mysql -uroot") && !load.args.join(" ").contains("@CLIENT@"));
    }

    #[test]
    fn credentials_saved_before_the_engine_was_recorded_are_mariadbs() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("database.json"), r#"{"rootPassword":"a","user":"mangos","userPassword":"b"}"#).unwrap();
        let creds = Credentials::load(&paths).unwrap().unwrap();
        assert_eq!(creds.engine, DatabaseEngine::Mariadb);
        assert!(sql_spec("mangos-zero", &creds, "SELECT 1").args.iter().any(|a| a == "mariadb"));
    }

    #[test]
    fn credentials_never_show_in_debug_output() {
        assert!(!format!("{:?}", creds()).contains("secret"));
    }

    #[test]
    fn a_broken_credentials_file_is_reported_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("database.json"), "{ nope").unwrap();
        assert!(Credentials::load_or_create(&paths, "mangos", Default::default()).unwrap_err().contains("cannot be read"));
        assert_eq!(std::fs::read_to_string(paths.config.join("database.json")).unwrap(), "{ nope");
    }

    #[test]
    fn the_account_statements_create_the_schemas_and_grant_only_the_recipes_privileges() {
        let sql = accounts_sql(&zero(), &creds());
        for name in ["worldstone", "realmd", "character0", "mangos0"] {
            assert!(sql.contains(&format!("CREATE DATABASE IF NOT EXISTS `{name}`")), "{name}");
        }
        assert!(sql.contains("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE, DROP, ALTER, LOCK TABLES, CREATE ROUTINE, ALTER ROUTINE ON `mangos0`.* TO 'mangos'@'%'"));
        assert!(!sql.contains("`worldstone`.* TO"), "the game account cannot touch the ledger");
        assert!(!sql.contains("ALL PRIVILEGES") && !sql.contains("GRANT OPTION"));
        assert!(sql.contains("ALTER USER 'mangos'@'%'"), "the password follows the saved credentials");
    }

    #[test]
    fn quotes_in_a_password_cannot_end_the_string() {
        let mut c = creds();
        c.user_password = "a'b\\c".into();
        assert!(accounts_sql(&zero(), &c).contains("IDENTIFIED BY 'a''b\\\\c'"));
    }

    #[test]
    fn the_root_password_travels_in_the_environment_never_in_an_argument() {
        let spec = sql_spec("mangos-zero", &creds(), "SELECT 1;");
        assert_eq!(spec.program, "docker");
        assert!(spec.env.contains(&("MYSQL_PWD".to_string(), "root-secret-value".to_string())));
        assert!(spec.args.iter().all(|a| !a.contains("secret")), "{:?}", spec.args);
        assert!(spec.args.windows(2).any(|w| w == ["-e", "MYSQL_PWD"]), "the variable is named without a value");
        assert!(spec.args.contains(&"worldstone-mangos-zero-db".to_string()) && spec.args.contains(&"-i".to_string()));
        assert_eq!(spec.stdin.as_deref(), Some(b"SELECT 1;".as_slice()));
    }

    #[test]
    fn a_script_gets_names_as_arguments_and_never_has_them_pasted_in() {
        let spec = script_spec("mangos-zero", &creds(), &script(POPULATE_SCRIPT), &["mangos0".into(), "/worldstone/sql/World/Setup/FullDB".into(), "0".into(), "2".into(), "a.sql".into()]);
        assert!(!spec.args.contains(&"-i".to_string()), "nothing is fed to it");
        assert_eq!(spec.args[4..6], ["sh", "-c"], "docker exec <container> sh -c <script> sh <args...>");
        assert_eq!(spec.args[7..], ["sh", "mangos0", "/worldstone/sql/World/Setup/FullDB", "0", "2", "a.sql"]);
        let text = spec.args.join(" ");
        assert!(text.contains("worldstone.ledger") && !text.contains("@LEDGER@"), "{text}");
        assert!(!script(POPULATE_SCRIPT).contains("mangos0"), "schema names come in as arguments");
        assert!(!script(UPDATE_SCRIPT).contains("@VERSION@") && script(UPDATE_SCRIPT).contains("db_version"));
    }

    #[test]
    fn the_overview_queries_ask_about_every_schema_and_the_ledger() {
        let sql = overview_sql(&plan());
        assert!(sql.contains("IN ('realmd','mangos0','worldstone')"), "{sql}");
    }

    #[test]
    fn detail_queries_are_only_asked_of_tables_that_exist() {
        let recipe = zero();
        let empty = Facts::default();
        assert_eq!(details_sql(&plan(), &recipe, &empty), "");
        let seen = facts("T\trealmd\tdb_version\nT\trealmd\trealmlist\nT\tworldstone\tledger\nT\tmangos0\tcreature_template");
        let sql = details_sql(&plan(), &recipe, &seen);
        assert!(sql.contains("FROM `realmd`.`db_version`") && !sql.contains("`mangos0`.`db_version`"), "{sql}");
        assert!(sql.contains("FROM `worldstone`.`ledger`") && sql.contains("COUNT(*) FROM `realmd`.`realmlist`"));
    }

    #[test]
    fn facts_are_read_from_tagged_lines_and_other_output_is_ignored() {
        let f = facts(
            "S\trealmd\nT\trealmd\taccount\nT\trealmd\tdb_version\nL\trealmd:setup:done\nV\trealmd\t22\t1\t1\nV\trealmd\t22\t5\t1\nR\t1\n\
             mariadb: Deprecated program name\nV\trealmd\tx\t1\t1\n",
        );
        assert!(f.schemas.contains("realmd") && f.ledger.contains("realmd:setup:done"));
        assert_eq!(f.tables["realmd"].len(), 2);
        assert_eq!(f.versions["realmd"].iter().next_back(), Some(&(22, 5, 1)));
        assert_eq!(f.versions["realmd"].len(), 2, "a garbled row is not counted");
        assert_eq!(f.realms, Some(1));
    }

    fn ready_facts() -> Facts {
        facts(
            "S\trealmd\nS\tmangos0\nT\trealmd\tdb_version\nT\trealmd\trealmlist\nT\tmangos0\tdb_version\nT\tmangos0\tcreature_template\n\
             L\trealmd:setup:done\nL\tmangos0:setup:done\nL\tmangos0:populate:a.sql\nL\tmangos0:populate:b.sql\n\
             V\trealmd\t22\t1\t1\nV\trealmd\t22\t5\t1\nV\tmangos0\t22\t4\t30\nR\t1\n",
        )
    }

    #[test]
    fn a_complete_database_is_ready_and_reports_its_version() {
        let schemas = classify(&plan(), &ready_facts());
        assert_eq!(overall(&schemas, Some(1)), DatabaseState::Ready);
        assert_eq!(schemas[0].version.as_deref(), Some("22.5.1"));
        assert_eq!(schemas[1].version.as_deref(), Some("22.4.30"));
        assert!(schemas.iter().all(|s| s.updates_pending == 0 && s.missing_tables.is_empty()));
    }

    #[test]
    fn a_missing_update_row_makes_the_database_outdated_not_ready() {
        let mut f = ready_facts();
        f.versions.get_mut("realmd").unwrap().remove(&(22, 5, 1));
        let schemas = classify(&plan(), &f);
        assert_eq!(schemas[0].updates_pending, 1);
        assert_eq!(overall(&schemas, Some(1)), DatabaseState::Outdated);
        assert_eq!(describe_gaps(&schemas, Some(1)), "realmd has 1 update to apply");
    }

    #[test]
    fn unloaded_data_files_a_missing_table_or_no_realm_each_mean_incomplete() {
        let mut f = ready_facts();
        f.ledger.remove("mangos0:populate:b.sql");
        let schemas = classify(&plan(), &f);
        assert_eq!(schemas[1].populate_pending, 1);
        assert_eq!(overall(&schemas, Some(1)), DatabaseState::Incomplete);
        assert!(describe_gaps(&schemas, Some(1)).contains("mangos0 still has 1 data files to load"));

        let mut f = ready_facts();
        f.tables.get_mut("mangos0").unwrap().remove("creature_template");
        let schemas = classify(&plan(), &f);
        assert_eq!(schemas[1].missing_tables, ["creature_template"]);
        assert_eq!(overall(&schemas, Some(1)), DatabaseState::Incomplete);

        assert_eq!(overall(&classify(&plan(), &ready_facts()), Some(0)), DatabaseState::Incomplete, "no realm to join");
        assert_eq!(overall(&classify(&plan(), &ready_facts()), None), DatabaseState::Incomplete);
    }

    #[test]
    fn tables_the_app_did_not_create_are_flagged_and_never_counted_as_set_up() {
        let f = facts("S\trealmd\nT\trealmd\taccount\nT\trealmd\tdb_version");
        let schemas = classify(&plan(), &f);
        assert!(schemas[0].unmanaged && !schemas[0].setup_done);
        assert_eq!(overall(&schemas, None), DatabaseState::Incomplete);
        assert!(describe_gaps(&schemas, None).contains("realmd already holds tables this app did not create"));
        // Its own interrupted setup is not "foreign": the ledger says it started.
        let f = facts("S\trealmd\nT\trealmd\taccount\nL\trealmd:setup:started");
        assert!(!classify(&plan(), &f)[0].unmanaged);
    }

    #[test]
    fn an_empty_database_needs_everything() {
        let schemas = classify(&plan(), &facts("S\trealmd\nS\tmangos0\nS\tworldstone"));
        assert!(schemas.iter().all(|s| s.exists && s.tables == 0 && !s.setup_done && !s.unmanaged));
        assert_eq!(overall(&schemas, None), DatabaseState::Incomplete);
        assert_eq!(schemas[1].populate_pending, 2);
    }

    #[test]
    fn container_states_come_from_docker_inspect() {
        let seen = |status: &str, health: Option<&str>| ContainerState { status: status.into(), health: health.map(str::to_string), exit_code: 0, started_at: String::new() };
        assert_eq!(container_of(Some(&seen("running", Some("healthy")))), Container::Healthy);
        assert_eq!(container_of(Some(&seen("running", Some("starting")))), Container::Starting);
        assert_eq!(container_of(Some(&seen("running", None))), Container::Starting, "a container that reports no health is not known to be ready");
        assert_eq!(container_of(Some(&seen("exited", Some("unhealthy")))), Container::Stopped);
        assert_eq!(container_of(Some(&seen("exited", None))), Container::Stopped);
        assert_eq!(container_of(None), Container::Missing);
    }

    #[test]
    fn progress_comes_only_from_the_scripts_own_lines() {
        assert_eq!(script_progress("FILE 37 120 creature.sql"), Some(("FILE", 37, 120)));
        assert_eq!(script_progress("UPDATE 2 20 Rel22_04_031_x.sql"), Some(("UPDATE", 2, 20)));
        assert_eq!(script_progress("FILE x 120 a"), None);
        assert_eq!(script_progress("* UPDATE COMPLETE *"), None);
        let mut load = tracker("Loading the mangos0 data".into(), "FILE", "files");
        assert_eq!(load("FILE 37 120 c.sql").as_deref(), Some("Loading the mangos0 data · 37 of 120 files"));
        assert_eq!(load("UPDATE 1 2 x.sql"), None);
    }

    #[test]
    fn a_game_with_its_own_importer_plans_no_scripts() {
        let plan = import_plan(Some("abc"));
        assert_eq!(plan.schemas.len(), 4);
        assert!(plan.schemas.iter().all(|s| s.updates.is_empty() && s.populate.is_empty() && s.dir.is_empty()));
        assert!(plan.schemas.iter().find(|s| s.name == "acore_playerbots").unwrap().self_managed);
        assert_eq!(plan.realm_script, None, "the auth database's own data holds the realm");
        let import = plan.import.unwrap();
        assert_eq!(import.tool, "/azerothcore/bin/dbimport");
        assert_eq!(import.args, ["-c", "/worldstone/config/dbimport.conf"]);
        assert_eq!(import.service, "worldserver");
    }

    #[test]
    fn nothing_is_imported_or_ready_before_the_importer_has_run() {
        let plan = import_plan(Some("abc"));
        let schemas = classify(&plan, &facts("S\tacore_auth\nS\tacore_characters\nS\tacore_world\nS\tacore_playerbots\nS\tworldstone"));
        let managed: Vec<_> = schemas.iter().filter(|s| s.name != "acore_playerbots").collect();
        assert!(managed.iter().all(|s| !s.setup_done && !s.unmanaged && s.updates_pending == 0));
        let bots = schemas.iter().find(|s| s.name == "acore_playerbots").unwrap();
        assert!(bots.setup_done && bots.missing_tables.is_empty(), "the module fills its own schema when the world server first runs");
        assert_eq!(overall(&schemas, None), DatabaseState::Incomplete);
    }

    #[test]
    fn a_finished_import_for_this_build_is_ready_and_a_rebuild_makes_it_outdated() {
        let done = format!("{IMPORTED}L\t{IMPORT_STARTED}\nL\t{IMPORT_DONE}abc\n");
        let schemas = classify(&import_plan(Some("abc")), &facts(&done));
        assert_eq!(overall(&schemas, Some(1)), DatabaseState::Ready);
        assert!(schemas.iter().all(|s| s.updates_pending == 0));

        let rebuilt = classify(&import_plan(Some("def")), &facts(&done));
        assert_eq!(overall(&rebuilt, Some(1)), DatabaseState::Outdated, "a new build may bring updates, so the importer is due");
        let due: Vec<_> = rebuilt.iter().filter(|s| s.updates_pending > 0).map(|s| s.name.as_str()).collect();
        assert_eq!(due, ["acore_auth", "acore_characters", "acore_world"], "the module's own schema updates itself");

        let unknown = classify(&import_plan(None), &facts(&done));
        assert_eq!(overall(&unknown, Some(1)), DatabaseState::Ready, "with no build to compare, nothing is claimed to be due");
    }

    #[test]
    fn missing_tables_or_a_missing_realm_stop_an_import_from_counting_as_ready() {
        let done = format!("{IMPORTED}L\t{IMPORT_STARTED}\nL\t{IMPORT_DONE}abc\n").replace("T\tacore_world\titem_template\n", "");
        let schemas = classify(&import_plan(Some("abc")), &facts(&done));
        assert_eq!(schemas.iter().find(|s| s.name == "acore_world").unwrap().missing_tables, ["item_template"]);
        assert_eq!(overall(&schemas, Some(1)), DatabaseState::Incomplete);
        let ok = classify(&import_plan(Some("abc")), &facts(&format!("{IMPORTED}L\t{IMPORT_DONE}abc\n")));
        assert_eq!(overall(&ok, Some(0)), DatabaseState::Incomplete);
    }

    #[test]
    fn tables_the_app_did_not_create_are_never_imported_over_but_its_own_partial_import_is_recognised() {
        let foreign = classify(&import_plan(Some("abc")), &facts("S\tacore_world\nT\tacore_world\tcreature_template"));
        assert!(foreign.iter().find(|s| s.name == "acore_world").unwrap().unmanaged);
        let partial = classify(&import_plan(Some("abc")), &facts(&format!("S\tacore_world\nT\tacore_world\tcreature_template\nL\t{IMPORT_STARTED}")));
        let world = partial.iter().find(|s| s.name == "acore_world").unwrap();
        assert!(!world.unmanaged && !world.setup_done, "the ledger says the app started this import");
        let bots = classify(&import_plan(Some("abc")), &facts("S\tacore_playerbots\nT\tacore_playerbots\tplayerbots_names"));
        assert!(!bots.iter().find(|s| s.name == "acore_playerbots").unwrap().unmanaged, "the module's own tables are its own");
    }

    #[test]
    fn the_importer_runs_in_a_throwaway_container_with_the_servers_folders() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("azerothcore").unwrap();
        let recipe = azerothcore();
        let plan = import_plan(Some("abc"));
        let spec = import_command(&paths, "azerothcore", &recipe, plan.import.as_ref().unwrap()).unwrap();
        assert_eq!(spec.program, "docker");
        let text = spec.args.join(" ");
        assert!(text.starts_with("run --rm --init --name worldstone-azerothcore-import --network worldstone-azerothcore_default "), "{text}");
        let mounts: Vec<&String> = spec.args.iter().filter(|a| a.contains(":/worldstone/") || a.contains(":/azerothcore/")).collect();
        assert_eq!(mounts.len(), 3, "{mounts:?}");
        assert!(mounts[0].ends_with(":/worldstone/config:ro"));
        assert!(mounts[1].contains("modules") && mounts[1].ends_with(":/azerothcore/etc/modules:ro"), "the module's file is only read from here: {}", mounts[1]);
        assert!(mounts[2].ends_with(":/worldstone/logs") && !mounts[2].ends_with(":ro"));
        assert!(text.ends_with("--entrypoint /azerothcore/bin/dbimport worldstone/wotlk-world:latest -c /worldstone/config/dbimport.conf"), "{text}");
        assert!(text.contains("-w /worldstone/config"));
    }

    #[test]
    fn schemas_are_created_and_emptied_in_the_games_own_character_set() {
        let ac = accounts_sql(&azerothcore(), &creds());
        assert!(ac.contains("CREATE DATABASE IF NOT EXISTS `acore_world` DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci;"), "{ac}");
        assert!(ac.contains("`acore_playerbots`") && ac.contains("GRANT SELECT, INSERT, UPDATE, DELETE, CREATE, DROP, ALTER, INDEX"));
        assert!(accounts_sql(&zero(), &creds()).contains("`mangos0` DEFAULT CHARACTER SET utf8mb3"), "Vanilla keeps its own");
        assert_eq!(
            recreate_sql("utf8mb4", "acore_world"),
            "DROP DATABASE IF EXISTS `acore_world`;\nCREATE DATABASE `acore_world` DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci;\n"
        );
    }

    #[test]
    fn ledger_keys_are_per_schema_and_quoted() {
        assert_eq!(plan().schemas[1].key("populate:a.sql"), "mangos0:populate:a.sql");
        assert!(ledger_insert("mangos0:setup:done").contains("VALUES ('mangos0:setup:done')"));
        assert!(ledger_insert("a'b").contains("'a''b'"));
    }
}
