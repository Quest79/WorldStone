//! Turning the user's game client into the data the server reads (Step 3).
//!
//! The emulator's own extractor runs in the server image with the client mounted read-only.
//! Its output goes to a Docker volume, because thousands of small files are far slower to write
//! through the Windows file share than into Docker's own storage, and the server containers
//! mount the same volume later. Inside the volume:
//!
//! ```text
//! staging/    being written; an interrupted run is resumed from here
//! current/    the finished data the server reads, with a record of how it was made
//! previous/   the data `current/` replaced, kept until the next replacement
//! ```
//!
//! Data only reaches `current/` after every output folder is verified to hold files, so a failed
//! or cancelled run never damages what the server is using.

use crate::{
    builder::{self, format_size, ImageInfo},
    client::{self, ClientInfo},
    docker,
    paths::InstancePaths,
    process::{probe, CommandSpec},
    recipe::{ExtractPhase, Recipe},
    tasks::{Task, CANCELLED},
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CLIENT_MOUNT: &str = "/client";
const DATA_MOUNT: &str = "/data";
const RECORD_FILE: &str = ".worldstone-extract.json";
/// The extractor is memory-light per thread; this errs on the low side and the run retries with
/// fewer threads if the kernel still kills it.
const MEMORY_PER_THREAD: u64 = 1 << 30;

pub fn volume_name(instance: &str) -> String {
    format!("worldstone-{instance}-gamedata")
}

fn container_name(instance: &str) -> String {
    format!("worldstone-{instance}-extract")
}

/// The container that exists only for the moment it takes to read a file out of the data volume.
fn reader_name(instance: &str) -> String {
    format!("worldstone-{instance}-reader")
}

/// How many extractor threads the Docker VM can feed: one per CPU, but no more than its memory
/// allows after leaving room for the system.
pub fn threads_for(cpus: u32, memory: u64) -> u32 {
    let by_memory = memory.saturating_sub(builder::MEMORY_HEADROOM) / MEMORY_PER_THREAD;
    (by_memory.min(u64::from(cpus)) as u32).max(1)
}

// ---------------------------------------------------------------------------------------
// The record of a finished extraction
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutputStat {
    pub name: String,
    pub files: u64,
    pub bytes: u64,
}

/// Stored beside the data, so what the app reports is what is actually in the volume.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub client_build: u32,
    pub client_version: String,
    /// The server commit whose extractor produced the data. The formats can change with it.
    pub extractor_commit: String,
    pub finished_unix: u64,
    /// Time spent in the run that finished it (an interrupted run's earlier work is not counted).
    pub seconds: u64,
    pub outputs: Vec<OutputStat>,
}

impl Record {
    /// True when every output the recipe requires is present and holds files.
    fn covers(&self, outputs: &[String]) -> Result<(), String> {
        for name in outputs {
            match self.outputs.iter().find(|o| &o.name == name) {
                Some(o) if o.files > 0 => {}
                Some(_) => return Err(format!("The record shows no files in {name}.")),
                None => return Err(format!("The record does not list {name}.")),
            }
        }
        Ok(())
    }
}

/// Reads the `STAT <folder> <files> <KiB>` lines the verification step prints. Every required
/// folder must be there and hold at least one file.
pub fn parse_stats(lines: &[String], outputs: &[String]) -> Result<Vec<OutputStat>, String> {
    outputs
        .iter()
        .map(|name| {
            let stat = lines
                .iter()
                .rev()
                .filter_map(|l| {
                    let mut parts = l.split_whitespace();
                    (parts.next()? == "STAT" && parts.next()? == name).then(|| {
                        let files: u64 = parts.next()?.parse().ok()?;
                        let kib: u64 = parts.next()?.parse().ok()?;
                        Some(OutputStat { name: name.clone(), files, bytes: kib * 1024 })
                    })?
                })
                .next()
                .ok_or_else(|| format!("Could not count the files in {name}."))?;
            if stat.files == 0 {
                return Err(format!("The extractor finished but {name} holds no files."));
            }
            Ok(stat)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ExtractState {
    /// Docker or the image could not be asked.
    Unknown,
    NotExtracted,
    Ready,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExtractStatus {
    pub state: ExtractState,
    /// Why extraction cannot start right now, if it cannot.
    pub blocked: Option<String>,
    /// Something worth knowing that does not stop extraction, such as an unreadable record.
    pub note: Option<String>,
    pub volume: String,
    pub record: Option<Record>,
    /// Whether an interrupted extraction is waiting to be resumed.
    pub resumable: bool,
}

/// Splits what the status script printed into "an interrupted run is waiting" and the record
/// (or `NONE`). The marker may share a line with a record that has no trailing newline.
fn split_volume_text(text: &str) -> (bool, String) {
    const MARKER: &str = "RESUMABLE";
    let resumable = text.lines().any(|l| l.trim_end().ends_with(MARKER));
    let body = text
        .lines()
        .map(|l| l.trim_end().strip_suffix(MARKER).unwrap_or(l))
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (resumable, body)
}

/// What extraction needs before it can start, and why it cannot if something is missing.
struct Readiness {
    reference: String,
    image: Option<ImageInfo>,
    client: Option<ClientInfo>,
    /// The first thing standing in the way, if anything is.
    blocked: Option<String>,
}

async fn readiness(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Readiness {
    let reference = builder::tools_image(instance, recipe);
    if let Err(reason) = builder::engine().await {
        return Readiness { reference, image: None, client: None, blocked: Some(reason) };
    }
    let image = builder::inspect_image(&reference).await;
    let status = client::status(paths, &recipe.client);
    let blocked = if image.is_none() {
        Some("Build the server first: the extractor comes with it.".to_string())
    } else {
        match (&status.info, status.problem) {
            (Some(_), _) => None,
            (None, Some(problem)) => Some(problem),
            (None, None) => Some("Choose your game client folder first.".to_string()),
        }
    };
    Readiness { reference, image, client: status.info, blocked }
}

pub(crate) async fn volume_exists(volume: &str) -> bool {
    docker::volume_exists(volume).await
}

/// What the data volume says, read with the volume shown read-only: whether an interrupted run is
/// waiting to be resumed, and the record of the last extraction (`None` when there is none).
async fn read_volume(instance: &str, image: &str, volume: &str) -> Result<(bool, Option<String>), String> {
    let record = format!("{DATA_MOUNT}/current/{RECORD_FILE}");
    let session = format!("{DATA_MOUNT}/staging/.session");
    if let Some(files) = docker::read_volume_files(&reader_name(instance), image, volume, DATA_MOUNT, &[&record, &session]).await? {
        let mut files = files.into_iter();
        let record = files.next().flatten().map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        let resumable = files.next().flatten().is_some();
        return Ok((resumable, record));
    }
    // Docker is not reachable on a local pipe, so ask through the command line instead.
    let script = format!(
        "f={DATA_MOUNT}/current/{RECORD_FILE}; if [ -f \"$f\" ]; then cat \"$f\"; echo; else echo NONE; fi; \
         if [ -f {DATA_MOUNT}/staging/.session ]; then echo RESUMABLE; fi"
    );
    let mount = format!("{volume}:{DATA_MOUNT}:ro");
    let out = probe("docker", &["run", "--rm", "-v", &mount, "--entrypoint", "sh", image, "-c", &script]).await;
    if !out.success {
        return Err(out.text);
    }
    let (resumable, body) = split_volume_text(&out.text);
    Ok((resumable, (body.trim() != "NONE").then_some(body)))
}

pub async fn status(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> ExtractStatus {
    let volume = volume_name(instance);
    let base = ExtractStatus {
        state: ExtractState::Unknown,
        blocked: None,
        note: None,
        volume: volume.clone(),
        record: None,
        resumable: false,
    };
    let ready = readiness(paths, instance, recipe).await;
    let blocked = ready.blocked;
    // The image is needed to look inside the volume as well as to extract, so without it (or
    // Docker) the data cannot be judged either way.
    if ready.image.is_none() {
        return ExtractStatus { blocked, ..base };
    }
    if !volume_exists(&volume).await {
        return ExtractStatus { state: ExtractState::NotExtracted, blocked, ..base };
    }
    let (resumable, body) = match read_volume(instance, &ready.reference, &volume).await {
        Ok(read) => read,
        Err(e) => return ExtractStatus { blocked, note: Some(format!("Could not read the data volume: {e}")), ..base },
    };
    let Some(body) = body else {
        return ExtractStatus { state: ExtractState::NotExtracted, blocked, resumable, ..base };
    };
    match serde_json::from_str::<Record>(&body) {
        Ok(record) => match record.covers(&recipe.extract.outputs) {
            Ok(()) => ExtractStatus { state: ExtractState::Ready, blocked, resumable, record: Some(record), ..base },
            Err(why) => ExtractStatus { state: ExtractState::NotExtracted, blocked, resumable, note: Some(why), ..base },
        },
        Err(_) => ExtractStatus {
            state: ExtractState::NotExtracted,
            blocked,
            resumable,
            note: Some("The record of the last extraction is unreadable, so the data cannot be trusted. Extract again.".into()),
            ..base
        },
    }
}

// ---------------------------------------------------------------------------------------
// Running the extractor
// ---------------------------------------------------------------------------------------

/// Fills in the recipe's placeholders.
fn fill(arg: &str, src: &str, dest: &str, threads: u32, prefix: &str) -> String {
    arg.replace("{client}", CLIENT_MOUNT)
        .replace("{src}", src)
        .replace("{dest}", dest)
        .replace("{threads}", &threads.to_string())
        .replace("{prefix}", prefix)
}

/// `docker run` for one extraction phase. The tool runs under `sh` so that a marker file is
/// written only when it exits 0; `--init` makes an interrupt reach it instead of being ignored
/// by a container's first process.
fn phase_command(recipe: &Recipe, instance: &str, image: &str, client: &ClientInfo, phase: &ExtractPhase, threads: u32) -> CommandSpec {
    let prefix = &recipe.build.install_prefix;
    let src = format!("{CLIENT_MOUNT}/{}", client.data_dir);
    let dest = format!("{DATA_MOUNT}/staging");
    let tool = format!("{prefix}/{}", phase.tool.as_ref().unwrap_or(&recipe.extract.tool));
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "--init".into(),
        "--name".into(),
        container_name(instance),
        "-v".into(),
        format!("{}:{CLIENT_MOUNT}:ro", client.path),
        "-v".into(),
        format!("{}:{DATA_MOUNT}", volume_name(instance)),
        "-e".into(),
        format!("DONE={DATA_MOUNT}/staging/.done/{}", phase.id),
    ];
    if phase.in_output {
        args.extend(["-w".into(), dest.clone()]);
    }
    // Leftovers of an interrupted attempt are cleared first; the names are plain (validated on load).
    let clean: String = phase.clean.iter().map(|folder| format!("rm -rf \"{dest}/{folder}\"; ")).collect();
    args.extend([
        "--entrypoint".into(),
        "sh".into(),
        image.into(),
        "-c".into(),
        format!("{clean}\"$0\" \"$@\" && touch \"$DONE\""),
        tool,
    ]);
    args.extend(phase.args.iter().chain(&recipe.extract.args).map(|a| fill(a, &src, &dest, threads, prefix)));
    // The tool does not react to an interrupt relayed through the Docker client, so waiting the
    // default grace period only delays a cancel. The container is removed right after either way.
    CommandSpec { program: "docker".into(), args, grace: Some(Duration::from_secs(2)), ..CommandSpec::default() }
}

/// Turns the extractor's per-unit output lines into a live status such as
/// `Step 4 of 5: World tiles · 12 maps done`. When the tool itself reports a total, as
/// `[7/51]`, that is shown instead (`7 of 51 maps done`); it is never guessed.
pub struct Counter {
    header: String,
    prefix: Option<String>,
    unit: String,
    percent: bool,
    done: u32,
}

/// The `[done/total]` or `(done/total)` a tool prints on a line, if it does.
fn reported_progress(line: &str) -> Option<(u32, u32)> {
    line.match_indices(['[', '(']).find_map(|(at, open)| {
        let close = if open == "[" { ']' } else { ')' };
        let inner = &line[at + 1..];
        let (done, rest) = inner.split_once('/')?;
        let (total, _) = rest.split_once(close)?;
        Some((done.trim().parse().ok()?, total.trim().parse().ok()?))
    })
}

/// The `37%` a tool prints at the start of a line, if it does.
fn reported_percent(line: &str) -> Option<u32> {
    let (number, _) = line.trim_start().split_once('%')?;
    number.parse().ok().filter(|p| *p <= 100)
}

impl Counter {
    pub fn new(phase: &ExtractPhase, number: usize, total: usize) -> Self {
        Self {
            header: format!("Step {number} of {total}: {}", phase.label),
            prefix: phase.count_prefix.clone(),
            unit: phase.count_unit.clone().unwrap_or_else(|| "items".to_string()),
            percent: phase.count_percent,
            done: 0,
        }
    }

    pub fn header(&self) -> &str {
        &self.header
    }

    /// The new status text if this line finished another unit.
    pub fn line(&mut self, line: &str) -> Option<String> {
        if self.percent {
            let percent = reported_percent(line)?;
            return Some(format!("{} · {percent}% done", self.header));
        }
        let prefix = self.prefix.as_deref()?;
        if !line.trim_start().starts_with(prefix) {
            return None;
        }
        match reported_progress(line) {
            Some((done, total)) if done <= total => {
                self.done = done;
                Some(format!("{} · {done} of {total} {} done", self.header, self.unit))
            }
            _ => {
                self.done += 1;
                Some(format!("{} · {} {} done", self.header, self.done, self.unit))
            }
        }
    }
}

/// The last few lines of output that say what went wrong.
pub(crate) fn failure_detail(tail: &[String]) -> Option<String> {
    let lines: Vec<&str> = tail.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let start = lines.len().saturating_sub(3);
    (!lines.is_empty()).then(|| lines[start..].join(" | ").chars().take(400).collect())
}

fn explain_failure(error: &str, phase: &ExtractPhase, threads: u32, tail: &[String]) -> String {
    if error.contains("exit code 137") {
        return format!(
            "{} ran out of memory even with {threads} thread{}. Give Docker Desktop more memory \
             (Settings > Resources, or memory= in %UserProfile%\\.wslconfig) and extract again; finished steps are kept.",
            phase.label,
            if threads == 1 { "" } else { "s" }
        );
    }
    match failure_detail(tail) {
        Some(detail) => format!("{}: {error} Last output: {detail}", phase.label),
        None => format!("{}: {error}", phase.label),
    }
}

/// The names of the phases marked done in a resumable staging area, from the prepare step's output.
fn parse_prepare(text: &str) -> (bool, Vec<String>) {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let resume = lines.next() == Some("RESUME");
    (resume, if resume { lines.map(str::to_string).collect() } else { Vec::new() })
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Removes a leftover extraction container (an app that was closed mid-run leaves one behind).
async fn remove_container(instance: &str) {
    let _ = probe("docker", &["rm", "-f", &container_name(instance)]).await;
}

/// Extracts the client's data into the instance's volume, resuming an interrupted run of the
/// same client and build, and replaces the data the server uses only when it is complete.
pub async fn extract(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    let started = Instant::now();
    task.step("Checking the client and the server image");
    let ready = readiness(paths, instance, recipe).await;
    if let Some(reason) = ready.blocked {
        return Err(reason);
    }
    let (image, info, client) = match (ready.image, ready.client) {
        (Some(info), Some(client)) => (ready.reference, info, client),
        _ => return Err("The client or the server image could not be checked.".to_string()),
    };
    let docker = builder::engine().await?;
    task.info(&format!("Client: {} (build {}) at {}", client.version, client.build, client.path));
    task.info(&format!("Extractor from the server image built from commit {}.", info.commit));

    let volume = volume_name(instance);
    let prefix = &recipe.build.install_prefix;
    let missing_input = {
        let mut args = vec!["run", "--rm", "--entrypoint", "ls", image.as_str(), "-1"];
        let files: Vec<String> = std::iter::once(&recipe.extract.tool)
            .chain(recipe.extract.phases.iter().filter_map(|p| p.tool.as_ref()))
            .chain(&recipe.extract.inputs)
            .map(|f| format!("{prefix}/{f}"))
            .collect();
        args.extend(files.iter().map(String::as_str));
        let out = probe("docker", &args).await;
        (!out.success).then_some(out.text)
    };
    if let Some(text) = missing_input {
        return Err(format!("The server image lacks files the extractor needs. Rebuild the server. ({text})"));
    }

    task.step("Preparing storage");
    remove_container(instance).await;
    let created = probe("docker", &["volume", "create", "--label", &format!("io.worldstone.instance={instance}"), &volume]).await;
    if !created.success {
        return Err(format!("Cannot create the data volume {volume}: {}", created.text));
    }
    let session = format!("{}|{}|{}", info.fingerprint, client.build, client.path);
    let prepare = format!(
        "set -eu; s={DATA_MOUNT}/staging; \
         if [ -f \"$s/.session\" ] && [ \"$(cat \"$s/.session\")\" = \"$SESSION\" ]; then echo RESUME; ls -1 \"$s/.done\" 2>/dev/null || true; \
         else rm -rf \"$s\"; mkdir -p \"$s/.done\"; printf '%s' \"$SESSION\" > \"$s/.session\"; echo FRESH; fi"
    );
    // Not a probe: clearing an old staging area can take longer than a probe waits.
    task.run(CommandSpec::new(
        "docker",
        &[
            "run", "--rm", "-v", &format!("{volume}:{DATA_MOUNT}"), "-e", &format!("SESSION={session}"),
            "--entrypoint", "sh", &image, "-c", &prepare,
        ],
    ))
    .await
    .map_err(|e| format!("Cannot prepare the data volume: {e}"))?;
    let (resuming, finished) = parse_prepare(&task.recent_output().join("\n"));
    if resuming && !finished.is_empty() {
        task.info(&format!("Resuming an interrupted extraction; finished: {}.", finished.join(", ")));
    }

    let mut threads = threads_for(docker.cpus, docker.memory);
    task.info(&format!(
        "Docker has {} CPUs and {}; using {threads} thread{}.",
        docker.cpus,
        format_size(docker.memory),
        if threads == 1 { "" } else { "s" }
    ));

    let phases = &recipe.extract.phases;
    for (index, phase) in phases.iter().enumerate() {
        let mut counter = Counter::new(phase, index + 1, phases.len());
        if finished.contains(&phase.id) {
            task.info(&format!("{}: finished earlier, not repeated.", phase.label));
            continue;
        }
        loop {
            task.step(counter.header());
            let spec = phase_command(recipe, instance, &image, &client, phase, threads);
            match task.run_tracked(spec, &mut |line| counter.line(line)).await {
                Ok(()) => break,
                Err(e) => {
                    remove_container(instance).await;
                    if e == CANCELLED {
                        return Err(e);
                    }
                    let tail = task.recent_output();
                    if e.contains("exit code 137") && threads > 1 {
                        threads = (threads / 2).max(1);
                        task.info(&format!(
                            "{} ran out of memory. Trying again with {threads} thread{}.",
                            phase.label,
                            if threads == 1 { "" } else { "s" }
                        ));
                        counter = Counter::new(phase, index + 1, phases.len());
                        continue;
                    }
                    return Err(explain_failure(&e, phase, threads, &tail));
                }
            }
        }
    }

    task.step("Verifying the extracted data");
    let count = format!(
        "for d in \"$@\"; do n=$(find \"{DATA_MOUNT}/staging/$d\" -type f 2>/dev/null | wc -l); \
         k=$(du -sk \"{DATA_MOUNT}/staging/$d\" 2>/dev/null | cut -f1); echo \"STAT $d ${{n:-0}} ${{k:-0}}\"; done"
    );
    let mut args = vec!["run".to_string(), "--rm".into(), "-v".into(), format!("{volume}:{DATA_MOUNT}:ro")];
    args.extend(["--entrypoint".into(), "sh".into(), image.clone(), "-c".into(), count, "sh".into()]);
    args.extend(recipe.extract.outputs.iter().cloned());
    task.run(CommandSpec { program: "docker".into(), args, ..CommandSpec::default() })
        .await
        .map_err(|e| format!("Could not verify the extracted data: {e}"))?;
    let outputs = parse_stats(&task.recent_output(), &recipe.extract.outputs)?;
    for o in &outputs {
        task.info(&format!("{}: {} files, {}", o.name, o.files, format_size(o.bytes)));
    }

    task.step("Putting the data in place");
    let record = Record {
        client_build: client.build,
        client_version: client.version.clone(),
        extractor_commit: info.commit.clone(),
        finished_unix: unix_now(),
        seconds: started.elapsed().as_secs(),
        outputs,
    };
    let json = serde_json::to_string(&record).map_err(|e| e.to_string())?;
    // Working files between phases are dropped here, once the outputs are known to be complete.
    let scratch: String = recipe.extract.scratch.iter().map(|folder| format!(" \"$s/{folder}\"")).collect();
    let promote = format!(
        "set -eu; s={DATA_MOUNT}/staging; rm -rf \"$s/.done\" \"$s/.session\"{scratch}; printf '%s' \"$RECORD\" > \"$s/{RECORD_FILE}\"; \
         if [ -d {DATA_MOUNT}/current ]; then rm -rf {DATA_MOUNT}/previous; mv {DATA_MOUNT}/current {DATA_MOUNT}/previous; fi; \
         mv \"$s\" {DATA_MOUNT}/current"
    );
    task.run(CommandSpec::new(
        "docker",
        &[
            "run", "--rm", "-v", &format!("{volume}:{DATA_MOUNT}"), "-e", &format!("RECORD={json}"),
            "--entrypoint", "sh", &image, "-c", &promote,
        ],
    ))
    .await
    .map_err(|e| format!("Could not put the extracted data in place: {e}"))?;
    task.info(&format!(
        "Client data is ready in volume {volume} (the previous data, if any, is kept as 'previous')."
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero() -> Recipe {
        crate::recipe::builtin().unwrap().remove(0)
    }

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| s.to_string()).collect()
    }

    fn outputs() -> Vec<String> {
        zero().extract.outputs
    }

    fn client() -> ClientInfo {
        ClientInfo {
            path: "C:\\Games\\Wow Vanilla 1.12.1".into(),
            data_dir: "Data".into(),
            build: 5875,
            version: "1.12.1".into(),
            locales: vec![],
            data_bytes: 0,
        }
    }

    #[test]
    fn threads_follow_cpus_and_memory() {
        const GIB: u64 = 1 << 30;
        assert_eq!(threads_for(12, 16 * GIB), 12);
        assert_eq!(threads_for(16, 8 * GIB), 6, "8 GiB leaves 6 after headroom");
        assert_eq!(threads_for(4, 64 * GIB), 4);
        assert_eq!(threads_for(8, GIB), 1, "never zero");
    }

    #[test]
    fn counts_come_from_the_tools_own_output_and_nothing_else() {
        let recipe = zero();
        let tile = recipe.extract.phases.iter().find(|p| p.id == "tile").unwrap();
        let mut counter = Counter::new(tile, 4, 5);
        assert_eq!(counter.header(), "Step 4 of 5: World tiles");
        assert_eq!(counter.line("opened 8 archives from /client/Data ()"), None);
        assert_eq!(counter.line("  map    0 Eastern Kingdoms        1024 tiles").as_deref(), Some("Step 4 of 5: World tiles · 1 maps done"));
        assert_eq!(counter.line("  map    1 Kalimdor                 900 tiles").as_deref(), Some("Step 4 of 5: World tiles · 2 maps done"));

        let dbc = recipe.extract.phases.iter().find(|p| p.id == "dbc").unwrap();
        assert_eq!(Counter::new(dbc, 1, 5).line("  map 1 x"), None, "a phase with nothing to count never invents a number");
    }

    #[test]
    fn a_total_the_tool_reports_is_shown_and_one_it_does_not_is_never_made_up() {
        let recipe = zero();
        let nav = recipe.extract.phases.iter().find(|p| p.id == "nav").unwrap();
        let mut counter = Counter::new(nav, 5, 5);
        assert_eq!(
            counter.line("nav map 47  [7/51] -> 6/6 mmtiles").as_deref(),
            Some("Step 5 of 5: Movement maps · 7 of 51 maps done")
        );
        assert_eq!(counter.line("nav map 48  [xx/51] -> 4/6 mmtiles").as_deref(), Some("Step 5 of 5: Movement maps · 8 maps done"));
        assert_eq!(counter.line("nav map 49  [99/51] -> 4/6 mmtiles").as_deref(), Some("Step 5 of 5: Movement maps · 9 maps done"));
        assert_eq!(reported_progress("no brackets here"), None);
    }

    #[test]
    fn the_phase_command_mounts_the_client_read_only_and_keeps_paths_with_spaces_whole() {
        let recipe = zero();
        let phase = recipe.extract.phases.iter().find(|p| p.id == "nav").unwrap();
        let spec = phase_command(&recipe, "mangos-zero", "worldstone/vanilla-world:latest", &client(), phase, 6);
        assert_eq!(spec.program, "docker");
        let a = &spec.args;
        assert!(a.contains(&"C:\\Games\\Wow Vanilla 1.12.1:/client:ro".to_string()), "{a:?}");
        assert!(a.contains(&"worldstone-mangos-zero-gamedata:/data".to_string()));
        assert!(a.contains(&"--init".to_string()) && a.contains(&"--rm".to_string()));
        assert!(a.contains(&"DONE=/data/staging/.done/nav".to_string()));
        let joined = a.join(" ");
        assert!(joined.contains("/mangos/bin/tools/mangos-extractor nav --src /client/Data --dest /data/staging --threads 6 --no-menu"), "{joined}");
        assert!(joined.contains("--vessels /mangos/bin/tools/vessels.txt --offmesh /mangos/bin/tools/offmesh.txt"));
        assert!(!joined.contains('{'), "every placeholder is filled: {joined}");
    }

    fn azerothcore() -> Recipe {
        crate::recipe::builtin().unwrap().into_iter().find(|r| r.id == "azerothcore").unwrap()
    }

    fn wrath_client() -> ClientInfo {
        ClientInfo { path: "C:\\Games\\Wow Wrath 3.3.5a".into(), build: 12340, version: "3.3.5a".into(), ..client() }
    }

    fn command_of(recipe: &Recipe, phase: &str) -> String {
        let phase = recipe.extract.phases.iter().find(|p| p.id == phase).unwrap();
        phase_command(recipe, &recipe.id, "worldstone/wotlk-world:latest", &wrath_client(), phase, 6).args.join(" ")
    }

    #[test]
    fn each_azerothcore_tool_runs_where_it_needs_to_and_starts_from_a_clean_slate() {
        let recipe = azerothcore();
        let maps = command_of(&recipe, "maps");
        assert!(maps.contains("/azerothcore/bin/map_extractor -i /client -o /data/staging"), "{maps}");
        assert!(!maps.contains(" -w "), "the map extractor is told where to write");
        assert!(maps.contains("C:\\Games\\Wow Wrath 3.3.5a:/client:ro"), "the client is only ever read");

        let models = command_of(&recipe, "models");
        assert!(models.contains("-w /data/staging"), "it writes to the folder it runs in: {models}");
        assert!(models.contains("rm -rf \"/data/staging/Buildings\"; "), "it refuses to run over an interrupted attempt");
        assert!(models.contains("/azerothcore/bin/vmap4_extractor -d /client/Data"));

        let vmaps = command_of(&recipe, "vmaps");
        assert!(vmaps.contains("rm -rf \"/data/staging/vmaps\"; ") && vmaps.contains("/azerothcore/bin/vmap4_assembler Buildings vmaps"), "{vmaps}");

        let mmaps = command_of(&recipe, "mmaps");
        assert!(mmaps.contains("-w /data/staging") && mmaps.contains("/azerothcore/bin/mmaps_generator --threads 6 --silent"), "{mmaps}");
        assert!(!mmaps.contains('{'), "every placeholder is filled: {mmaps}");
    }

    #[test]
    fn azerothcores_own_progress_lines_give_real_counts() {
        let recipe = azerothcore();
        let phase = |id: &str| recipe.extract.phases.iter().find(|p| p.id == id).unwrap().clone();
        let mut maps = Counter::new(&phase("maps"), 1, 4);
        assert_eq!(maps.line("Extract Azeroth (3/68)").as_deref(), Some("Step 1 of 4: Client databases and maps · 3 of 68 maps done"));
        assert_eq!(maps.line("Loaded 25 tile files"), None);

        let mut models = Counter::new(&phase("models"), 2, 4);
        assert_eq!(models.line("Processing Map 12 (5/61)").as_deref(), Some("Step 2 of 4: Building models · 5 of 61 maps done"));

        let mut mmaps = Counter::new(&phase("mmaps"), 4, 4);
        assert_eq!(mmaps.line("37% [Map 0] Building tile").as_deref(), Some("Step 4 of 4: Navigation meshes · 37% done"));
        assert_eq!(mmaps.line("building tile without a percentage"), None, "no percentage, no progress made up");
        assert_eq!(mmaps.line("250% nonsense"), None);
    }

    #[test]
    fn verification_needs_every_output_to_hold_files() {
        let out = outputs();
        let good = lines(&["STAT dbc 130 4200", "STAT tiles 7300 900000", "STAT gomodels 900 50000", "STAT mmaps 5000 1200000"]);
        let stats = parse_stats(&good, &out).unwrap();
        assert_eq!(stats[1], OutputStat { name: "tiles".into(), files: 7300, bytes: 900_000 * 1024 });

        let empty_nav = lines(&["STAT dbc 130 4200", "STAT tiles 7300 9", "STAT gomodels 900 5", "STAT mmaps 0 0"]);
        assert!(parse_stats(&empty_nav, &out).unwrap_err().contains("mmaps holds no files"));
        let missing = lines(&["STAT dbc 130 4200"]);
        assert!(parse_stats(&missing, &out).unwrap_err().contains("tiles"));
        let noise = lines(&["find: cannot access", "STAT dbc x y"]);
        assert!(parse_stats(&noise, &out).is_err(), "unreadable counts are not zero, they are unknown");
    }

    #[test]
    fn the_resume_marker_never_corrupts_the_record_beside_it() {
        let record = r#"{"clientBuild":5875}"#;
        assert_eq!(split_volume_text(&format!("{record}\nRESUMABLE\n")), (true, record.to_string()));
        assert_eq!(split_volume_text(&format!("{record}RESUMABLE\n")), (true, record.to_string()));
        assert_eq!(split_volume_text(&format!("{record}\n")), (false, record.to_string()));
        assert_eq!(split_volume_text("NONE\nRESUMABLE\n"), (true, "NONE".to_string()));
    }

    #[test]
    fn a_record_covers_the_recipe_only_when_every_output_has_files() {
        let stat = |name: &str, files| OutputStat { name: name.into(), files, bytes: 1 };
        let record = Record {
            client_build: 5875,
            client_version: "1.12.1".into(),
            extractor_commit: "abc".into(),
            finished_unix: 1,
            seconds: 2,
            outputs: vec![stat("dbc", 1), stat("tiles", 1), stat("gomodels", 1), stat("mmaps", 1)],
        };
        assert!(record.covers(&outputs()).is_ok());
        let mut less = record.clone();
        less.outputs.pop();
        assert!(less.covers(&outputs()).unwrap_err().contains("mmaps"));
        let mut hollow = record.clone();
        hollow.outputs[0].files = 0;
        assert!(hollow.covers(&outputs()).is_err());
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), record);
        assert!(json.contains("\"clientBuild\":5875"));
    }

    #[test]
    fn the_prepare_step_says_whether_to_resume_and_what_is_finished() {
        assert_eq!(parse_prepare("FRESH"), (false, vec![]));
        assert_eq!(parse_prepare("RESUME\ndbc\ngomodels\n"), (true, vec!["dbc".to_string(), "gomodels".to_string()]));
        assert_eq!(parse_prepare("RESUME"), (true, vec![]));
        assert_eq!(parse_prepare(""), (false, vec![]));
    }

    #[test]
    fn failures_name_the_phase_and_the_last_output() {
        let recipe = zero();
        let nav = recipe.extract.phases.iter().find(|p| p.id == "nav").unwrap();
        let tail = lines(&["", "nav: bake failed; inspect the earlier diagnostics and /data/staging/tiles"]);
        let message = explain_failure("docker failed with exit code 1.", nav, 4, &tail);
        assert!(message.starts_with("Movement maps:") && message.contains("bake failed"), "{message}");

        let oom = explain_failure("docker failed with exit code 137. See the log for details.", nav, 1, &[]);
        assert!(oom.contains("Docker Desktop more memory") && oom.contains("1 thread."), "{oom}");
        assert_eq!(explain_failure("x", nav, 2, &[]), "Movement maps: x");
    }
}
