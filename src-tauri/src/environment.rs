//! What WorldStone needs on the host. Building, extraction and the database all run inside
//! containers, so the host needs only Git and a running Docker engine with Linux containers.

use crate::{
    docker::{self, EngineInfo},
    models::{Check, Status},
    process::{probe, Probe},
};
use std::{collections::BTreeMap, future::Future, path::PathBuf, sync::Mutex};

/// Version answers, each with what it was asked of. A tool's version can only change when its
/// program does, so an answer is reused for exactly as long as the program is the same file.
static KNOWN: Mutex<BTreeMap<String, (String, Probe)>> = Mutex::new(BTreeMap::new());

/// The program a bare name runs: the first `name.exe` on the search path, and what identifies that
/// file (where it is, how big, when it was last written). `None` when there is none.
fn fingerprint(program: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate: PathBuf = dir.join(format!("{program}.exe"));
        let meta = std::fs::metadata(&candidate).ok().filter(std::fs::Metadata::is_file)?;
        Some(format!("{}|{}|{:?}", candidate.display(), meta.len(), meta.modified().ok()))
    })
}

/// `ask`'s answer, or the one given the last time when `fingerprint` (which says what the answer
/// depends on) has not changed. Only a success is remembered: a tool that was missing is asked again.
async fn remembered(id: &str, fingerprint: Option<String>, ask: impl Future<Output = Probe>) -> Probe {
    if let Some(now) = &fingerprint {
        let known = KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, answer)) = known.get(id).filter(|(then, _)| then == now) {
            return answer.clone();
        }
    }
    let answer = ask.await;
    if let Some(now) = fingerprint.filter(|_| answer.success) {
        KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.to_string(), (now, answer.clone()));
    }
    answer
}

pub async fn check() -> Vec<Check> {
    let docker_file = fingerprint("docker");
    // Compose is a plugin of Docker Desktop that is replaced when Docker is updated, which also
    // changes what the engine reports, so the two together say whether it is still the same one.
    let engine = docker::engine_info().await;
    let compose_of = docker_file.clone().zip(engine.as_ref().ok()).map(|(file, info)| format!("{file}|{}", info.version));
    let (git, docker, compose) = tokio::join!(
        remembered("git", fingerprint("git"), probe("git", &["--version"])),
        remembered("docker", docker_file, probe("docker", &["--version"])),
        remembered("compose", compose_of, probe("docker", &["compose", "version", "--short"])),
    );
    let docker_present = docker.success;
    vec![
        tool("git", "Git", &git, "Install Git for Windows."),
        tool("docker", "Docker", &docker, "Install Docker Desktop."),
        engine_check(&engine, docker_present),
        compose_check(&compose, docker_present),
    ]
}

fn tool(id: &str, label: &str, probe: &Probe, hint: &str) -> Check {
    if probe.success {
        Check::new(id, label, Status::Ready, probe.text.lines().next().unwrap_or(""))
    } else {
        Check::new(id, label, Status::Missing, hint)
    }
}

pub fn engine_check(engine: &Result<EngineInfo, String>, docker_present: bool) -> Check {
    let label = "Docker engine";
    if !docker_present {
        return Check::new("docker-engine", label, Status::Unknown, "Docker is not installed.");
    }
    match engine {
        Err(reason) if reason == docker::NOT_RUNNING => {
            Check::new("docker-engine", label, Status::Missing, "Docker is installed but not running. Start Docker Desktop.")
        }
        Err(reason) => Check::new("docker-engine", label, Status::Warning, reason.clone()),
        Ok(info) if info.os_type == "linux" => Check::new("docker-engine", label, Status::Ready, format!("Linux containers, version {}", info.version)),
        Ok(info) => Check::new(
            "docker-engine",
            label,
            Status::Warning,
            format!("Docker is using {} containers. Switch Docker Desktop to Linux containers.", info.os_type),
        ),
    }
}

pub fn compose_check(compose: &Probe, docker_present: bool) -> Check {
    let label = "Docker Compose";
    if !docker_present {
        return Check::new("docker-compose", label, Status::Unknown, "Docker is not installed.");
    }
    if compose.success {
        Check::new("docker-compose", label, Status::Ready, format!("Version {}", compose.text.trim()))
    } else {
        Check::new("docker-compose", label, Status::Missing, "Compose ships with Docker Desktop; update or reinstall it.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn probe(success: bool, text: &str) -> Probe {
        Probe { success, text: text.into() }
    }

    fn engine(os: &str) -> Result<EngineInfo, String> {
        Ok(EngineInfo { os_type: os.into(), cpus: 8, memory: 8 << 30, version: "29.8.0".into() })
    }

    #[test]
    fn linux_containers_are_ready() {
        let c = engine_check(&engine("linux"), true);
        assert_eq!(c.status, Status::Ready);
        assert!(c.detail.contains("29.8.0"));
    }

    #[test]
    fn windows_containers_are_a_warning_with_a_fix() {
        let c = engine_check(&engine("windows"), true);
        assert_eq!(c.status, Status::Warning);
        assert!(c.detail.contains("Linux containers"));
    }

    #[test]
    fn an_installed_but_stopped_engine_says_to_start_it() {
        let c = engine_check(&Err(docker::NOT_RUNNING.to_string()), true);
        assert_eq!(c.status, Status::Missing);
        assert!(c.detail.contains("Start Docker Desktop"));
    }

    #[test]
    fn an_engine_that_answers_badly_says_what_it_said() {
        let c = engine_check(&Err("Docker gave an empty answer.".to_string()), true);
        assert_eq!(c.status, Status::Warning);
        assert!(c.detail.contains("empty answer"));
    }

    #[test]
    fn engine_and_compose_are_unknown_without_docker() {
        assert_eq!(engine_check(&engine("linux"), false).status, Status::Unknown);
        assert_eq!(compose_check(&probe(false, ""), false).status, Status::Unknown);
    }

    #[tokio::test]
    async fn a_version_is_asked_again_only_when_its_program_changed() {
        let asked = AtomicUsize::new(0);
        let ask = |text: &'static str| {
            let asked = &asked;
            async move {
                asked.fetch_add(1, Ordering::SeqCst);
                probe(true, text)
            }
        };
        let id = "test-version-cache";
        assert_eq!(remembered(id, Some("v1".into()), ask("1.0")).await.text, "1.0");
        assert_eq!(remembered(id, Some("v1".into()), ask("never asked")).await.text, "1.0", "same program, same answer");
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert_eq!(remembered(id, Some("v2".into()), ask("2.0")).await.text, "2.0", "a changed program is asked again");
        assert_eq!(remembered(id, None, ask("3.0")).await.text, "3.0", "an unknown program is never assumed to be the same");
        assert_eq!(remembered(id, Some("v2".into()), ask("never asked")).await.text, "2.0");
        assert_eq!(asked.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_failure_is_never_remembered() {
        let id = "test-failure-cache";
        assert!(!remembered(id, Some("v1".into()), async { probe(false, "not installed") }).await.success);
        assert!(remembered(id, Some("v1".into()), async { probe(true, "1.0") }).await.success, "installing it shows at once");
    }

    #[test]
    fn a_program_that_is_not_on_the_path_has_no_fingerprint() {
        assert_eq!(fingerprint("worldstone-no-such-program"), None);
    }
}
