//! Repository management: inspect, clone and fast-forward update.
//!
//! Safety rules: updates only ever fast-forward, so local edits are never reset; a clone
//! is staged in `Temp` and moved into place only when complete; a folder that exists but
//! is not a Git checkout is reported, never deleted.

use crate::{
    paths::InstancePaths,
    process::{probe, CommandSpec, Probe},
    recipe::{Recipe, RepoSpec},
    tasks::Task,
};
use futures_util::future::join_all;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum RepoState {
    Missing,
    Ready,
    Incomplete,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatus {
    pub id: String,
    pub url: String,
    pub branch: String,
    pub state: RepoState,
    pub commit: Option<String>,
    pub summary: Option<String>,
    pub dirty: bool,
    pub detail: String,
}

/// Every Git command runs with these, passed through the environment so they also reach
/// submodules. Checkouts are consumed inside Linux containers, so line endings must not be
/// rewritten to CRLF, and Windows needs long paths for the deep source trees.
fn git(args: &[&str]) -> CommandSpec {
    #[allow(unused_mut)] // only the tests add to it
    let mut config = vec![("core.autocrlf", "false"), ("core.longpaths", "true")];
    // Tests clone submodules from local paths, which git blocks unless the file transport is
    // allowed. Real repositories are fetched over https and never need this.
    #[cfg(test)]
    config.push(("protocol.file.allow", "always"));

    let mut spec = CommandSpec::new("git", args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_COUNT", &config.len().to_string());
    for (i, (key, value)) in config.iter().enumerate() {
        spec = spec.env(&format!("GIT_CONFIG_KEY_{i}"), key).env(&format!("GIT_CONFIG_VALUE_{i}"), value);
    }
    spec
}

/// `git submodule status` lines: a leading `-` marks a submodule that is not checked out.
pub fn parse_submodule_status(text: &str) -> Vec<(bool, String)> {
    text.lines()
        .filter_map(|line| {
            let initialized = !line.starts_with('-');
            let rest = line.trim_start_matches(['-', '+', 'U', ' ']);
            let path = rest.split_whitespace().nth(1)?;
            Some((initialized, path.to_string()))
        })
        .collect()
}

/// Where a repository is checked out, and the folders inside it that belong to repositories
/// cloned into it (its own status and source state leave those out).
fn location(paths: &InstancePaths, recipe: &Recipe, repo: &RepoSpec) -> (PathBuf, Vec<String>) {
    let nested = recipe.nested_in(repo).map(|r| r.path.clone()).collect();
    (paths.repos.join(recipe.repo_folder(repo)), nested)
}

/// The trailing arguments that leave nested repositories out of a `git status` or `git diff`.
fn without(nested: &[String]) -> Vec<String> {
    if nested.is_empty() {
        return Vec::new();
    }
    ["--".to_string(), ".".to_string()].into_iter().chain(nested.iter().map(|n| format!(":(exclude){n}"))).collect()
}

pub async fn inspect(paths: &InstancePaths, recipe: &Recipe, repo: &RepoSpec) -> RepoStatus {
    let (dest, nested) = location(paths, recipe, repo);
    inspect_at(&dest, &nested, repo).await
}

/// [`inspect`] for several repositories at once, in the order given.
pub async fn inspect_all<'a>(paths: &InstancePaths, recipe: &Recipe, repos: impl IntoIterator<Item = &'a RepoSpec>) -> Vec<RepoStatus> {
    join_all(repos.into_iter().map(|repo| inspect(paths, recipe, repo))).await
}

async fn inspect_at(dest: &Path, nested: &[String], repo: &RepoSpec) -> RepoStatus {
    let status = |state, commit, summary, dirty, detail: &str| RepoStatus {
        id: repo.id.clone(),
        url: repo.url.clone(),
        branch: repo.branch.clone(),
        state,
        commit,
        summary,
        dirty,
        detail: detail.to_string(),
    };
    if !dest.exists() {
        return status(RepoState::Missing, None, None, false, "Not downloaded yet");
    }
    if !dest.join(".git").exists() {
        return status(RepoState::Incomplete, None, None, false, "The folder exists but is not a Git checkout");
    }
    let dir = dest.to_string_lossy();
    let exclude = without(nested);
    let mut status_args = vec!["-C", &dir, "status", "--porcelain"];
    status_args.extend(exclude.iter().map(String::as_str));
    let log_args = ["-C", &dir, "log", "-1", "--format=%h%x09%s"];
    let submodule_args = ["-C", &dir, "submodule", "status", "--recursive"];
    // Three independent questions, asked together.
    let (log, changes, subs) = tokio::join!(
        probe("git", &log_args),
        probe("git", &status_args),
        async {
            match repo.submodules {
                true => Some(probe("git", &submodule_args).await),
                false => None,
            }
        },
    );
    let Some((commit, summary)) = log.success.then(|| log.text.split_once('\t')).flatten() else {
        return status(RepoState::Incomplete, None, None, false, "The checkout is damaged or has no commits");
    };
    let dirty = changes.text.lines().next().is_some();
    if let Some(subs) = subs {
        let missing: Vec<_> = parse_submodule_status(&subs.text)
            .into_iter()
            .filter(|(ok, _)| !ok)
            .map(|(_, p)| p)
            .collect();
        if !subs.success || !missing.is_empty() {
            let detail = format!("Submodules not downloaded: {}", missing.join(", "));
            return status(RepoState::Incomplete, Some(commit.into()), Some(summary.into()), dirty, &detail);
        }
    }
    let detail = if dirty { "Ready, with local changes" } else { "Ready" };
    status(RepoState::Ready, Some(commit.into()), Some(summary.into()), dirty, detail)
}

/// What a build of a checkout would compile, as one comparable value.
#[derive(Clone, Debug)]
pub struct SourceState {
    pub commit: String,
    /// Changes whenever the compiled source does: HEAD, every submodule's commit, and any
    /// uncommitted or untracked edits (which files, and a hash of what changed in them).
    pub digest: String,
}

/// The state of what a build of this repository compiles: its checkout, plus the repositories
/// cloned into it, each with its own commit and edits.
pub async fn source_state(paths: &InstancePaths, recipe: &Recipe, repo: &RepoSpec) -> Result<SourceState, String> {
    let (dest, nested) = location(paths, recipe, repo);
    let children: Vec<(&RepoSpec, PathBuf)> = recipe.nested_in(repo).map(|child| (child, paths.repos.join(recipe.repo_folder(child)))).collect();
    let (own, inner) = tokio::join!(state_at(&dest, &nested), join_all(children.iter().map(|(_, dir)| state_at(dir, &[]))));
    let mut state = own?;
    for ((child, _), inner) in children.iter().zip(inner) {
        state.digest.push_str(&format!("\nnested {}\n{}", child.id, inner?.digest));
    }
    Ok(state)
}

/// What `git submodule status --recursive` prints. That command is slow on Windows (it runs a shell
/// script; about 0.8 s), so a checkout that has no submodule at all, which is what it prints
/// nothing for, is recognised from Git's index instead (about 0.1 s). Anything unusual, such as a
/// checkout with a `.gitmodules` file or an index that cannot be read, still asks the command.
async fn submodule_status(dir: &Path) -> Probe {
    let d = dir.to_string_lossy();
    if !dir.join(".gitmodules").exists() {
        let index = probe("git", &["-C", &d, "ls-files", "--stage"]).await;
        if index.success && !index.text.lines().any(|entry| entry.starts_with("160000")) {
            return Probe { success: true, text: String::new() };
        }
    }
    probe("git", &["-C", &d, "submodule", "status", "--recursive"]).await
}

async fn state_at(dir: &Path, nested: &[String]) -> Result<SourceState, String> {
    let d = dir.to_string_lossy();
    let exclude = without(nested);
    let mut status_args = vec!["-C", &d, "status", "--porcelain", "--untracked-files=all"];
    status_args.extend(exclude.iter().map(String::as_str));
    let head_args = ["-C", &d, "rev-parse", "HEAD"];
    let (head, subs, changes) = tokio::join!(probe("git", &head_args), submodule_status(dir), probe("git", &status_args));
    if !head.success {
        return Err(format!("Cannot read the source revision: {}", head.text));
    }
    let mut diff_hash = String::new();
    if !changes.text.is_empty() {
        let mut diff_args = vec!["-C", &d, "diff", "HEAD"];
        diff_args.extend(exclude.iter().map(String::as_str));
        let diff = probe("git", &diff_args).await;
        diff_hash = format!("{:x}", Sha256::digest(diff.text.as_bytes()));
    }
    Ok(SourceState {
        commit: head.text.clone(),
        digest: format!("head {}\nsubmodules\n{}\nchanges\n{}\ndiff {}", head.text, subs.text, changes.text, diff_hash),
    })
}

/// Downloads every repository in the recipe, or fast-forwards those already present.
pub async fn sync_all(task: &mut Task, paths: &InstancePaths, recipe: &Recipe) -> Result<(), String> {
    paths.ensure()?;
    for repo in &recipe.repos {
        task.step(&format!("Repository: {} ({})", repo.id, repo.url));
        sync(task, paths, recipe, repo).await?;
    }
    Ok(())
}

async fn sync(task: &mut Task, paths: &InstancePaths, recipe: &Recipe, repo: &RepoSpec) -> Result<(), String> {
    sync_at(task, paths, repo, &paths.repos.join(recipe.repo_folder(repo))).await
}

async fn sync_at(task: &mut Task, paths: &InstancePaths, repo: &RepoSpec, dest: &Path) -> Result<(), String> {
    if !dest.exists() {
        clone(task, paths, repo, dest).await
    } else if dest.join(".git").exists() {
        update(task, repo, dest).await
    } else {
        Err(format!(
            "{} exists but is not a Git checkout. Move it aside and run setup again; WorldStone will not delete it.",
            dest.display()
        ))
    }
}

async fn clone(task: &mut Task, paths: &InstancePaths, repo: &RepoSpec, dest: &Path) -> Result<(), String> {
    let staging = paths.temp.join(format!("clone-{}", repo.id));
    if staging.exists() {
        // Only ever an interrupted download of our own: a finished clone is moved out.
        task.info("Discarding an interrupted download and starting again");
        std::fs::remove_dir_all(&staging).map_err(|e| format!("Cannot clear {}: {e}", staging.display()))?;
    }
    let staging_arg = staging.to_string_lossy().into_owned();
    let mut args = vec!["clone", "--progress", "--branch", &repo.branch];
    if repo.submodules {
        args.push("--recurse-submodules");
    }
    args.extend(["--", &repo.url, &staging_arg]);
    task.run(git(&args)).await?;
    // A repository cloned into another goes into a folder that may not exist yet.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    std::fs::rename(&staging, dest).map_err(|e| format!("Cannot move the finished download into place: {e}"))
}

async fn update(task: &mut Task, repo: &RepoSpec, dest: &Path) -> Result<(), String> {
    let dir = dest.to_string_lossy().into_owned();
    let head = probe("git", &["-C", &dir, "rev-parse", "--abbrev-ref", "HEAD"]).await;
    if !head.success || head.text != repo.branch {
        return Err(format!(
            "{} is on '{}', not '{}'. WorldStone will not switch branches; switch back or leave it as is.",
            repo.path, head.text, repo.branch
        ));
    }
    task.run(git(&["-C", &dir, "fetch", "--progress", "origin"])).await?;
    let upstream = format!("origin/{}", repo.branch);
    task.run(git(&["-C", &dir, "merge", "--ff-only", &upstream])).await.map_err(|e| {
        format!("{e} Local commits or edits may be in the way; nothing was reset.")
    })?;
    if repo.submodules {
        task.run(git(&["-C", &dir, "submodule", "update", "--init", "--recursive", "--progress"])).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{paths::Paths, tasks::test_support::manager};
    use std::{fs, path::PathBuf};

    /// Runs real git for test setup and asserts it worked.
    fn run_git(dir: &Path, args: &[&str]) {
        let mut command = std::process::Command::new("git");
        command
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir);
        // As the app does: another test may be cancelling a command, which briefly changes this
        // process's console, and a child without this flag can fail to start in that moment.
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
        let out = command.output().expect("git must be installed to run these tests");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn commit_file(repo: &Path, name: &str, content: &str) {
        fs::write(repo.join(name), content).unwrap();
        run_git(repo, &["add", "."]);
        run_git(repo, &["commit", "-m", &format!("edit {name}")]);
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        origin: PathBuf,
        paths: InstancePaths,
        repo: RepoSpec,
        tasks: crate::tasks::TaskManager,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        fs::create_dir_all(&origin).unwrap();
        run_git(&origin, &["init", "-b", "main"]);
        commit_file(&origin, "a.txt", "one");
        let paths = Paths::at(dir.path().join("home")).instance("test").unwrap();
        paths.ensure().unwrap();
        let repo = RepoSpec {
            id: "r".into(),
            url: origin.to_string_lossy().into_owned(),
            branch: "main".into(),
            path: "r".into(),
            into: None,
            submodules: false,
        };
        let (tasks, _) = manager(dir.path());
        Fixture { _dir: dir, origin, paths, repo, tasks }
    }

    async fn sync_now(f: &Fixture) -> Result<(), String> {
        let mut task = f.tasks.begin("test", true).unwrap();
        let result = sync_at(&mut task, &f.paths, &f.repo, &f.paths.repos.join("r")).await;
        task.finish(result.clone());
        result
    }

    #[tokio::test]
    async fn clones_then_fast_forwards() {
        let f = fixture();
        assert_eq!(inspect_at(&f.paths.repos.join("r"), &[], &f.repo).await.state, RepoState::Missing);
        sync_now(&f).await.unwrap();
        let dest = f.paths.repos.join("r");
        let status = inspect_at(&f.paths.repos.join("r"), &[], &f.repo).await;
        assert_eq!(status.state, RepoState::Ready);
        assert!(!status.dirty && status.commit.is_some());
        assert!(!f.paths.temp.join("clone-r").exists(), "staging is moved out on success");

        commit_file(&f.origin, "b.txt", "two");
        sync_now(&f).await.unwrap();
        assert_eq!(fs::read_to_string(dest.join("b.txt")).unwrap(), "two");
    }

    #[tokio::test]
    async fn checkouts_keep_lf_line_endings_whatever_the_user_git_config_says() {
        let f = fixture();
        fs::write(f.origin.join("script.sh"), "echo hi\n").unwrap();
        run_git(&f.origin, &["add", "."]);
        run_git(&f.origin, &["commit", "-m", "script"]);
        sync_now(&f).await.unwrap();
        let bytes = fs::read(f.paths.repos.join("r").join("script.sh")).unwrap();
        assert_eq!(bytes, b"echo hi\n", "no CRLF conversion");
    }

    #[tokio::test]
    async fn an_update_never_discards_local_edits() {
        let f = fixture();
        sync_now(&f).await.unwrap();
        let dest = f.paths.repos.join("r");
        fs::write(dest.join("a.txt"), "my local edit").unwrap();
        assert!(inspect_at(&f.paths.repos.join("r"), &[], &f.repo).await.dirty);

        // Upstream changes the same file: the fast-forward must refuse rather than overwrite.
        commit_file(&f.origin, "a.txt", "upstream edit");
        let err = sync_now(&f).await.unwrap_err();
        assert!(err.contains("nothing was reset"), "{err}");
        assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "my local edit");
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_checkout_is_reported_and_left_alone() {
        let f = fixture();
        let dest = f.paths.repos.join("r");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("precious.txt"), "keep me").unwrap();
        assert_eq!(inspect_at(&f.paths.repos.join("r"), &[], &f.repo).await.state, RepoState::Incomplete);
        let err = sync_now(&f).await.unwrap_err();
        assert!(err.contains("will not delete it"), "{err}");
        assert_eq!(fs::read_to_string(dest.join("precious.txt")).unwrap(), "keep me");
    }

    #[tokio::test]
    async fn an_update_will_not_switch_branches() {
        let f = fixture();
        sync_now(&f).await.unwrap();
        run_git(&f.paths.repos.join("r"), &["checkout", "-b", "feature"]);
        let err = sync_now(&f).await.unwrap_err();
        assert!(err.contains("will not switch branches"), "{err}");
    }

    #[tokio::test]
    async fn an_interrupted_download_is_replaced_by_a_clean_one() {
        let f = fixture();
        let staging = f.paths.temp.join("clone-r");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("half-a-clone"), "x").unwrap();
        sync_now(&f).await.unwrap();
        assert_eq!(inspect_at(&f.paths.repos.join("r"), &[], &f.repo).await.state, RepoState::Ready);
        assert!(!f.paths.repos.join("r").join("half-a-clone").exists());
    }

    /// The real bug: mangoszero/database keeps its Realm schema in a submodule, and a
    /// checkout without it looked "Ready" while Realm/ was empty.
    #[tokio::test]
    async fn a_checkout_missing_a_submodule_is_incomplete_and_sync_repairs_it() {
        let mut f = fixture();
        // Git refuses local-path submodules unless the file transport is allowed. Allow it for
        // these repositories only: a process-wide setting would leak into tests running alongside.
        let allow_file = ["-c", "protocol.file.allow=always"];

        let sub = f._dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        run_git(&sub, &["init", "-b", "main"]);
        commit_file(&sub, "schema.sql", "CREATE TABLE t;");
        run_git(&f.origin, &[allow_file[0], allow_file[1], "submodule", "add", &sub.to_string_lossy(), "Realm"]);
        run_git(&f.origin, &["commit", "-m", "add the Realm submodule"]);
        f.repo.submodules = true;

        // A checkout made without --recurse-submodules, as the buggy recipe produced.
        let dest = f.paths.repos.join("r");
        let dest_arg = dest.to_string_lossy().into_owned();
        run_git(&f.paths.repos, &["clone", "--branch", "main", &f.repo.url, &dest_arg]);
        assert_eq!(fs::read_dir(dest.join("Realm")).unwrap().count(), 0, "Realm/ starts empty");

        let status = inspect_at(&f.paths.repos.join("r"), &[], &f.repo).await;
        assert_eq!(status.state, RepoState::Incomplete);
        assert!(status.detail.contains("Realm"), "{}", status.detail);

        sync_now(&f).await.unwrap();
        assert_eq!(inspect_at(&f.paths.repos.join("r"), &[], &f.repo).await.state, RepoState::Ready);
        assert_eq!(fs::read_to_string(dest.join("Realm").join("schema.sql")).unwrap(), "CREATE TABLE t;");
        // A checkout that has a submodule is never taken for one that has none.
        assert!(submodule_status(&dest).await.text.contains("Realm"));
    }

    #[test]
    fn reads_submodule_status_lines() {
        let text = " 3f2a1b dep (heads/master)\n-9c8d7e src/realmd\n+abc123 src/modules/SD3 (v1)\nU111222 src/modules/Eluna\n";
        assert_eq!(
            parse_submodule_status(text),
            [
                (true, "dep".to_string()),
                (false, "src/realmd".to_string()),
                (true, "src/modules/SD3".to_string()),
                (true, "src/modules/Eluna".to_string()),
            ]
        );
    }

    #[test]
    fn empty_status_has_no_submodules() {
        assert!(parse_submodule_status("").is_empty());
    }

    #[tokio::test]
    async fn the_source_state_changes_exactly_when_the_compiled_source_does() {
        let f = fixture();
        sync_now(&f).await.unwrap();
        let dest = f.paths.repos.join("r");
        let first = state_at(&dest, &[]).await.unwrap();
        assert_eq!(state_at(&dest, &[]).await.unwrap().digest, first.digest, "stable while nothing changes");
        assert_eq!(first.commit.len(), 40);

        fs::write(dest.join("a.txt"), "edited").unwrap();
        let edited = state_at(&dest, &[]).await.unwrap();
        assert_ne!(edited.digest, first.digest, "a tracked edit");

        fs::write(dest.join("a.txt"), "edited again").unwrap();
        assert_ne!(state_at(&dest, &[]).await.unwrap().digest, edited.digest, "a further edit to the same file");

        fs::write(dest.join("a.txt"), "one").unwrap();
        assert_eq!(state_at(&dest, &[]).await.unwrap().digest, first.digest, "reverting restores the state");

        fs::write(dest.join("new.txt"), "x").unwrap();
        assert_ne!(state_at(&dest, &[]).await.unwrap().digest, first.digest, "an untracked file is copied into the build");
    }

    #[tokio::test]
    async fn the_shortcut_for_a_checkout_without_submodules_says_what_git_says() {
        let f = fixture();
        sync_now(&f).await.unwrap();
        let dest = f.paths.repos.join("r");
        let real = probe("git", &["-C", &dest.to_string_lossy(), "submodule", "status", "--recursive"]).await;
        let quick = submodule_status(&dest).await;
        assert_eq!((quick.success, quick.text.as_str()), (real.success, real.text.as_str()));
        assert_eq!(quick.text, "");
    }

    #[tokio::test]
    async fn the_source_state_of_a_missing_checkout_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(state_at(&dir.path().join("nope"), &[]).await.is_err());
    }

    fn azerothcore() -> Recipe {
        crate::recipe::builtin().unwrap().into_iter().find(|r| r.id == "azerothcore").unwrap()
    }

    /// A checkout made by hand: the recipe's repositories are cloned over the network in real use.
    fn checkout(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        run_git(dir, &["init", "-b", "main"]);
        commit_file(dir, "a.txt", "one");
    }

    #[tokio::test]
    async fn a_repository_cloned_inside_another_is_part_of_its_source_but_not_its_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::at(tmp.path().to_path_buf()).instance("azerothcore").unwrap();
        let recipe = azerothcore();
        let (server, module) = (&recipe.repos[0], &recipe.repos[1]);
        assert_eq!(module.into.as_deref(), Some(server.id.as_str()));
        checkout(&paths.repos.join(recipe.repo_folder(server)));
        checkout(&paths.repos.join(recipe.repo_folder(module)));
        assert!(paths.repos.join("azerothcore-wotlk/modules/mod-playerbots/.git").exists());

        let status = inspect(&paths, &recipe, server).await;
        assert_eq!(status.state, RepoState::Ready);
        assert!(!status.dirty, "the module's folder is not the server's edit: {}", status.detail);
        assert_eq!(inspect(&paths, &recipe, module).await.state, RepoState::Ready);

        let first = source_state(&paths, &recipe, server).await.unwrap();
        assert!(first.digest.contains("nested playerbots"));
        assert_eq!(source_state(&paths, &recipe, server).await.unwrap().digest, first.digest, "stable");

        // An edit inside the module changes what is built, and the server stays clean.
        fs::write(paths.repos.join("azerothcore-wotlk/modules/mod-playerbots/a.txt"), "edited").unwrap();
        assert_ne!(source_state(&paths, &recipe, server).await.unwrap().digest, first.digest);
        assert!(inspect(&paths, &recipe, module).await.dirty);
        assert!(!inspect(&paths, &recipe, server).await.dirty);

        // An edit in the server itself is still seen.
        fs::write(paths.repos.join("azerothcore-wotlk/a.txt"), "edited").unwrap();
        assert!(inspect(&paths, &recipe, server).await.dirty);
    }

    #[tokio::test]
    async fn a_nested_repository_is_cloned_into_its_place_inside_the_parent() {
        let f = fixture();
        sync_now(&f).await.unwrap();
        let child_origin = f._dir.path().join("child-origin");
        checkout(&child_origin);
        let child = RepoSpec {
            id: "c".into(),
            url: child_origin.to_string_lossy().into_owned(),
            branch: "main".into(),
            path: "mods/child".into(),
            into: Some("r".into()),
            submodules: false,
        };
        let dest = f.paths.repos.join("r").join("mods").join("child");
        let mut task = f.tasks.begin("test", true).unwrap();
        let result = sync_at(&mut task, &f.paths, &child, &dest).await;
        task.finish(result.clone());
        result.unwrap();
        assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "one");
        assert!(!f.paths.temp.join("clone-c").exists());
    }

    #[test]
    fn git_commands_keep_line_endings_and_never_prompt() {
        let spec = git(&["status"]);
        let has = |k: &str, v: &str| spec.env.iter().any(|(a, b)| a == k && b == v);
        assert!(has("GIT_CONFIG_KEY_0", "core.autocrlf") && has("GIT_CONFIG_VALUE_0", "false"));
        assert!(has("GIT_CONFIG_KEY_1", "core.longpaths") && has("GIT_CONFIG_VALUE_1", "true"));
        assert!(has("GIT_TERMINAL_PROMPT", "0"));
    }
}
