//! Building the server inside a container.
//!
//! The recipe says what to compile and with which flags; this module turns that into a
//! Dockerfile, builds it, and proves the result: the installed files exist, the programs run,
//! and the playerbots marker file (installed only when bots are compiled in) is present.
//!
//! The Docker image is the record of the build. Its tag is a fingerprint of the Dockerfile
//! and the exact source, and its labels say which commit it was built from, so "is this build
//! current?" is answered from Docker itself and cannot drift from a separate lockfile.

use crate::{
    docker::{self, EngineInfo},
    git::{self, RepoState, SourceState},
    paths::InstancePaths,
    process::{probe, CommandSpec},
    recipe::{Recipe, ServiceSpec, KEPT_SOURCE_FOLDER},
    tasks::{Task, CANCELLED},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const LABEL_FINGERPRINT: &str = "io.worldstone.fingerprint";
const LABEL_COMMIT: &str = "io.worldstone.commit";
const LABEL_PLAYERBOTS: &str = "io.worldstone.playerbots";
const LABEL_RECIPE: &str = "io.worldstone.recipe";

/// Bumped when the generated Dockerfile's structure changes in a way the text alone would not show.
const TEMPLATE_VERSION: &str = "1";
/// Memory kept back for Docker's own processes and the rest of its VM.
pub(crate) const MEMORY_HEADROOM: u64 = 2 << 30;
/// Memory one compile job is assumed to need: the heavy game translation units take about this.
const MEMORY_PER_JOB: u64 = 3 << 29;

/// Where the image of one of an instance's servers lives: `worldstone/<edition>-<role>`, for
/// example `worldstone/vanilla-realm` and `worldstone/vanilla-world`. A second instance of the same
/// edition adds its own name (`worldstone/vanilla-second-world`) so the two never share an image.
pub fn image_repo(instance: &str, recipe: &Recipe, service: &ServiceSpec) -> String {
    if instance == recipe.id {
        format!("worldstone/{}-{}", recipe.edition, service.role)
    } else {
        format!("worldstone/{}-{instance}-{}", recipe.edition, service.role)
    }
}

/// The name the database image goes by on this machine: `worldstone/<edition>-database:<tag>`, for
/// example `worldstone/vanilla-database:11.4`. It is the recipe's public image under a name that says
/// which server it belongs to; the tag stays the public image's own, so its version still shows.
pub fn database_image(instance: &str, recipe: &Recipe) -> String {
    let tag = recipe.database.image.rsplit_once(':').map_or("latest", |(_, tag)| tag);
    if instance == recipe.id {
        format!("worldstone/{}-database:{tag}", recipe.edition)
    } else {
        format!("worldstone/{}-{instance}-database:{tag}", recipe.edition)
    }
}

/// The image a server's container starts from: the one the build tagged `latest`.
pub fn image_ref(instance: &str, recipe: &Recipe, service: &ServiceSpec) -> String {
    format!("{}:latest", image_repo(instance, recipe, service))
}

/// The image the tools, the sample configuration files and the extractors are read from.
pub fn tools_image(instance: &str, recipe: &Recipe) -> String {
    image_ref(instance, recipe, recipe.tools_service())
}

/// Where builds were kept before images were named by edition and role: `worldstone/<instance>`.
/// Nothing builds there any more; what is left is listed as an old build.
pub fn legacy_repo(instance: &str) -> String {
    format!("worldstone/{instance}")
}

/// The dangling images a rebuild leaves behind carry this label, so they can be found again.
pub const LABEL_INSTANCE: &str = "io.worldstone.instance";

// ---------------------------------------------------------------------------------------
// Dockerfile
// ---------------------------------------------------------------------------------------

/// The Dockerfile for a recipe. Pure: the same recipe always gives the same text, which is
/// what makes the fingerprint meaningful. The parallel job count is a build argument, not
/// part of the text, because it depends on the machine and must not force a rebuild.
pub fn dockerfile(recipe: &Recipe, instance: &str) -> String {
    let b = &recipe.build;
    let prefix = &b.install_prefix;
    let options: String = b.cmake.iter().map(|(k, v)| format!(" \\\n      -D{k}={v}")).collect();

    let mut checks = String::from("set -eu;");
    for file in &b.required_files {
        checks += &format!(
            " \\\n    test -f {prefix}/{file} || {{ echo \"The build did not produce {prefix}/{file}\" >&2; exit 1; }};"
        );
    }
    let marker = &recipe.playerbots.marker;
    checks += &format!(
        " \\\n    test -f {prefix}/{marker} || {{ echo \"Playerbots are not compiled in: {prefix}/{marker} is missing\" >&2; exit 1; }};"
    );
    for smoke in &b.smoke {
        let args: String = smoke.args.iter().map(|a| format!(" {a}")).collect();
        checks += &format!(" \\\n    {prefix}/{}{args};", smoke.run);
    }
    checks.pop(); // the final `;`

    // Source folders the running servers read (the database importer looks for its scripts there).
    let keep: String = b
        .keep_source
        .iter()
        .map(|folder| {
            format!(" \\\n && mkdir -p {prefix}/{KEPT_SOURCE_FOLDER}/{folder} \\\n && cp -a /src/{folder}/. {prefix}/{KEPT_SOURCE_FOLDER}/{folder}/")
        })
        .collect();

    format!(
        r#"# syntax=docker/dockerfile:1
# Generated by WorldStone from the "{recipe_id}" recipe and rewritten on every build.
# Change the recipe, not this file.

FROM {base} AS build
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends ccache {build_packages} \
 && rm -rf /var/lib/apt/lists/*
COPY . /src
ARG JOBS=2
RUN --mount=type=cache,target=/ccache,id=worldstone-{instance}-ccache \
    export CCACHE_DIR=/ccache \
 && cmake -S /src -B /build -G "Unix Makefiles" \
      -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_INSTALL_PREFIX={prefix} \
      -DCMAKE_C_COMPILER_LAUNCHER=ccache \
      -DCMAKE_CXX_COMPILER_LAUNCHER=ccache{options} \
 && cmake --build /build --parallel "$JOBS" \
 && cmake --install /build{keep} \
 && ccache --show-stats

FROM {base} AS runtime
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends {runtime_packages} \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build {prefix} {prefix}
RUN {checks}
{services}"#,
        recipe_id = recipe.id,
        keep = keep,
        base = b.base_image,
        build_packages = b.build_packages.join(" "),
        runtime_packages = b.runtime_packages.join(" "),
        services = recipe.run.services.iter().map(|s| service_stage(recipe, s)).collect::<String>(),
    )
}

/// The last stage of one server's image. All of them hold the same install, so they share every
/// layer and cost no extra disk; what makes each its own image, with its own id and name, is the
/// role it is labelled with and the program it starts by default.
fn service_stage(recipe: &Recipe, service: &ServiceSpec) -> String {
    format!(
        "\nFROM runtime AS service-{id}\nLABEL io.worldstone.edition=\"{edition}\" io.worldstone.role=\"{role}\"\nCMD [\"{prefix}/{binary}\"]\n",
        id = service.id,
        edition = recipe.edition,
        role = service.role,
        prefix = recipe.build.install_prefix,
        binary = service.binary,
    )
}

/// Keeps `.git` out of the build context, as upstream's own `.dockerignore` does. Submodule
/// checkouts have a `.git` file pointing outside the tree, useless inside the container.
pub const DOCKERIGNORE: &str = "**/.git\n";

/// Identifies one exact build: the generated Dockerfile plus the source it compiles.
pub fn fingerprint(dockerfile: &str, source_digest: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(TEMPLATE_VERSION.as_bytes());
    hash.update(b"\n");
    hash.update(dockerfile.as_bytes());
    hash.update(b"\n");
    hash.update(source_digest.as_bytes());
    format!("{:x}", hash.finalize())
}

// ---------------------------------------------------------------------------------------
// Docker engine
// ---------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub struct Engine {
    pub cpus: u32,
    pub memory: u64,
}

/// What a build can rely on from Docker's answer: Linux containers, and the CPUs and memory of the
/// machine they run on.
pub fn engine_of(info: &EngineInfo) -> Result<Engine, String> {
    if info.os_type != "linux" {
        return Err(format!("Docker is using {} containers. Switch Docker Desktop to Linux containers.", info.os_type));
    }
    match u32::try_from(info.cpus) {
        Ok(cpus) => Ok(Engine { cpus, memory: info.memory }),
        Err(_) => Err("Docker gave an unexpected answer about its resources.".to_string()),
    }
}

pub(crate) async fn engine() -> Result<Engine, String> {
    engine_of(&docker::engine_info().await?)
}

/// How many compile jobs the Docker VM can run at once: one per CPU, but no more than its
/// memory can feed. Running out of memory kills the compiler, so this errs on the low side.
pub fn jobs_for(cpus: u32, memory: u64) -> u32 {
    let by_memory = memory.saturating_sub(MEMORY_HEADROOM) / MEMORY_PER_JOB;
    (by_memory.min(u64::from(cpus)) as u32).max(1)
}

/// True when the output shows a process was killed for lack of memory.
pub fn looks_like_out_of_memory(tail: &[String]) -> bool {
    const SIGNS: [&str; 6] = ["out of memory", "cannot allocate memory", "bad_alloc", "exit code: 137", "signal 9", "killed"];
    tail.iter().any(|line| {
        let line = line.to_ascii_lowercase();
        SIGNS.iter().any(|s| line.contains(s))
    })
}

/// The job count to retry with after an out-of-memory failure, if there is a lower one to try.
pub fn retry_jobs(jobs: u32, tail: &[String]) -> Option<u32> {
    (jobs > 1 && looks_like_out_of_memory(tail)).then(|| (jobs / 2).max(1))
}

/// The most recent compiler, CMake or apt error in the output, for the failure message.
pub fn last_error_line(tail: &[String]) -> Option<String> {
    tail.iter()
        .rev()
        .find(|l| l.contains("error:") || l.contains("CMake Error") || l.contains(" E: "))
        .map(|l| l.trim().chars().take(300).collect())
}

fn explain_failure(error: String, jobs: u32, tail: &[String]) -> String {
    if looks_like_out_of_memory(tail) {
        return format!(
            "The compiler ran out of memory even with {jobs} parallel job{}. Give Docker Desktop more memory \
             (Settings > Resources, or memory= in %UserProfile%\\.wslconfig) and build again; finished work is cached.",
            if jobs == 1 { "" } else { "s" }
        );
    }
    match last_error_line(tail) {
        Some(line) => format!("{error} Last error: {line}"),
        None => error,
    }
}

// ---------------------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub created: String,
    pub size: u64,
    pub commit: String,
    pub playerbots: bool,
    pub fingerprint: String,
}

fn image_info(image: docker::Image) -> ImageInfo {
    ImageInfo {
        playerbots: image.label(LABEL_PLAYERBOTS) == "true",
        commit: image.label(LABEL_COMMIT),
        fingerprint: image.label(LABEL_FINGERPRINT),
        created: image.created,
        size: image.size,
    }
}

pub(crate) async fn inspect_image(reference: &str) -> Option<ImageInfo> {
    docker::image(reference).await.map(image_info)
}

/// Whether every server image exists and was built from exactly this fingerprint.
async fn all_current(references: &[String], fingerprint: &str) -> bool {
    for reference in references {
        match inspect_image(reference).await {
            Some(info) if info.fingerprint == fingerprint => {}
            _ => return false,
        }
    }
    true
}

// ---------------------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum BuildState {
    /// Docker could not be asked.
    Unknown,
    NotBuilt,
    /// An image exists for exactly the current source and recipe.
    Ready,
    /// An image exists but the source or recipe has changed since.
    Outdated,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BuildStatus {
    pub state: BuildState,
    /// Why a build cannot start right now, if it cannot.
    pub blocked: Option<String>,
    pub image: String,
    pub built_at: Option<String>,
    pub size_bytes: Option<u64>,
    /// The source commit the existing image was built from.
    pub commit: Option<String>,
    /// Whether that image was verified to contain playerbots; `None` when there is no image.
    pub playerbots: Option<bool>,
}

impl BuildStatus {
    fn without_image(state: BuildState, blocked: Option<String>, image: String) -> Self {
        Self { state, blocked, image, built_at: None, size_bytes: None, commit: None, playerbots: None }
    }

    fn with_image(state: BuildState, blocked: Option<String>, image: String, info: ImageInfo) -> Self {
        Self {
            state,
            blocked,
            image,
            built_at: Some(info.created),
            size_bytes: Some(info.size),
            commit: Some(info.commit),
            playerbots: Some(info.playerbots),
        }
    }
}

/// The first of the build's repositories (the source and whatever is cloned into it) that is not
/// a complete checkout, if any.
async fn unready_source(paths: &InstancePaths, recipe: &Recipe) -> Option<git::RepoStatus> {
    let repo = recipe.build_repo();
    let all = git::inspect_all(paths, recipe, std::iter::once(repo).chain(recipe.nested_in(repo))).await;
    all.into_iter().find(|inspected| inspected.state != RepoState::Ready)
}

pub async fn status(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> BuildStatus {
    let references: Vec<String> = recipe.run.services.iter().map(|s| image_ref(instance, recipe, s)).collect();
    let shown = references.join(", ");
    if let Err(reason) = engine().await {
        return BuildStatus::without_image(BuildState::Unknown, Some(reason), shown);
    }

    let mut blocked = None;
    let mut expected = None;
    let repo = recipe.build_repo();
    // Asked together; the source's state only counts when every checkout is complete.
    let (unready, source) = tokio::join!(unready_source(paths, recipe), git::source_state(paths, recipe, repo));
    match unready {
        None => match source {
            Ok(source) => expected = Some(fingerprint(&dockerfile(recipe, instance), &source.digest)),
            Err(e) => blocked = Some(e),
        },
        Some(inspected) => {
            blocked = Some(match inspected.state {
                RepoState::Missing => "Download the source first.".to_string(),
                _ => format!("The source needs attention: {}", inspected.detail),
            });
        }
    }

    // The fingerprint is a label on each image, so what is current is read from Docker itself.
    let mut infos = Vec::new();
    for reference in &references {
        infos.push(inspect_image(reference).await);
    }
    let tools = image_ref(instance, recipe, recipe.tools_service());
    let shown_info = references.iter().position(|r| *r == tools).and_then(|i| infos[i].clone()).or_else(|| infos.iter().flatten().next().cloned());
    let Some(info) = shown_info else {
        return BuildStatus::without_image(BuildState::NotBuilt, blocked, shown);
    };
    // Without readable source there is nothing to compare against, so do not claim it is stale.
    let complete = infos.iter().all(Option::is_some);
    let current = complete && expected.as_deref().map_or(true, |fp| infos.iter().flatten().all(|i| i.fingerprint == fp));
    let state = if current { BuildState::Ready } else { BuildState::Outdated };
    BuildStatus::with_image(state, blocked, shown, info)
}

// ---------------------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------------------

/// Everything a `docker build` needs that does not change between retries.
struct Prepared {
    instance: String,
    dockerfile: PathBuf,
    context: PathBuf,
    fingerprint: String,
    source: SourceState,
}

/// One `docker build` for one server's image. Every image is a stage of the same Dockerfile, so
/// only the first build compiles anything; the others reuse its layers.
fn docker_build(recipe: &Recipe, prepared: &Prepared, service: &ServiceSpec, jobs: u32, force: bool) -> CommandSpec {
    let mut args: Vec<String> = vec!["build".into(), "--progress=plain".into()];
    if force {
        // A rebuild from scratch also fetches a fresh base image.
        args.extend(["--no-cache".into(), "--pull".into()]);
    }
    args.extend(["--file".into(), prepared.dockerfile.to_string_lossy().into_owned()]);
    args.extend(["--build-arg".into(), format!("JOBS={jobs}")]);
    args.extend(["--target".into(), format!("service-{}", service.id)]);
    // One tag per image, so Docker Desktop lists each server once, under a name that says what it is.
    args.extend(["--tag".into(), image_ref(&prepared.instance, recipe, service)]);
    let labels = [
        (LABEL_INSTANCE, prepared.instance.clone()),
        (LABEL_RECIPE, recipe.id.clone()),
        (LABEL_FINGERPRINT, prepared.fingerprint.clone()),
        (LABEL_COMMIT, prepared.source.commit.chars().take(12).collect()),
        // Only reached when the Dockerfile's own check found the marker file in the image.
        (LABEL_PLAYERBOTS, "true".to_string()),
    ];
    for (key, value) in labels {
        args.extend(["--label".into(), format!("{key}={value}")]);
    }
    args.push(prepared.context.to_string_lossy().into_owned());
    CommandSpec { program: "docker".into(), args, ..CommandSpec::default() }.env("DOCKER_BUILDKIT", "1")
}

pub(crate) fn format_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let mib = bytes as f64 / MIB;
    if mib >= 1024.0 {
        format!("{:.1} GiB", mib / 1024.0)
    } else {
        format!("{mib:.0} MiB")
    }
}

/// Builds the server image, or confirms the current one when nothing has changed.
/// `force` rebuilds from scratch even if the image is current.
pub async fn build(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe, force: bool) -> Result<(), String> {
    paths.ensure()?;

    task.step("Checking Docker and the source");
    let docker = engine().await?;
    let repo = recipe.build_repo();
    if let Some(inspected) = unready_source(paths, recipe).await {
        return Err(format!("The {} source is not ready ({}). Download it first.", inspected.id, inspected.detail));
    }

    task.step("Preparing the build");
    let context = paths.repos.join(recipe.repo_folder(repo));
    let source = git::source_state(paths, recipe, repo).await?;
    let text = dockerfile(recipe, instance);
    let fp = fingerprint(&text, &source.digest);
    let build_dir = paths.docker.join("build");
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("Cannot create {}: {e}", build_dir.display()))?;
    let dockerfile_path = build_dir.join("Dockerfile");
    std::fs::write(&dockerfile_path, &text).map_err(|e| format!("Cannot write {}: {e}", dockerfile_path.display()))?;
    // BuildKit reads `<dockerfile>.dockerignore` when the Dockerfile is outside the context.
    let ignore_path = build_dir.join("Dockerfile.dockerignore");
    std::fs::write(&ignore_path, DOCKERIGNORE).map_err(|e| format!("Cannot write {}: {e}", ignore_path.display()))?;

    let prepared = Prepared {
        instance: instance.to_string(),
        dockerfile: dockerfile_path,
        context,
        fingerprint: fp.clone(),
        source,
    };
    let references: Vec<String> = recipe.run.services.iter().map(|s| image_ref(instance, recipe, s)).collect();
    if !force && all_current(&references, &fp).await {
        task.info(&format!("Already built from this source ({}). Nothing to compile.", references.join(", ")));
    } else {
        let mut jobs = jobs_for(docker.cpus, docker.memory);
        task.info(&format!(
            "Docker has {} CPUs and {}; compiling with {jobs} parallel job{}.",
            docker.cpus,
            format_size(docker.memory),
            if jobs == 1 { "" } else { "s" }
        ));
        for (index, service) in recipe.run.services.iter().enumerate() {
            // A rebuild from scratch applies to the first image, which compiles; the rest reuse its layers.
            let from_scratch = force && index == 0;
            loop {
                task.step(&format!(
                    "Building the {} image ({jobs} parallel job{})",
                    service.role,
                    if jobs == 1 { "" } else { "s" }
                ));
                match task.run(docker_build(recipe, &prepared, service, jobs, from_scratch)).await {
                    Ok(()) => break,
                    Err(e) if e == CANCELLED => return Err(e),
                    Err(e) => {
                        let tail = task.recent_output();
                        match retry_jobs(jobs, &tail) {
                            Some(next) => {
                                task.info(&format!(
                                    "The compiler ran out of memory with {jobs} jobs. Retrying with {next}; finished work is cached."
                                ));
                                jobs = next;
                            }
                            None => return Err(explain_failure(e, jobs, &tail)),
                        }
                    }
                }
            }
        }
    }

    for reference in &references {
        task.step(&format!("Verifying {reference}"));
        verify(task, reference, &fp, recipe).await?;
    }
    tidy(task, instance).await;
    Ok(())
}

/// Removes the builds this one replaced, when nothing uses them. Moving a tag can leave the image it
/// pointed to without a name, which some Docker setups list as `<none>` (Docker Desktop's newer image
/// store drops it by itself); that is clutter, not a backup, since the compile cache is what makes the
/// next build fast. Docker refuses to remove an image a container uses, and nothing here forces it, so
/// a running server never loses its image; the Maintenance page removes it later.
async fn tidy(task: &mut Task, instance: &str) {
    task.step("Removing the builds this one replaced");
    let filter = format!("label={LABEL_INSTANCE}={instance}");
    let out = probe("docker", &["image", "prune", "--force", "--filter", &filter]).await;
    if !out.success {
        task.info(&format!("Could not tidy the old builds ({}). The Maintenance page can remove them.", out.text.trim()));
    } else if out.text.contains("deleted:") {
        task.info("Removed the replaced build; nothing was using it.");
    } else {
        task.info("No replaced build to remove, or a container still uses it (the Maintenance page removes it later).");
    }
}

/// Checks the finished image from the outside: Docker's record of it, the playerbots marker
/// file, and that each program actually starts.
async fn verify(task: &mut Task, reference: &str, fp: &str, recipe: &Recipe) -> Result<(), String> {
    let info = inspect_image(reference)
        .await
        .ok_or_else(|| format!("Docker has no image {reference} after the build."))?;
    if info.fingerprint != fp {
        return Err(format!("{reference} was not built from this source and recipe."));
    }
    task.info(&format!("Image {reference}: built from commit {}, {}.", info.commit, format_size(info.size)));

    let prefix = &recipe.build.install_prefix;
    let marker = format!("{prefix}/{}", recipe.playerbots.marker);
    task.run(CommandSpec::new("docker", &["run", "--rm", "--entrypoint", "ls", reference, "-l", &marker]))
        .await
        .map_err(|e| format!("Playerbots are not built into the image: {e}"))?;
    task.info("Playerbots are compiled in: the marker file is in the image.");

    for smoke in &recipe.build.smoke {
        let program = format!("{prefix}/{}", smoke.run);
        let mut args = vec!["run", "--rm", "--entrypoint", program.as_str(), reference];
        args.extend(smoke.args.iter().map(String::as_str));
        task.run(CommandSpec::new("docker", &args))
            .await
            .map_err(|e| format!("{} does not run in the image: {e}", smoke.run))?;
    }
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

    #[test]
    fn the_dockerfile_builds_with_the_recipe_flags_and_proves_playerbots() {
        let text = dockerfile(&zero(), "mangos-zero");
        assert!(text.contains("FROM ubuntu:22.04 AS build"));
        assert!(text.contains("-DPLAYERBOTS=ON"), "playerbots are compiled in");
        assert!(text.contains("-DBUILD_TOOLS=ON") && text.contains("-DCMAKE_CXX_COMPILER=clang++"));
        assert!(text.contains("-DCMAKE_INSTALL_PREFIX=/mangos"));
        assert!(text.contains("libmysqlclient-dev") && text.contains("libmysqlclient21"));
        assert!(
            text.contains("test -f /mangos/etc/aiplayerbot.conf.dist"),
            "the image cannot exist without the playerbots marker"
        );
        assert!(text.contains("/mangos/bin/mangosd --version") && text.contains("/mangos/bin/realmd --version"));
        assert!(text.contains("id=worldstone-mangos-zero-ccache"), "the compiler cache is per instance");
        assert!(text.contains("\"$JOBS\"") && !text.contains("JOBS=8"), "parallelism is a build argument");
    }

    #[test]
    fn the_last_shell_command_of_the_verification_has_no_trailing_separator() {
        let text = dockerfile(&zero(), "mangos-zero");
        let run = text.lines().skip_while(|l| !l.starts_with("RUN set -eu;")).take_while(|l| !l.is_empty()).collect::<Vec<_>>();
        assert!(run.last().unwrap().trim_end().ends_with("realmd --version"), "{run:?}");
    }

    #[test]
    fn the_same_recipe_gives_the_same_dockerfile_and_a_changed_flag_gives_a_different_one() {
        let a = dockerfile(&zero(), "mangos-zero");
        assert_eq!(a, dockerfile(&zero(), "mangos-zero"));
        let mut changed = zero();
        changed.build.cmake.insert("PLAYERBOTS".into(), "OFF".into());
        assert_ne!(fingerprint(&a, "src"), fingerprint(&dockerfile(&changed, "mangos-zero"), "src"));
    }

    #[test]
    fn the_fingerprint_follows_the_source_and_is_a_valid_image_label() {
        let d = dockerfile(&zero(), "mangos-zero");
        let a = fingerprint(&d, "head 1");
        assert_eq!(a, fingerprint(&d, "head 1"));
        assert_ne!(a, fingerprint(&d, "head 2"));
        assert!(a.len() == 64 && a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn each_server_has_its_own_image_named_for_the_edition_and_the_role() {
        let recipe = zero();
        let names: Vec<String> = recipe.run.services.iter().map(|s| image_ref("mangos-zero", &recipe, s)).collect();
        assert_eq!(names, ["worldstone/vanilla-realm:latest", "worldstone/vanilla-world:latest"]);
        assert_eq!(tools_image("mangos-zero", &recipe), "worldstone/vanilla-world:latest");
        let second: Vec<String> = recipe.run.services.iter().map(|s| image_repo("second", &recipe, s)).collect();
        assert_eq!(second, ["worldstone/vanilla-second-realm", "worldstone/vanilla-second-world"], "another instance never shares an image");
        assert_eq!(legacy_repo("mangos-zero"), "worldstone/mangos-zero");
        assert_eq!(database_image("mangos-zero", &recipe), "worldstone/vanilla-database:11.4");
        assert_eq!(database_image("second", &recipe), "worldstone/vanilla-second-database:11.4");
    }

    #[test]
    fn the_dockerfile_ends_in_one_labelled_stage_per_server_that_shares_the_install() {
        let text = dockerfile(&zero(), "mangos-zero");
        assert!(text.contains("FROM runtime AS service-realmd\nLABEL io.worldstone.edition=\"vanilla\" io.worldstone.role=\"realm\"\nCMD [\"/mangos/bin/realmd\"]"), "{text}");
        assert!(text.contains("FROM runtime AS service-mangosd\nLABEL io.worldstone.edition=\"vanilla\" io.worldstone.role=\"world\"\nCMD [\"/mangos/bin/mangosd\"]"));
        assert_eq!(text.matches("COPY --from=build").count(), 1, "the install is copied once; the stages only add a label and a default program");
    }

    #[test]
    fn parallelism_follows_cpus_and_memory() {
        const GIB: u64 = 1 << 30;
        assert_eq!(jobs_for(12, 16 * GIB), 9, "16 GiB VM: (16-2)/1.5 jobs");
        assert_eq!(jobs_for(4, 64 * GIB), 4, "never more jobs than CPUs");
        assert_eq!(jobs_for(16, 4 * GIB), 1, "little memory means one job");
        assert_eq!(jobs_for(8, GIB), 1, "never zero");
        assert_eq!(jobs_for(1, 0), 1);
    }

    fn engine_info(os: &str, cpus: u64, memory: u64) -> EngineInfo {
        EngineInfo { os_type: os.into(), cpus, memory, version: "29.8.0".into() }
    }

    #[test]
    fn reads_docker_engine_facts() {
        assert_eq!(engine_of(&engine_info("linux", 12, 16_299_724_800)), Ok(Engine { cpus: 12, memory: 16_299_724_800 }));
        assert!(engine_of(&engine_info("windows", 4, 8_000_000_000)).unwrap_err().contains("Linux containers"));
        assert!(engine_of(&engine_info("linux", u64::MAX, 1)).is_err());
    }

    #[test]
    fn out_of_memory_is_recognised_and_retried_with_fewer_jobs() {
        let killed = lines(&["clang: error: unable to execute command: Killed", "ERROR: exit code: 2"]);
        let buildkit = lines(&["ERROR: process \"/bin/sh -c cmake\" did not complete successfully: exit code: 137"]);
        let ordinary = lines(&["/src/a.cpp:1:1: error: expected ';'", "make: *** [all] Error 2"]);
        assert!(looks_like_out_of_memory(&killed) && looks_like_out_of_memory(&buildkit));
        assert!(!looks_like_out_of_memory(&ordinary));
        assert_eq!(retry_jobs(8, &killed), Some(4));
        assert_eq!(retry_jobs(2, &buildkit), Some(1));
        assert_eq!(retry_jobs(1, &killed), None, "nothing lower to try");
        assert_eq!(retry_jobs(8, &ordinary), None, "a real compile error is not retried");
    }

    #[test]
    fn failures_are_explained_with_the_relevant_line() {
        let tail = lines(&["#9 40.1 /src/a.cpp:12:3: error: use of undeclared identifier 'x'", "#9 40.2 make: *** Error 1"]);
        let message = explain_failure("docker failed with exit code 1.".into(), 4, &tail);
        assert!(message.starts_with("docker failed with exit code 1."), "{message}");
        assert!(message.contains("use of undeclared identifier"), "{message}");

        let oom = explain_failure("x".into(), 1, &lines(&["exit code: 137"]));
        assert!(oom.contains("Docker Desktop more memory"), "{oom}");
        assert_eq!(explain_failure("plain".into(), 2, &[]), "plain");
    }

    #[test]
    fn apt_and_cmake_errors_count_as_the_last_error() {
        assert!(last_error_line(&lines(&["#7 2.1 E: Unable to locate package nope"])).unwrap().contains("Unable to locate"));
        assert!(last_error_line(&lines(&["CMake Error at CMakeLists.txt:163 (find_package):"])).is_some());
        assert_eq!(last_error_line(&lines(&["all fine"])), None);
    }

    fn docker_image(labels: &[(&str, &str)]) -> docker::Image {
        docker::Image {
            id: "sha256:abc".into(),
            created: "2026-09-29T05:22:11.123456789Z".into(),
            size: 412_000_000,
            labels: labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn reads_what_docker_says_about_an_image() {
        let info = image_info(docker_image(&[(LABEL_COMMIT, "a1b2c3d4e5f6"), (LABEL_PLAYERBOTS, "true"), (LABEL_FINGERPRINT, "abcdef")]));
        assert_eq!(
            info,
            ImageInfo {
                created: "2026-09-29T05:22:11.123456789Z".into(),
                size: 412_000_000,
                commit: "a1b2c3d4e5f6".into(),
                playerbots: true,
                fingerprint: "abcdef".into()
            }
        );
        assert!(!image_info(docker_image(&[(LABEL_COMMIT, "c"), (LABEL_FINGERPRINT, "f")])).playerbots, "an image without the label has no proven bots");
        assert!(!image_info(docker_image(&[(LABEL_PLAYERBOTS, "false")])).playerbots);
        assert_eq!(image_info(docker_image(&[])).fingerprint, "", "an image without labels has no fingerprint, so it is never current");
    }

    #[test]
    fn the_docker_build_command_tags_labels_and_sets_parallelism() {
        let recipe = zero();
        let prepared = Prepared {
            instance: "mangos-zero".into(),
            dockerfile: PathBuf::from("C:/x y/Dockerfile"),
            context: PathBuf::from("C:/x y/src"),
            fingerprint: "f".repeat(64),
            source: SourceState { commit: "0123456789abcdef0123456789abcdef01234567".into(), digest: String::new() },
        };
        let (realm, world) = (&recipe.run.services[0], &recipe.run.services[1]);
        let spec = docker_build(&recipe, &prepared, world, 6, false);
        let joined = spec.args.join(" ");
        assert_eq!(spec.program, "docker");
        assert!(joined.contains("--build-arg JOBS=6"));
        assert!(joined.contains("--target service-mangosd --tag worldstone/vanilla-world:latest"), "{joined}");
        assert_eq!(joined.matches("--tag").count(), 1, "one name per image, so Docker Desktop lists it once");
        assert!(docker_build(&recipe, &prepared, realm, 6, false).args.join(" ").contains("--target service-realmd --tag worldstone/vanilla-realm:latest"));
        assert!(joined.contains("io.worldstone.commit=0123456789ab") && joined.contains("io.worldstone.playerbots=true"));
        assert!(joined.contains("io.worldstone.instance=mangos-zero"), "the instance is on the image so a replaced build can be found");
        assert_eq!(spec.args.last().unwrap(), "C:/x y/src", "paths with spaces stay one argument");
        assert!(!joined.contains("--no-cache"));
        let forced = docker_build(&recipe, &prepared, world, 1, true);
        assert!(forced.args.contains(&"--no-cache".to_string()) && forced.args.contains(&"--pull".to_string()));
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(format_size(412 << 20), "412 MiB");
        assert_eq!(format_size(3 << 30), "3.0 GiB");
    }
}
