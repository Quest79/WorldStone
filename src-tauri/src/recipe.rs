//! Emulator recipes: the declarative facts about one server, loaded from TOML.
//!
//! Values that later reach a shell command, a path or SQL are validated on load, so a
//! recipe cannot smuggle in a path traversal or an injected statement.

use crate::paths::{valid_component, valid_id};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

const BUILTIN: [&str; 2] = [include_str!("../recipes/mangos-zero.toml"), include_str!("../recipes/azerothcore.toml")];

#[derive(Deserialize, Clone, Debug)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub game: String,
    /// The one word people know this game edition by ("vanilla"). It names the Docker images,
    /// so `docker image ls` says what each one is: `worldstone/<edition>-<role>`.
    pub edition: String,
    pub client_build: u32,
    /// How this server looks in the app: the colours and emblem of its expansion. A recipe
    /// without one gets the app's own look.
    #[serde(default)]
    pub theme: Option<Theme>,
    pub repos: Vec<RepoSpec>,
    pub build: BuildSpec,
    pub playerbots: PlayerbotsSpec,
    pub accounts: AccountsSpec,
    pub client: ClientSpec,
    pub extract: ExtractSpec,
    pub database: DatabaseSpec,
    pub config: ConfigSpec,
    pub run: RunSpec,
}

/// The look of one expansion. The values go to the UI as colours and a name, so they are
/// checked on load: `#rrggbb` only, and an emblem name is a plain id.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct Theme {
    /// `#rrggbb`: the text, border and glow of the server's button.
    pub accent: String,
    /// `#rrggbb`: the dark tint the button fades from.
    pub deep: String,
    /// One of the emblems the UI knows (`swords`, `snowflake`, ...); an unknown name shows a shield.
    pub emblem: String,
    /// The scene drawn faintly behind the button (`azeroth`, `northrend`); none or an unknown name draws none.
    #[serde(default)]
    pub backdrop: Option<String>,
}

pub(crate) fn hex_colour(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Deserialize, Clone, Debug)]
pub struct RepoSpec {
    pub id: String,
    pub url: String,
    pub branch: String,
    /// Directory name under the instance's `Repos` folder; with `into`, the folder inside that repo.
    pub path: String,
    /// Id of another repository this one is cloned into, at `path` inside it. A module that must
    /// sit in the emulator's source tree is built as part of it, and is one source with it.
    #[serde(default)]
    pub into: Option<String>,
    /// Fetch submodules. On unless a recipe opts out: upstream builds always clone
    /// recursively, and a repo can hold real content in a submodule (the Zero database's
    /// whole Realm schema does), so leaving it off silently produces a broken checkout.
    #[serde(default = "yes")]
    pub submodules: bool,
}

fn yes() -> bool {
    true
}

fn nine() -> u32 {
    9
}

#[derive(Deserialize, Clone, Debug)]
pub struct BuildSpec {
    /// Id of the repository whose source is compiled.
    pub repo: String,
    /// Linux image the build stage and the runtime image both start from.
    pub base_image: String,
    /// apt packages needed to compile.
    pub build_packages: Vec<String>,
    /// apt packages the installed binaries need at run time.
    pub runtime_packages: Vec<String>,
    /// Absolute path inside the image that `cmake --install` writes to.
    pub install_prefix: String,
    #[serde(default)]
    pub cmake: BTreeMap<String, String>,
    /// Files, relative to the install prefix, the finished image must contain.
    #[serde(default)]
    pub required_files: Vec<String>,
    /// Programs, relative to the install prefix, that must run and exit 0 in the finished image.
    #[serde(default)]
    pub smoke: Vec<SmokeTest>,
    /// Folders of the source (relative to the built repository) the servers read at run time, such
    /// as SQL. They are kept in the image at `<install_prefix>/source/<folder>`.
    #[serde(default)]
    pub keep_source: Vec<String>,
}

/// Where the source folders named by `keep_source` are inside the image.
pub const KEPT_SOURCE_FOLDER: &str = "source";

#[derive(Deserialize, Clone, Debug)]
pub struct SmokeTest {
    pub run: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// How playerbots arrive in this emulator. Every supported emulator must declare it: the
/// server is only useful to the owner with bots built in, on and working.
#[derive(Deserialize, Clone, Debug)]
pub struct PlayerbotsSpec {
    /// A file, relative to the install prefix, that exists only when bots are compiled in.
    pub marker: String,
    /// Where the bots' characters are stored, which is how the app counts the bots that are in the
    /// world. The accounts they belong to are the recipe's `[accounts]` marked as bots.
    pub characters_schema: String,
    /// Characters the bots create on each of their accounts: one per class.
    #[serde(default = "nine")]
    pub classes_per_account: u32,
    pub characters_table: String,
    pub characters_account_column: String,
    /// Non-zero while the character is in the world.
    pub characters_online_column: String,
}

/// How the emulator stores accounts and how the console manages them. Everything here reaches SQL
/// or the server's console, so it is validated on load.
#[derive(Deserialize, Clone, Debug)]
pub struct AccountsSpec {
    pub schema: String,
    pub table: String,
    pub id_column: String,
    pub name_column: String,
    /// Column of the account table holding the game-master level. Exactly one of this and
    /// `gm_table` is given.
    #[serde(default)]
    pub gm_column: Option<String>,
    /// Separate table holding the level, for emulators that keep it apart from the account.
    #[serde(default)]
    pub gm_table: Option<GmTable>,
    pub created_column: String,
    pub last_login_column: String,
    /// Non-zero on the accounts the playerbots created for themselves. Left out when the bots'
    /// accounts carry no flag; they are then the accounts whose name starts with `bot_prefix`.
    #[serde(default)]
    pub bot_column: Option<String>,
    /// Upper-case name prefix of those accounts; a person cannot take it.
    pub bot_prefix: String,
    /// The column holding the password hash (for SRP6, the verifier).
    pub password_column: String,
    /// The column holding the SRP6 salt. Required for `srp6`, and only then.
    #[serde(default)]
    pub salt_column: Option<String>,
    pub password_hash: PasswordHash,
    pub name_max: usize,
    pub password_max: usize,
    /// The highest game-master level the console accepts.
    pub max_gm_level: u8,
    /// Console commands. `{name}`, `{password}` and `{level}` are filled in.
    pub create_command: String,
    pub password_command: String,
    pub gm_command: String,
}

/// Where an emulator keeps game-master levels apart from the account itself.
#[derive(Deserialize, Clone, Debug)]
pub struct GmTable {
    pub table: String,
    /// Column holding the account id.
    pub id_column: String,
    pub level_column: String,
}

/// How the database keeps an account's password, so the app can check it without reading it.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PasswordHash {
    /// SHA-1 of `NAME:PASSWORD`, both in upper case, as hex. The database can compute it.
    Sha1NamePassword,
    /// SRP6 as WoW 3.x uses it: a 32-byte salt and a verifier of the same length. Only the app can
    /// compute it, so the salt and verifier are read and compared here.
    Srp6,
}

/// Placeholders a console command may contain.
pub const ACCOUNT_TOKENS: [&str; 3] = ["{name}", "{password}", "{level}"];

/// The game client the user supplies.
#[derive(Deserialize, Clone, Debug)]
pub struct ClientSpec {
    /// The game executable, whose version identifies the build.
    pub executable: String,
    /// Folder inside the client that holds the MPQ archives.
    pub data_dir: String,
    /// Archives (in `data_dir`) a usable client of this game cannot lack.
    pub archives: Vec<String>,
    /// Archive that marks a language folder, with `{locale}` standing for its name.
    pub locale_archive: String,
    /// The file, in the client folder, holding the address of the realm list server the client
    /// logs in to, and the word before that address on its line (`set realmlist <address>`).
    /// `{locale}` stands for the client's language folder, for clients that keep the file there.
    pub realmlist_file: String,
    pub realmlist_key: String,
    /// The client builds this emulator supports.
    pub builds: Vec<ClientBuild>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientBuild {
    pub number: u32,
    /// The patch level players know it by, such as `1.12.1`.
    pub name: String,
}

/// How the emulator's own tool turns the client into the data the server reads.
#[derive(Deserialize, Clone, Debug)]
pub struct ExtractSpec {
    /// Program, relative to the install prefix, that a phase runs unless it names its own.
    pub tool: String,
    /// Arguments for every phase. `{client}`, `{src}`, `{dest}`, `{threads}` and `{prefix}` are
    /// filled in.
    #[serde(default)]
    pub args: Vec<String>,
    /// Files, relative to the install prefix, the tool reads and the image must therefore hold.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Folders in the output that must hold files when extraction is done.
    pub outputs: Vec<String>,
    /// Folders in the output that are only working files between phases. They are removed once
    /// the outputs are verified, so they never take room in the data the server reads.
    #[serde(default)]
    pub scratch: Vec<String>,
    pub phases: Vec<ExtractPhase>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ExtractPhase {
    pub id: String,
    pub label: String,
    /// This phase's own program, relative to the install prefix, for emulators whose extraction
    /// is several tools.
    #[serde(default)]
    pub tool: Option<String>,
    /// Run in the output folder, for tools that read and write there instead of taking paths.
    #[serde(default)]
    pub in_output: bool,
    /// Folders in the output removed before this phase starts, for tools that refuse to run over
    /// the leftovers of an attempt that was interrupted.
    #[serde(default)]
    pub clean: Vec<String>,
    /// The tool prints its own percentage at the start of a line (`37% ...`); it is shown as
    /// the phase's progress.
    #[serde(default)]
    pub count_percent: bool,
    /// Arguments that choose what this phase produces, before the shared ones.
    pub args: Vec<String>,
    /// Output lines starting with this (after leading spaces) each mean one unit finished.
    #[serde(default)]
    pub count_prefix: Option<String>,
    /// What those units are called, for display.
    #[serde(default)]
    pub count_unit: Option<String>,
}

/// The configuration files the servers read, generated from the ones the build installs.
#[derive(Deserialize, Clone, Debug)]
pub struct ConfigSpec {
    pub files: Vec<ConfigFile>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ConfigFile {
    pub id: String,
    pub label: String,
    /// The sample file the build installs, relative to the install prefix. Its name without the
    /// `.dist` ending is the name of the file WorldStone keeps in the instance's config folder.
    pub template: String,
    /// The service that reads this file. A file with none belongs to a tool the app runs itself
    /// (the database importer).
    #[serde(default)]
    pub service: Option<String>,
    /// Folder inside the instance's config folder the file is kept in, for a file the server
    /// insists on finding in a folder of its own.
    #[serde(default)]
    pub folder: Option<String>,
    /// Where in the image (relative to the install prefix) that folder appears. Required with
    /// `folder`, and only then.
    #[serde(default)]
    pub mount: Option<String>,
    /// Keys WorldStone owns, as `key = value`. Every other key in the file is left as the person
    /// wrote it. Values may hold the [`CONFIG_TOKENS`] placeholders.
    pub set: BTreeMap<String, String>,
}

impl ConfigFile {
    /// Where the file is kept inside the instance's config folder: its own name, in its own folder
    /// when the server insists on one.
    pub fn stored_at(&self) -> String {
        match &self.folder {
            Some(folder) => format!("{folder}/{}", self.file_name()),
            None => self.file_name().to_string(),
        }
    }

    /// `mangosd.conf` for the template `etc/mangosd.conf.dist`.
    pub fn file_name(&self) -> &str {
        let name = self.template.rsplit('/').next().unwrap_or(&self.template);
        name.strip_suffix(".dist").unwrap_or(name)
    }
}

/// What runs and how it is wired: one container per server program.
#[derive(Deserialize, Clone, Debug)]
pub struct RunSpec {
    /// Where the volume holding the extracted client data appears inside a container.
    pub data_mount: String,
    /// The folder in that volume that holds the data in use.
    pub data_folder: String,
    /// Where the generated configuration files appear (read-only).
    pub config_mount: String,
    /// Where the servers write their own log files.
    pub logs_mount: String,
    /// Started in this order, after the database.
    pub services: Vec<ServiceSpec>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ServiceSpec {
    pub id: String,
    pub label: String,
    /// What the program does, in one word ("realm", "world"); the second half of its image name.
    pub role: String,
    /// Program, relative to the install prefix.
    pub binary: String,
    /// Arguments; `{config}` is the path of this service's configuration file.
    #[serde(default)]
    pub args: Vec<String>,
    /// Port the program listens on inside its container; it is also published on the host.
    pub port: u16,
    /// Id of the configuration file this program is started with.
    pub config: String,
    /// Reads the extracted client data.
    #[serde(default)]
    pub data: bool,
    /// Has an interactive console on standard input.
    #[serde(default)]
    pub console: bool,
    /// What the console prints to ask for a command, so a reply can be told from the prompt.
    #[serde(default)]
    pub console_prompt: Option<String>,
    /// Seconds it gets to save and exit when stopped.
    #[serde(default = "default_grace")]
    pub stop_grace_secs: u32,
}

fn default_grace() -> u32 {
    30
}

/// Placeholders a configuration value may contain, and what they stand for.
pub const CONFIG_TOKENS: [&str; 14] = [
    "{db_host}", "{db_port}", "{db_user}", "{db_password}", "{data_dir}", "{logs_dir}", "{source_dir}", "{port}",
    "{realm_id}", "{player_limit}", "{bots_enabled}", "{bots_min}", "{bots_max}", "{bots_accounts}",
];

#[derive(Deserialize, Clone, Debug)]
pub struct DatabaseSpec {
    /// The public image the database runs from; its name is the engine's (`mariadb:11.4`).
    pub image: String,
    /// The database server the image holds. Some emulators only accept one of them.
    #[serde(default)]
    pub engine: DatabaseEngine,
    /// Id of the repository holding the SQL.
    pub repo: String,
    /// The one account the game servers connect with.
    pub user: String,
    /// Privileges that account gets in each schema.
    pub grants: Vec<String>,
    /// Extra arguments for the database server.
    #[serde(default)]
    pub server_args: Vec<String>,
    /// The character set the app creates the schemas with; the collation is its `_general_ci`.
    #[serde(default = "default_charset")]
    pub charset: String,
    /// How the schemas are filled and brought up to date.
    #[serde(default)]
    pub method: DatabaseMethod,
    /// The importer to run, for [`DatabaseMethod::ImportTool`].
    #[serde(default)]
    pub import: Option<ImportSpec>,
    pub realm: RealmSpec,
    pub schemas: Vec<SchemaSpec>,
}

fn default_charset() -> String {
    "utf8mb3".to_string()
}

/// The database server. The two are alike enough for the emulators' SQL, but the official images
/// name their programs and settings differently, and AzerothCore's importer refuses MariaDB.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseEngine {
    #[default]
    Mariadb,
    Mysql,
}

impl DatabaseEngine {
    /// The name of the official image, which the recipe's image must carry.
    pub fn image_name(self) -> &'static str {
        match self {
            Self::Mariadb => "mariadb",
            Self::Mysql => "mysql",
        }
    }

    /// The command-line client inside the image.
    pub fn client(self) -> &'static str {
        match self {
            Self::Mariadb => "mariadb",
            Self::Mysql => "mysql",
        }
    }

    /// The dump program inside the image.
    pub fn dump(self) -> &'static str {
        match self {
            Self::Mariadb => "mariadb-dump",
            Self::Mysql => "mysqldump",
        }
    }

    /// What the image reads on its first start: the root password and the host root may connect from.
    pub fn root_env(self) -> (&'static str, &'static str) {
        match self {
            Self::Mariadb => ("MARIADB_ROOT_PASSWORD", "MARIADB_ROOT_HOST"),
            Self::Mysql => ("MYSQL_ROOT_PASSWORD", "MYSQL_ROOT_HOST"),
        }
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseMethod {
    /// The app loads the repository's setup and update scripts itself, in order, and keeps a
    /// ledger of what it loaded.
    #[default]
    Scripts,
    /// The emulator's own tool, run from the built image, creates the tables and applies the
    /// updates; the app backs up first, runs it, and checks the result.
    ImportTool,
}

/// The emulator's database importer.
#[derive(Deserialize, Clone, Debug)]
pub struct ImportSpec {
    /// The service whose image and mounts the tool runs with (it must see the same configuration
    /// folders the server does).
    pub service: String,
    /// Program, relative to the install prefix.
    pub tool: String,
    /// Id of the configuration file (one with no service) the tool is started with.
    pub config: String,
    /// Arguments; `{config}` is the path of that file.
    #[serde(default)]
    pub args: Vec<String>,
}

/// The realm list entry players connect through.
#[derive(Deserialize, Clone, Debug)]
pub struct RealmSpec {
    pub schema: String,
    pub table: String,
    /// Script in the SQL repository that adds an entry; it is only run when the table is empty.
    /// Left out when the emulator's own data already holds the entry, which is then only checked.
    #[serde(default)]
    pub sql: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SchemaSpec {
    pub name: String,
    /// Folder in the SQL repository, holding `Setup/` and `Updates/`. Scripts method only.
    #[serde(default)]
    pub dir: Option<String>,
    /// File in `Setup/` that creates the tables. Scripts method only.
    #[serde(default)]
    pub setup: Option<String>,
    /// Created empty and granted by the app, then filled by the server itself the first time it
    /// runs, so its tables are not expected before that.
    #[serde(default)]
    pub self_managed: bool,
    /// Folder in `Setup/` of data files loaded after the tables exist.
    #[serde(default)]
    pub populate: Option<String>,
    /// Folders in `Updates/` whose scripts bring the schema up to date.
    #[serde(default)]
    pub updates: Vec<String>,
    /// Tables the finished schema must have.
    pub required_tables: Vec<String>,
}

/// The table every update records itself in.
pub const VERSION_TABLE: &str = "db_version";

/// Privileges a recipe may grant. Anything that reaches beyond the game's own schemas (`ALL`,
/// `FILE`, `GRANT OPTION`, `SUPER`) is deliberately absent.
const ALLOWED_GRANTS: [&str; 15] = [
    "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "INDEX", "LOCK TABLES",
    "CREATE ROUTINE", "ALTER ROUTINE", "EXECUTE", "CREATE TEMPORARY TABLES", "TRIGGER", "REFERENCES",
];

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RecipeSummary {
    pub id: String,
    pub name: String,
    pub game: String,
    pub client_build: u32,
    pub client_builds: Vec<ClientBuild>,
    pub theme: Option<Theme>,
    pub repos: Vec<RepoSummary>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RepoSummary {
    pub id: String,
    pub url: String,
    pub branch: String,
}

impl Recipe {
    pub fn parse(text: &str) -> Result<Self, String> {
        let recipe: Recipe = toml::from_str(text).map_err(|e| format!("Invalid recipe: {e}"))?;
        recipe.validate()?;
        Ok(recipe)
    }

    pub fn summary(&self) -> RecipeSummary {
        RecipeSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            game: self.game.clone(),
            client_build: self.client_build,
            client_builds: self.client.builds.clone(),
            theme: self.theme.clone(),
            repos: self
                .repos
                .iter()
                .map(|r| RepoSummary { id: r.id.clone(), url: r.url.clone(), branch: r.branch.clone() })
                .collect(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        let bad = |what: &str, value: &str| Err(format!("Recipe '{}': invalid {what} '{value}'", self.id));
        if !valid_id(&self.id) {
            return bad("recipe id", &self.id);
        }
        if !valid_id(&self.edition) {
            return bad("edition", &self.edition);
        }
        if let Some(theme) = &self.theme {
            if !hex_colour(&theme.accent) {
                return bad("theme accent (use #rrggbb)", &theme.accent);
            }
            if !hex_colour(&theme.deep) {
                return bad("theme deep colour (use #rrggbb)", &theme.deep);
            }
            if !valid_id(&theme.emblem) {
                return bad("theme emblem", &theme.emblem);
            }
            if let Some(backdrop) = theme.backdrop.as_deref().filter(|b| !valid_id(b)) {
                return bad("theme backdrop", backdrop);
            }
        }
        if self.repos.is_empty() {
            return Err(format!("Recipe '{}' lists no repositories", self.id));
        }
        let mut seen = HashSet::new();
        for repo in &self.repos {
            if !seen.insert(repo.id.as_str()) {
                return bad("duplicate repository id", &repo.id);
            }
            if !valid_id(&repo.id) {
                return bad("repository id", &repo.id);
            }
            match &repo.into {
                None if !valid_component(&repo.path) => return bad("repository path", &repo.path),
                // A nested repository sits somewhere inside an earlier one, and never inside Git's own folder.
                Some(parent) => {
                    let parent_ok = self.repos.iter().take_while(|r| r.id != repo.id).any(|r| &r.id == parent && r.into.is_none());
                    let path_ok = relative_path(&repo.path) && !repo.path.split('/').any(|part| part.eq_ignore_ascii_case(".git"));
                    if !parent_ok {
                        return bad("repository to clone into (it must be an earlier repository that is not itself nested)", parent);
                    }
                    if !path_ok {
                        return bad("nested repository path", &repo.path);
                    }
                }
                None => {}
            }
            if !repo.url.starts_with("https://") || repo.url.contains(char::is_whitespace) {
                return bad("repository url", &repo.url);
            }
            if repo.branch.is_empty() || repo.branch.starts_with('-') || !safe_text(&repo.branch, "/._-") {
                return bad("branch", &repo.branch);
            }
        }
        self.validate_database()?;
        for (key, value) in &self.build.cmake {
            let key_ok = !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
            // `+` is allowed for compiler names such as clang++.
            if !key_ok || value.is_empty() || !safe_text(value, "._/:+-") {
                return bad("CMake option", &format!("{key}={value}"));
            }
        }
        self.validate_build()?;
        self.validate_client()?;
        self.validate_extract()?;
        self.validate_run()?;
        self.validate_accounts()
    }

    /// Account and bot table names are interpolated into SQL, and console commands are sent to the
    /// server as one line of text, so both are restricted to a plain vocabulary.
    fn validate_accounts(&self) -> Result<(), String> {
        let bad = |what: &str, value: &str| Err(format!("Recipe '{}': invalid {what} '{value}'", self.id));
        let identifier = |name: &str| !name.is_empty() && name.len() <= 64 && safe_text(name, "_");
        let (a, bots) = (&self.accounts, &self.playerbots);
        if !self.database.schemas.iter().any(|s| s.name == a.schema) {
            return bad("accounts schema", &a.schema);
        }
        if !self.database.schemas.iter().any(|s| s.name == bots.characters_schema) {
            return bad("bot characters schema", &bots.characters_schema);
        }
        let optional = [
            ("account game-master column", a.gm_column.as_ref()),
            ("account bot column", a.bot_column.as_ref()),
            ("account salt column", a.salt_column.as_ref()),
            ("game-master table", a.gm_table.as_ref().map(|g| &g.table)),
            ("game-master account column", a.gm_table.as_ref().map(|g| &g.id_column)),
            ("game-master level column", a.gm_table.as_ref().map(|g| &g.level_column)),
        ];
        for (what, name) in optional.into_iter().filter_map(|(what, name)| Some((what, name?))) {
            if !identifier(name) {
                return bad(what, name);
            }
        }
        if a.gm_column.is_some() == a.gm_table.is_some() {
            return Err(format!("Recipe '{}': give the game-master level either as an account column or as a table, not both or neither", self.id));
        }
        if (a.password_hash == PasswordHash::Srp6) != a.salt_column.is_some() {
            return Err(format!("Recipe '{}': a salt column goes with the srp6 password hash, and only with it", self.id));
        }
        for (what, name) in [
            ("accounts table", &a.table),
            ("account id column", &a.id_column),
            ("account name column", &a.name_column),
            ("account created column", &a.created_column),
            ("account last-login column", &a.last_login_column),
            ("account password column", &a.password_column),
            ("bot characters table", &bots.characters_table),
            ("bot characters account column", &bots.characters_account_column),
            ("bot characters online column", &bots.characters_online_column),
        ] {
            if !identifier(name) {
                return bad(what, name);
            }
        }
        if bots.classes_per_account == 0 || bots.classes_per_account > 20 {
            return bad("characters per bot account", &bots.classes_per_account.to_string());
        }
        if a.bot_prefix.is_empty() || a.bot_prefix.len() > 8 || !a.bot_prefix.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) {
            return bad("bot account prefix", &a.bot_prefix);
        }
        if a.name_max == 0 || a.name_max > 32 || a.password_max == 0 || a.password_max > 32 {
            return bad("account name or password length", &format!("{}/{}", a.name_max, a.password_max));
        }
        if a.max_gm_level == 0 || a.max_gm_level > 9 {
            return bad("highest game-master level", &a.max_gm_level.to_string());
        }
        for (what, command) in [("create command", &a.create_command), ("password command", &a.password_command), ("game-master command", &a.gm_command)] {
            let mut rest = command.clone();
            for token in ACCOUNT_TOKENS {
                rest = rest.replace(token, "");
            }
            if command.trim().is_empty() || !safe_text(&rest, " -") || command.starts_with(' ') || command.contains("  ") {
                return bad(what, command);
            }
        }
        if !a.create_command.contains("{name}") || !a.create_command.contains("{password}") {
            return bad("create command (needs a name and a password)", &a.create_command);
        }
        if !a.password_command.contains("{name}") || !a.password_command.contains("{password}") {
            return bad("password command (needs a name and a password)", &a.password_command);
        }
        if !a.gm_command.contains("{name}") || !a.gm_command.contains("{level}") {
            return bad("game-master command (needs a name and a level)", &a.gm_command);
        }
        if !self.run.services.iter().any(|s| s.console) {
            return Err(format!("Recipe '{}' has account commands but no service with a console", self.id));
        }
        Ok(())
    }

    /// Configuration values are written into files and service arguments become a container
    /// command, so both are restricted to a plain vocabulary and the placeholders above.
    fn validate_run(&self) -> Result<(), String> {
        let bad = |what: &str, value: &str| Err(format!("Recipe '{}': invalid {what} '{value}'", self.id));
        let (run, config) = (&self.run, &self.config);
        for (what, mount) in [("data mount", &run.data_mount), ("config mount", &run.config_mount), ("logs mount", &run.logs_mount)] {
            if !absolute_path(mount) {
                return bad(what, mount);
            }
        }
        if !valid_component(&run.data_folder) || !safe_text(&run.data_folder, "._-") {
            return bad("data folder", &run.data_folder);
        }
        if run.services.is_empty() {
            return Err(format!("Recipe '{}' lists no services to run", self.id));
        }
        let mut services = HashSet::new();
        let mut roles = HashSet::new();
        for service in &run.services {
            if !valid_id(&service.id) || !services.insert(service.id.as_str()) {
                return bad("service id", &service.id);
            }
            // Each role is one image name, so two services cannot share one.
            if !valid_id(&service.role) || !roles.insert(service.role.as_str()) {
                return bad("service role", &service.role);
            }
            // Service ids become Compose service names next to the database's.
            if service.id == crate::compose::DATABASE_SERVICE {
                return bad("service id (taken by the database)", &service.id);
            }
            if service.label.is_empty() || !safe_text(&service.label, " -") {
                return bad("service label", &service.label);
            }
            if let Some(prompt) = service.console_prompt.as_ref().filter(|p| p.is_empty() || !safe_text(p, ">$#:")) {
                return bad("console prompt", prompt);
            }
            if !relative_path(&service.binary) {
                return bad("service program", &service.binary);
            }
            if service.port == 0 {
                return bad("service port", "0");
            }
            if service.stop_grace_secs == 0 || service.stop_grace_secs > 600 {
                return bad("stop grace period", &service.stop_grace_secs.to_string());
            }
            for arg in &service.args {
                if arg.is_empty() || !safe_text(&arg.replace("{config}", ""), "._/=:-") {
                    return bad("service argument", arg);
                }
            }
            if !config.files.iter().any(|f| f.id == service.config) {
                return bad("service configuration", &service.config);
            }
        }
        let mut files = HashSet::new();
        let mut names = HashSet::new();
        for file in &config.files {
            if !valid_id(&file.id) || !files.insert(file.id.as_str()) {
                return bad("configuration id", &file.id);
            }
            if file.label.is_empty() || !safe_text(&file.label, " -") {
                return bad("configuration label", &file.label);
            }
            if !relative_path(&file.template) || !file.template.ends_with(".dist") || !names.insert(file.file_name()) {
                return bad("configuration template", &file.template);
            }
            if let Some(service) = file.service.as_ref().filter(|s| !services.contains(s.as_str())) {
                return bad("configuration service", service);
            }
            match (&file.folder, &file.mount) {
                (None, None) => {}
                (Some(folder), Some(mount)) => {
                    if !valid_component(folder) || !safe_text(folder, "._-") {
                        return bad("configuration folder", folder);
                    }
                    if !relative_path(mount) {
                        return bad("configuration mount", mount);
                    }
                }
                _ => return Err(format!("Recipe '{}': configuration '{}' needs both a folder and a mount, or neither", self.id, file.id)),
            }
            if file.folder.is_some() && file.service.is_none() {
                return Err(format!("Recipe '{}': configuration '{}' has a mount but no service to mount it in", self.id, file.id));
            }
            if file.set.is_empty() {
                return Err(format!("Recipe '{}': configuration '{}' manages no keys", self.id, file.id));
            }
            for (key, value) in &file.set {
                if key.is_empty() || !safe_text(key, "._") {
                    return bad("configuration key", key);
                }
                let mut rest = value.clone();
                for token in CONFIG_TOKENS {
                    rest = rest.replace(token, "");
                }
                if value.is_empty() || !safe_text(&rest, " \"._/:;,=+-~") {
                    return bad("configuration value", value);
                }
            }
        }
        Ok(())
    }

    /// Names here are interpolated into SQL, a Compose file and shell scripts, so they are plain
    /// identifiers and file names, and privileges come from a short allowlist.
    fn validate_database(&self) -> Result<(), String> {
        let bad = |what: &str, value: &str| Err(format!("Recipe '{}': invalid {what} '{value}'", self.id));
        let db = &self.database;
        let identifier = |name: &str| !name.is_empty() && name.len() <= 64 && safe_text(name, "_");
        let folder = |name: &str| valid_component(name) && safe_text(name, "._-");
        if db.image.is_empty() || db.image.starts_with('-') || !safe_text(&db.image, "._/:@-") {
            return bad("database image", &db.image);
        }
        // The engine decides which programs and settings are used, so the image has to be one of its own.
        if !db.image.starts_with(&format!("{}:", db.engine.image_name())) {
            return Err(format!(
                "Recipe '{}': the database image '{}' is not a {} image",
                self.id,
                db.image,
                db.engine.image_name()
            ));
        }
        if !self.repos.iter().any(|r| r.id == db.repo) {
            return bad("database repository", &db.repo);
        }
        if !identifier(&db.user) || db.user.len() > 32 {
            return bad("database user", &db.user);
        }
        if db.grants.is_empty() {
            return Err(format!("Recipe '{}' grants the database user nothing", self.id));
        }
        if let Some(g) = db.grants.iter().find(|g| !ALLOWED_GRANTS.contains(&g.as_str())) {
            return bad("database privilege", g);
        }
        for arg in &db.server_args {
            if !arg.starts_with("--") || !safe_text(&arg[2..], "-_=.,:") {
                return bad("database server argument", arg);
            }
        }
        if db.schemas.is_empty() {
            return Err(format!("Recipe '{}' lists no database schemas", self.id));
        }
        if !identifier(&db.charset) {
            return bad("database character set", &db.charset);
        }
        let scripts = db.method == DatabaseMethod::Scripts;
        match (&db.import, scripts) {
            (Some(_), true) => return Err(format!("Recipe '{}': [database.import] is only for the import_tool method", self.id)),
            (None, false) => return Err(format!("Recipe '{}': the import_tool database method needs [database.import]", self.id)),
            (Some(import), false) => {
                if !relative_path(&import.tool) {
                    return bad("database import tool", &import.tool);
                }
                if !self.run.services.iter().any(|s| s.id == import.service) {
                    return bad("database import service", &import.service);
                }
                match self.config.files.iter().find(|f| f.id == import.config) {
                    Some(file) if file.service.is_none() => {}
                    _ => return bad("database import configuration (a configuration file with no service)", &import.config),
                }
                for arg in &import.args {
                    if arg.is_empty() || !safe_text(&arg.replace("{config}", ""), "._/=:-") {
                        return bad("database import argument", arg);
                    }
                }
            }
            (None, true) => {}
        }
        let mut seen = HashSet::new();
        for schema in &db.schemas {
            if !identifier(&schema.name) || !seen.insert(schema.name.as_str()) {
                return bad("schema name", &schema.name);
            }
            match (&schema.dir, &schema.setup, scripts) {
                (Some(dir), Some(setup), true) => {
                    if !folder(dir) {
                        return bad("schema directory", dir);
                    }
                    if !folder(setup) || !setup.ends_with(".sql") {
                        return bad("schema setup file", setup);
                    }
                }
                (_, _, true) => return Err(format!("Recipe '{}': schema '{}' needs a directory and a setup file", self.id, schema.name)),
                (None, None, false) => {}
                (_, _, false) => return Err(format!("Recipe '{}': schema '{}' names scripts, but the import tool loads it", self.id, schema.name)),
            }
            if schema.self_managed && (schema.populate.is_some() || !schema.updates.is_empty()) {
                return Err(format!("Recipe '{}': schema '{}' is filled by the server, so it lists no scripts", self.id, schema.name));
            }
            if let Some(p) = schema.populate.iter().find(|p| !folder(p)) {
                return bad("schema data folder", p);
            }
            if let Some(u) = schema.updates.iter().find(|u| !folder(u)) {
                return bad("schema update folder", u);
            }
            if let Some(t) = schema.required_tables.iter().find(|t| !identifier(t)) {
                return bad("required table", t);
            }
            // Whether an update ran is read from the version table, so it must be there.
            if scripts && !schema.updates.is_empty() && !schema.required_tables.iter().any(|t| t == VERSION_TABLE) {
                return Err(format!("Recipe '{}': schema '{}' has updates but does not require {VERSION_TABLE}", self.id, schema.name));
            }
        }
        let realm = &db.realm;
        if !db.schemas.iter().any(|s| s.name == realm.schema) {
            return bad("realm schema", &realm.schema);
        }
        if !identifier(&realm.table) {
            return bad("realm table", &realm.table);
        }
        match &realm.sql {
            Some(sql) if !relative_path(sql) || !sql.ends_with(".sql") => return bad("realm script", sql),
            Some(_) if !scripts => return Err(format!("Recipe '{}': the realm script belongs to the scripts method", self.id)),
            None if scripts => return Err(format!("Recipe '{}': the scripts method needs a realm script", self.id)),
            _ => {}
        }
        Ok(())
    }

    /// Where a repository's checkout is, relative to the instance's `Repos` folder: its own folder,
    /// or its place inside the repository it is cloned into.
    pub fn repo_folder(&self, repo: &RepoSpec) -> String {
        match repo.into.as_ref().and_then(|parent| self.repos.iter().find(|r| &r.id == parent)) {
            Some(parent) => format!("{}/{}", parent.path, repo.path),
            None => repo.path.clone(),
        }
    }

    /// The repositories cloned into `repo`.
    pub fn nested_in<'a>(&'a self, repo: &'a RepoSpec) -> impl Iterator<Item = &'a RepoSpec> {
        self.repos.iter().filter(move |r| r.into.as_deref() == Some(repo.id.as_str()))
    }

    /// The repository the database scripts come from. Validation guarantees it exists.
    pub fn database_repo(&self) -> &RepoSpec {
        self.repos
            .iter()
            .find(|r| r.id == self.database.repo)
            .expect("validated on load")
    }

    fn validate_client(&self) -> Result<(), String> {
        let bad = |what: &str, value: &str| Err(format!("Recipe '{}': invalid {what} '{value}'", self.id));
        let client = &self.client;
        let file_name = |name: &str| valid_component(name) && safe_text(name, "._-");
        if !file_name(&client.executable) {
            return bad("client executable", &client.executable);
        }
        if !file_name(&client.data_dir) {
            return bad("client data folder", &client.data_dir);
        }
        if client.archives.is_empty() {
            return Err(format!("Recipe '{}' lists no required client archives", self.id));
        }
        if let Some(a) = client.archives.iter().find(|a| !file_name(a)) {
            return bad("client archive", a);
        }
        if !client.locale_archive.contains("{locale}") || !file_name(&client.locale_archive.replace("{locale}", "xxXX")) {
            return bad("locale archive", &client.locale_archive);
        }
        if !relative_path(&client.realmlist_file.replace("{locale}", "xxXX")) {
            return bad("client realm list file", &client.realmlist_file);
        }
        if client.realmlist_key.is_empty() || !safe_text(&client.realmlist_key, "") {
            return bad("client realm list key", &client.realmlist_key);
        }
        if client.builds.is_empty() {
            return Err(format!("Recipe '{}' lists no supported client builds", self.id));
        }
        if !client.builds.iter().any(|b| b.number == self.client_build) {
            return bad("primary client build (not among the supported builds)", &self.client_build.to_string());
        }
        if let Some(b) = client.builds.iter().find(|b| b.name.is_empty() || !safe_text(&b.name, ". ")) {
            return bad("client build name", &b.name);
        }
        Ok(())
    }

    /// Extraction arguments reach a container as separate arguments, never through a shell
    /// string, but they are still restricted to a plain vocabulary.
    fn validate_extract(&self) -> Result<(), String> {
        let bad = |what: &str, value: &str| Err(format!("Recipe '{}': invalid {what} '{value}'", self.id));
        let extract = &self.extract;
        if !relative_path(&extract.tool) {
            return bad("extraction tool", &extract.tool);
        }
        if let Some(i) = extract.inputs.iter().find(|i| !relative_path(i)) {
            return bad("extraction input", i);
        }
        if let Some(t) = extract.phases.iter().filter_map(|p| p.tool.as_ref()).find(|t| !relative_path(t)) {
            return bad("extraction tool", t);
        }
        if let Some(c) = extract.phases.iter().flat_map(|p| &p.clean).find(|c| !valid_component(c) || !safe_text(c, "._-")) {
            return bad("extraction clean-up folder", c);
        }
        if let Some(o) = extract.scratch.iter().find(|o| !valid_component(o) || !safe_text(o, "._-") || extract.outputs.contains(o)) {
            return bad("extraction scratch folder", o);
        }
        if extract.outputs.is_empty() {
            return Err(format!("Recipe '{}' lists no extraction outputs", self.id));
        }
        if let Some(o) = extract.outputs.iter().find(|o| !valid_component(o) || !safe_text(o, "._-")) {
            return bad("extraction output", o);
        }
        if extract.phases.is_empty() {
            return Err(format!("Recipe '{}' lists no extraction phases", self.id));
        }
        let mut seen = HashSet::new();
        for phase in &extract.phases {
            if !valid_id(&phase.id) || !seen.insert(phase.id.as_str()) {
                return bad("extraction phase id", &phase.id);
            }
            if phase.label.is_empty() || !safe_text(&phase.label, " -") {
                return bad("extraction phase label", &phase.label);
            }
            for prefix in phase.count_prefix.iter().chain(&phase.count_unit) {
                if prefix.is_empty() || !safe_text(prefix, " -") {
                    return bad("extraction phase count text", prefix);
                }
            }
        }
        for arg in extract.args.iter().chain(extract.phases.iter().flat_map(|p| &p.args)) {
            if !extraction_arg(arg) {
                return bad("extraction argument", arg);
            }
        }
        Ok(())
    }

    /// Everything in the build section ends up in a generated Dockerfile, so it is restricted
    /// to characters that cannot break out of a shell word.
    fn validate_build(&self) -> Result<(), String> {
        let bad = |what: &str, value: &str| Err(format!("Recipe '{}': invalid {what} '{value}'", self.id));
        let build = &self.build;
        if !self.repos.iter().any(|r| r.id == build.repo) {
            return bad("build repository", &build.repo);
        }
        if build.base_image.is_empty() || build.base_image.starts_with('-') || !safe_text(&build.base_image, "._/:@-") {
            return bad("base image", &build.base_image);
        }
        for (what, packages) in [("build package", &build.build_packages), ("runtime package", &build.runtime_packages)] {
            if packages.is_empty() {
                return Err(format!("Recipe '{}' lists no {what}s", self.id));
            }
            if let Some(p) = packages.iter().find(|p| !package_name(p)) {
                return bad(what, p);
            }
        }
        if !absolute_path(&build.install_prefix) {
            return bad("install prefix", &build.install_prefix);
        }
        let files = build
            .required_files
            .iter()
            .chain([&self.playerbots.marker])
            .chain(build.smoke.iter().map(|s| &s.run));
        for file in files {
            if !relative_path(file) {
                return bad("installed file", file);
            }
        }
        if let Some(folder) = build.keep_source.iter().find(|f| !relative_path(f)) {
            return bad("kept source folder", folder);
        }
        for arg in build.smoke.iter().flat_map(|s| &s.args) {
            if arg.is_empty() || !safe_text(arg, "._/=:-") {
                return bad("smoke-test argument", arg);
            }
        }
        Ok(())
    }

    /// The repository the build compiles. Validation guarantees it exists.
    pub fn build_repo(&self) -> &RepoSpec {
        self.repos
            .iter()
            .find(|r| r.id == self.build.repo)
            .expect("validated on load")
    }

    /// The service whose image the build tools, the sample configuration files and the client-data
    /// extractors are read from: the one that reads the extracted data, else the first. Every
    /// service image holds the whole install, so any would do; this one is named for the job.
    pub fn tools_service(&self) -> &ServiceSpec {
        self.run.services.iter().find(|s| s.data).unwrap_or(&self.run.services[0])
    }
}

/// ASCII letters and digits plus the listed extra characters.
fn safe_text(text: &str, extra: &str) -> bool {
    text.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// Debian package names: start alphanumeric, then letters, digits and `+ . -`.
fn package_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphanumeric()) && safe_text(name, "+.-")
}

/// Placeholders an extraction argument may contain.
const PLACEHOLDERS: [&str; 5] = ["{client}", "{src}", "{dest}", "{threads}", "{prefix}"];

/// A plain argument or one built from the known placeholders; nothing else can carry braces.
fn extraction_arg(arg: &str) -> bool {
    let mut rest = arg.to_string();
    for placeholder in PLACEHOLDERS {
        rest = rest.replace(placeholder, "");
    }
    !arg.is_empty() && safe_text(&rest, "._/=:-")
}

fn path_parts_ok(path: &str) -> bool {
    safe_text(path, "._/-") && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

fn relative_path(path: &str) -> bool {
    !path.starts_with('/') && path_parts_ok(path)
}

fn absolute_path(path: &str) -> bool {
    path.strip_prefix('/').is_some_and(path_parts_ok)
}

pub fn builtin() -> Result<Vec<Recipe>, String> {
    BUILTIN.iter().map(|text| Recipe::parse(text)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_mangos_zero_recipe_is_valid() {
        let recipes = builtin().unwrap();
        let zero = recipes.iter().find(|r| r.id == "mangos-zero").unwrap();
        assert_eq!(zero.client_build, 5875);
        assert_eq!(zero.repos.len(), 2);
        assert!(zero.repos.iter().all(|r| r.submodules), "both repos need their submodules");
        assert_eq!(zero.run.services.iter().map(|s| s.port).collect::<Vec<_>>(), [3724, 8085]);
        assert_eq!(zero.database.schemas.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["realmd", "character0", "mangos0"]);
        assert_eq!(zero.build.cmake["BUILD_TOOLS"], "ON");
    }

    #[test]
    fn mangos_zero_builds_with_playerbots_and_proves_it() {
        let zero = &builtin().unwrap()[0];
        assert_eq!(zero.build.cmake["PLAYERBOTS"], "ON", "playerbots are the point of the server");
        assert_eq!(zero.playerbots.marker, "etc/aiplayerbot.conf.dist");
        assert_eq!(zero.build_repo().id, "server");
        assert_eq!(zero.build.smoke.len(), 2);
    }

    #[test]
    fn the_world_server_keeps_no_gm_log_because_it_would_record_new_passwords() {
        let zero = &builtin().unwrap()[0];
        let world = zero.config.files.iter().find(|f| f.id == "mangosd").unwrap();
        assert_eq!(world.set["GmLogFile"], "\"\"");
    }

    fn with(replace: (&str, &str)) -> Result<Recipe, String> {
        Recipe::parse(&BUILTIN[0].replace(replace.0, replace.1))
    }

    #[test]
    fn rejects_repository_paths_that_escape_the_repos_folder() {
        assert!(with(("path = \"mangoszero\"", "path = \"../evil\"")).is_err());
        assert!(with(("path = \"mangoszero\"", "path = \"a/b\"")).is_err());
    }

    #[test]
    fn rejects_non_https_urls_and_option_like_branches() {
        assert!(with(("https://github.com/mangoszero/server.git", "file:///C:/x")).is_err());
        assert!(with(("branch = \"master\"", "branch = \"--upload-pack=x\"")).is_err());
    }

    #[test]
    fn rejects_schema_names_that_could_inject_sql() {
        assert!(with(("name = \"realmd\"", "name = \"realmd; DROP DATABASE x\"")).is_err());
        assert!(with(("name = \"realmd\"", "name = \"a`b\"")).is_err());
    }

    #[test]
    fn rejects_cmake_options_that_could_inject_arguments() {
        assert!(with(("BUILD_TOOLS = \"ON\"", "BUILD_TOOLS = \"ON -DEVIL=1\"")).is_err());
        assert!(with(("BUILD_TOOLS", "build-tools")).is_err());
    }

    #[test]
    fn rejects_build_values_that_could_break_out_of_the_generated_dockerfile() {
        for (from, to) in [
            ("base_image = \"ubuntu:22.04\"", "base_image = \"ubuntu:22.04\\nRUN evil\""),
            ("\"clang\", \"cmake\"", "\"clang\", \"cmake && evil\""),
            ("\"libssl3\"", "\"$(evil)\""),
            ("install_prefix = \"/mangos\"", "install_prefix = \"/mangos/../etc\""),
            ("install_prefix = \"/mangos\"", "install_prefix = \"mangos\""),
            ("\"bin/mangosd\"", "\"../bin/mangosd\""),
            ("marker = \"etc/aiplayerbot.conf.dist\"", "marker = \"/etc/passwd\""),
            ("args = [\"--version\"]", "args = [\"--version; evil\"]"),
            ("repo = \"server\"", "repo = \"nope\""),
        ] {
            assert!(with((from, to)).is_err(), "{to} should be rejected");
        }
    }

    #[test]
    fn mangos_zero_declares_its_client_and_the_extraction_it_needs() {
        let zero = &builtin().unwrap()[0];
        assert_eq!(zero.client.builds.iter().map(|b| b.number).collect::<Vec<_>>(), [5875, 6005, 6141]);
        assert_eq!(zero.client.builds[0].name, "1.12.1");
        assert_eq!(zero.extract.outputs, ["dbc", "tiles", "gomodels", "mmaps"]);
        let phases: Vec<_> = zero.extract.phases.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(phases, ["dbc", "gomodels", "trans", "tile", "nav"], "the extractor's own `all` order");
        assert!(zero.extract.args.contains(&"--no-menu".to_string()), "an unattended run must not wait for a keypress");
    }

    #[test]
    fn rejects_client_and_extraction_values_that_could_escape() {
        for (from, to) in [
            ("executable = \"WoW.exe\"", "executable = \"../WoW.exe\""),
            ("data_dir = \"Data\"", "data_dir = \"Data/..\""),
            ("\"dbc.MPQ\"", "\"dbc.MPQ; evil\""),
            ("locale_archive = \"locale-{locale}.MPQ\"", "locale_archive = \"locale.MPQ\""),
            ("client_build = 5875", "client_build = 9999"),
            ("tool = \"bin/tools/mangos-extractor\"", "tool = \"/bin/sh\""),
            ("\"{src}\"", "\"$(evil)\""),
            ("\"{dest}\"", "\"{unknown}\""),
            ("outputs = [\"dbc\"", "outputs = [\"../dbc\""),
            ("id = \"nav\"", "id = \"dbc\""),
        ] {
            assert!(with((from, to)).is_err(), "{to} should be rejected");
        }
    }

    #[test]
    fn mangos_zero_declares_what_a_finished_database_holds() {
        let zero = &builtin().unwrap()[0];
        let db = &zero.database;
        assert_eq!(zero.database_repo().id, "database");
        assert_eq!(db.realm.schema, "realmd");
        let world = db.schemas.iter().find(|s| s.name == "mangos0").unwrap();
        assert_eq!(world.populate.as_deref(), Some("FullDB"));
        let characters = db.schemas.iter().find(|s| s.name == "character0").unwrap();
        assert!(characters.required_tables.iter().any(|t| t == "ai_playerbot_names"), "a database without the bot tables is not done");
        assert!(db.schemas.iter().all(|s| s.required_tables.iter().any(|t| t == VERSION_TABLE)));
    }

    #[test]
    fn rejects_database_values_that_could_inject_sql_or_shell_or_over_grant() {
        for (from, to) in [
            ("user = \"mangos\"", "user = \"mangos'; DROP USER x; --\""),
            ("\"SELECT\", \"INSERT\"", "\"ALL PRIVILEGES\", \"INSERT\""),
            ("\"SELECT\", \"INSERT\"", "\"SELECT\", \"FILE\""),
            ("\"--max-allowed-packet=128M\"", "\"--max-allowed-packet=128M; evil\""),
            ("\"--max-allowed-packet=128M\"", "\"max-allowed-packet=128M\""),
            ("image = \"mariadb:11.4\"", "image = \"-v /:/host\""),
            ("repo = \"database\"\n# The one", "repo = \"nope\"\n# The one"),
            ("populate = \"FullDB\"", "populate = \"../FullDB\""),
            ("updates = [\"Rel21\", \"Rel22\"]\nrequired_tables = [\"db_version\", \"realmlist\"", "updates = [\"Rel21/..\", \"Rel22\"]\nrequired_tables = [\"db_version\", \"realmlist\""),
            ("\"creature_template\"", "\"creature_template`; DROP\""),
            ("sql = \"Tools/updateRealm.sql\"", "sql = \"/etc/passwd.sql\""),
            ("sql = \"Tools/updateRealm.sql\"", "sql = \"Tools/../../x.sql\""),
            ("schema = \"realmd\"\ntable", "schema = \"nope\"\ntable"),
        ] {
            assert!(with((from, to)).is_err(), "{to} should be rejected");
        }
    }

    #[test]
    fn mangos_zero_runs_the_realm_server_then_the_world_server_with_playerbots_on() {
        let zero = &builtin().unwrap()[0];
        let ids: Vec<&str> = zero.run.services.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["realmd", "mangosd"]);
        let world = &zero.run.services[1];
        assert!(world.data && world.console && world.stop_grace_secs >= 60, "the world server reads the data, has a console and needs time to save");
        assert!(!zero.run.services[0].data && !zero.run.services[0].console);
        let bots = zero.config.files.iter().find(|f| f.id == "aiplayerbot").expect("a playerbot configuration is managed");
        assert_eq!(bots.set["AiPlayerbot.Enabled"], "{bots_enabled}");
        assert_eq!(bots.set["AiPlayerbot.RandomBotAutologin"], "1", "bots log in by themselves");
        assert_eq!(zero.config.files.iter().map(ConfigFile::file_name).collect::<Vec<_>>(), ["realmd.conf", "mangosd.conf", "aiplayerbot.conf"]);
    }

    #[test]
    fn mangos_zero_is_vanilla_and_its_two_servers_are_the_realm_and_the_world() {
        let zero = &builtin().unwrap()[0];
        assert_eq!(zero.edition, "vanilla");
        assert_eq!(zero.run.services.iter().map(|s| s.role.as_str()).collect::<Vec<_>>(), ["realm", "world"]);
        assert_eq!(zero.tools_service().id, "mangosd", "the world server's image is the one the tools come from");
    }

    #[test]
    fn rejects_run_and_configuration_values_that_could_break_a_file_or_a_command() {
        for (from, to) in [
            ("RealmServerPort = \"{port}\"", "RealmServerPort = \"$(evil)\""),
            ("RealmServerPort = \"{port}\"", "RealmServerPort = \"{nope}\""),
            ("bin/realmd", "../bin/realmd"),
            ("args = [\"-c\", \"{config}\"]\nport = 3724", "args = [\"-c\", \"{config}; evil\"]\nport = 3724"),
            ("service = \"realmd\"\n\n[config.files.set]", "service = \"ghost\"\n\n[config.files.set]"),
            ("config = \"realmd\"", "config = \"ghost\""),
            ("port = 3724", "port = 0"),
            ("stop_grace_secs = 120", "stop_grace_secs = 0"),
            ("stop_grace_secs = 120", "stop_grace_secs = 9999"),
            ("logs_mount = \"/worldstone/logs\"", "logs_mount = \"logs\""),
            ("data_folder = \"current\"", "data_folder = \"../current\""),
            ("template = \"etc/realmd.conf.dist\"", "template = \"etc/realmd.conf\""),
            ("id = \"mangosd\"\nlabel = \"World server\"\nrole", "id = \"realmd\"\nlabel = \"World server\"\nrole"),
            ("id = \"realmd\"\nlabel = \"Realm server\"\n# The second half", "id = \"database\"\nlabel = \"Realm server\"\n# The second half"),
            ("role = \"realm\"", "role = \"world\""),
            ("role = \"realm\"", "role = \"Realm/x\""),
            ("edition = \"vanilla\"", "edition = \"Vanilla 1.12\""),
        ] {
            assert!(with((from, to)).is_err(), "{to} should be rejected");
        }
    }

    #[test]
    fn mangos_zero_declares_how_accounts_and_bots_are_stored_and_managed() {
        let zero = &builtin().unwrap()[0];
        assert_eq!((zero.accounts.schema.as_str(), zero.accounts.table.as_str()), ("realmd", "account"));
        assert_eq!(zero.accounts.bot_prefix, "RNDBOT");
        assert_eq!(zero.accounts.password_hash, PasswordHash::Sha1NamePassword);
        assert_eq!(zero.playerbots.characters_schema, "character0");
        assert_eq!((zero.client.realmlist_file.as_str(), zero.client.realmlist_key.as_str()), ("realmlist.wtf", "realmlist"));
    }

    #[test]
    fn rejects_account_and_bot_values_that_could_inject_sql_or_console_commands() {
        for (from, to) in [
            ("table = \"account\"\nid_column", "table = \"account`; DROP\"\nid_column"),
            ("name_column = \"username\"", "name_column = \"user name\""),
            ("bot_column = \"playerBot\"", "bot_column = \"\""),
            ("bot_prefix = \"RNDBOT\"", "bot_prefix = \"rndbot\""),
            ("bot_prefix = \"RNDBOT\"", "bot_prefix = \"\""),
            ("schema = \"realmd\"\ntable = \"account\"", "schema = \"ghost\"\ntable = \"account\""),
            ("characters_schema = \"character0\"", "characters_schema = \"ghost\""),
            ("characters_table = \"characters\"", "characters_table = \"chars;\""),
            ("name_max = 16", "name_max = 0"),
            ("password_max = 16", "password_max = 99"),
            ("max_gm_level = 3", "max_gm_level = 0"),
            ("create_command = \"account create {name} {password}\"", "create_command = \"account create {name} {password}; quit\""),
            ("create_command = \"account create {name} {password}\"", "create_command = \"account create {name}\""),
            ("create_command = \"account create {name} {password}\"", "create_command = \"account  create {name} {password}\""),
            ("gm_command = \"account set gmlevel {name} {level}\"", "gm_command = \"account set gmlevel {name}\""),
            ("password_command = \"account set password {name} {password} {password}\"", "password_command = \"account set password {name} {nope}\""),
            ("password_hash = \"sha1_name_password\"", "password_hash = \"plain\""),
        ] {
            assert!(with((from, to)).is_err(), "{to} should be rejected");
        }
    }

    #[test]
    fn a_schema_with_updates_must_require_the_version_table_they_are_tracked_in() {
        assert!(with(("required_tables = [\"db_version\", \"realmlist\", \"account\"]", "required_tables = [\"realmlist\"]")).is_err());
    }

    fn azerothcore() -> Recipe {
        builtin().unwrap().into_iter().find(|r| r.id == "azerothcore").unwrap()
    }

    fn ac_with(replace: (&str, &str)) -> Result<Recipe, String> {
        Recipe::parse(&BUILTIN[1].replace(replace.0, replace.1))
    }

    #[test]
    fn the_bundled_azerothcore_recipe_is_valid_and_builds_the_bots_into_the_server() {
        let ac = azerothcore();
        assert_eq!((ac.edition.as_str(), ac.client_build), ("wotlk", 12340));
        assert_eq!(ac.repos.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["server", "playerbots"]);
        let bots = &ac.repos[1];
        assert_eq!((bots.into.as_deref(), bots.path.as_str()), (Some("server"), "modules/mod-playerbots"));
        assert_eq!(ac.build.cmake["MODULES"], "static", "the module is compiled into the world server");
        assert_eq!(ac.playerbots.marker, "etc/modules/playerbots.conf.dist");
        assert_eq!(ac.playerbots.classes_per_account, 10, "death knights too");
        assert_eq!(ac.build.keep_source, ["data/sql", "modules/mod-playerbots/data/sql"], "the importer reads the scripts at run time");
        assert_eq!(ac.build_repo().id, "server");
        assert_eq!(ac.database_repo().id, "server");
    }

    #[test]
    fn azerothcore_runs_the_auth_server_then_the_world_server_and_imports_with_its_own_tool() {
        let ac = azerothcore();
        assert_eq!(ac.run.services.iter().map(|s| (s.id.as_str(), s.role.as_str(), s.port)).collect::<Vec<_>>(), [("authserver", "auth", 3724), ("worldserver", "world", 8085)]);
        assert_eq!(ac.tools_service().id, "worldserver");
        assert_eq!(ac.run.services[1].console_prompt.as_deref(), Some("AC>"));
        assert_eq!(ac.database.method, DatabaseMethod::ImportTool);
        assert_eq!(ac.database.charset, "utf8mb4");
        let import = ac.database.import.as_ref().expect("the import tool is named");
        assert_eq!((import.service.as_str(), import.tool.as_str(), import.config.as_str()), ("worldserver", "bin/dbimport", "dbimport"));
        assert!(ac.database.schemas.iter().find(|s| s.name == "acore_playerbots").unwrap().self_managed);
        assert!(ac.database.schemas.iter().filter(|s| !s.self_managed).all(|s| s.required_tables.iter().any(|t| t == "updates")));
    }

    #[test]
    fn azerothcore_keeps_the_bots_configuration_where_the_world_server_reads_modules_from() {
        let ac = azerothcore();
        let bots = ac.config.files.iter().find(|f| f.id == "playerbots").unwrap();
        assert_eq!(bots.stored_at(), "modules/playerbots.conf");
        assert_eq!(bots.mount.as_deref(), Some("etc/modules"));
        assert_eq!(bots.set["AiPlayerbot.Enabled"], "{bots_enabled}");
        let import = ac.config.files.iter().find(|f| f.id == "dbimport").unwrap();
        assert!(import.service.is_none(), "the importer's file belongs to no server");
        assert_eq!(import.set["Updates.EnableDatabases"], "7");
        for server in ["authserver", "worldserver"] {
            let file = ac.config.files.iter().find(|f| f.id == server).unwrap();
            assert_eq!(file.set["Updates.EnableDatabases"], "0", "only the app changes the databases, after a backup");
        }
    }

    #[test]
    fn azerothcore_keeps_passwords_as_srp6_and_bots_by_name_and_finds_the_realm_list_in_the_language_folder() {
        let ac = azerothcore();
        assert_eq!(ac.accounts.password_hash, PasswordHash::Srp6);
        assert_eq!(ac.accounts.salt_column.as_deref(), Some("salt"));
        assert!(ac.accounts.gm_table.is_some() && ac.accounts.gm_column.is_none(), "the level is in its own table");
        assert!(ac.accounts.bot_column.is_none() && ac.accounts.bot_prefix == "RNDBOT");
        assert_eq!(ac.client.realmlist_file, "Data/{locale}/realmlist.wtf");
        assert_eq!(ac.extract.phases.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["maps", "models", "vmaps", "mmaps"]);
        assert!(ac.extract.phases.iter().find(|p| p.id == "mmaps").unwrap().count_percent);
    }

    #[test]
    fn rejects_azerothcore_values_that_could_break_a_file_or_a_command() {
        for (from, to) in [
            ("charset = \"utf8mb4\"", "charset = \"utf8mb4; DROP\""),
            ("tool = \"bin/dbimport\"\nconfig", "tool = \"../bin/dbimport\"\nconfig"),
            ("tool = \"bin/dbimport\"\nconfig", "tool = \"/bin/sh\"\nconfig"),
            ("config = \"dbimport\"\nargs", "config = \"ghost\"\nargs"),
            ("args = [\"-c\", \"{config}\"]\n\n# The auth database", "args = [\"-c\", \"{config}; evil\"]\n\n# The auth database"),
            ("service = \"worldserver\"\ntool", "service = \"ghost\"\ntool"),
            ("keep_source = [\"data/sql\"", "keep_source = [\"../data/sql\""),
            ("into = \"server\"", "into = \"ghost\""),
            ("salt_column = \"salt\"", "salt_column = \"salt; DROP\""),
            ("password_hash = \"srp6\"", "password_hash = \"plain\""),
            ("mount = \"etc/modules\"", "mount = \"../etc\""),
            ("folder = \"modules\"", "folder = \"../modules\""),
            ("console_prompt = \"AC>\"", "console_prompt = \"AC>; evil\""),
            ("realmlist_file = \"Data/{locale}/realmlist.wtf\"", "realmlist_file = \"../{locale}/realmlist.wtf\""),
        ] {
            assert!(ac_with((from, to)).is_err(), "{to} should be rejected");
        }
    }

    #[test]
    fn azerothcore_runs_on_mysql_and_zero_on_mariadb_and_the_image_must_match_the_engine() {
        assert_eq!(azerothcore().database.engine, DatabaseEngine::Mysql);
        assert!(azerothcore().database.image.starts_with("mysql:"));
        assert_eq!(builtin().unwrap()[0].database.engine, DatabaseEngine::Mariadb, "the default, so Zero says nothing");
        let wrong = ac_with(("image = \"mysql:8.4\"", "image = \"mariadb:11.4\""));
        assert!(wrong.unwrap_err().contains("not a mysql image"));
        assert!(ac_with(("engine = \"mysql\"", "engine = \"postgres\"")).is_err(), "an unknown engine is refused");
    }

    #[test]
    fn a_password_hash_that_needs_a_salt_names_where_it_is_kept() {
        assert!(ac_with(("salt_column = \"salt\"\n", "")).is_err(), "SRP6 cannot be checked without the salt");
    }

    #[test]
    fn every_bundled_recipe_names_its_own_images_containers_and_edition() {
        let all = builtin().unwrap();
        let editions: Vec<&str> = all.iter().map(|r| r.edition.as_str()).collect();
        assert_eq!(editions, ["vanilla", "wotlk"]);
        let ids: std::collections::HashSet<&str> = all.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), all.len(), "recipe ids are unique");
    }

    #[test]
    fn every_bundled_recipe_has_its_own_theme_and_a_bad_one_is_rejected() {
        let all = builtin().unwrap();
        let themes: Vec<(&str, &str, Option<&str>)> =
            all.iter().map(|r| r.theme.as_ref().map(|t| (t.accent.as_str(), t.emblem.as_str(), t.backdrop.as_deref())).unwrap()).collect();
        assert_eq!(themes, [("#e3b04b", "swords", Some("azeroth")), ("#6cc6f5", "snowflake", Some("northrend"))]);
        assert_eq!(all[1].summary().theme, all[1].theme);
        for (from, to) in [
            ("accent = \"#e3b04b\"", "accent = \"red\""),
            ("accent = \"#e3b04b\"", "accent = \"#e3b04\""),
            ("accent = \"#e3b04b\"", "accent = \"#e3b04g\""),
            ("accent = \"#e3b04b\"", "accent = \"#e3b04b; x\""),
            ("deep = \"#3b2a12\"", "deep = \"url(x)\""),
            ("emblem = \"swords\"", "emblem = \"Swords/../x\""),
            ("backdrop = \"azeroth\"", "backdrop = \"../azeroth\""),
        ] {
            assert!(with((from, to)).is_err(), "{to} should be rejected");
        }
    }

    #[test]
    fn allows_compiler_names_with_a_plus() {
        assert!(with(("\"clang++\"", "\"g++\"")).is_ok());
    }

    #[test]
    fn rejects_duplicate_repository_ids() {
        assert!(with(("id = \"database\"", "id = \"server\"")).is_err());
    }

    #[test]
    fn summary_exposes_only_what_the_ui_needs() {
        let json = serde_json::to_string(&builtin().unwrap()[0].summary()).unwrap();
        assert!(json.contains("\"clientBuild\":5875"));
        assert!(!json.contains("cmake"));
    }
}
