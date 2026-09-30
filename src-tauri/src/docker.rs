//! What the app asks Docker, in the app's own terms.
//!
//! Each function asks the engine directly (see `dockerapi.rs`) and answers in a few milliseconds.
//! Only when the engine is not on a local pipe does one fall back to the `docker` program, which
//! prints the same JSON the engine sends, so both routes share one reader.
//!
//! Nothing here remembers an answer for more than a moment (see [`dockerapi::Coalesced`]): a card
//! shows what Docker said just now.

use crate::{
    dockerapi::{self, ApiError, Coalesced},
    process::{probe, Probe},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};

pub const NOT_RUNNING: &str = "Docker is not running. Start Docker Desktop.";

const PROJECT_LABEL: &str = "com.docker.compose.project";

fn describe(error: ApiError) -> String {
    match error {
        ApiError::Unreachable(_) => NOT_RUNNING.to_string(),
        other => other.to_string(),
    }
}

/// `docker <kind> inspect <name>` prints what the engine sends, as an array of one.
async fn inspect_by_program(kind: &str, name: &str) -> Option<Value> {
    let out = probe("docker", &[kind, "inspect", name]).await;
    out.success.then(|| serde_json::from_str::<Value>(&out.text).ok()).flatten()?.get(0).cloned()
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineInfo {
    /// `linux` or `windows`: which kind of containers Docker runs.
    pub os_type: String,
    pub cpus: u64,
    /// Bytes of memory the engine's virtual machine has.
    pub memory: u64,
    pub version: String,
}

pub fn info_from_json(info: &Value) -> Result<EngineInfo, String> {
    let os_type = text(info, "OSType");
    if os_type.is_empty() {
        return Err("Docker gave an empty answer.".to_string());
    }
    match (info.get("NCPU").and_then(Value::as_u64), info.get("MemTotal").and_then(Value::as_u64)) {
        (Some(cpus), Some(memory)) => Ok(EngineInfo { os_type, cpus, memory, version: text(info, "ServerVersion") }),
        _ => Err("Docker gave an unexpected answer about its resources.".to_string()),
    }
}

async fn info_by_program() -> Result<EngineInfo, String> {
    let out = probe("docker", &["info", "--format", "{{json .}}"]).await;
    if !out.success {
        return Err(NOT_RUNNING.to_string());
    }
    let json: Value = serde_json::from_str(&out.text).map_err(|_| "Docker gave an unexpected answer.".to_string())?;
    info_from_json(&json)
}

static INFO: Coalesced<Result<EngineInfo, String>> = dockerapi::shared();

/// Whether Docker answers, and what it runs on. Every card asks, so the answer is shared.
pub async fn engine_info() -> Result<EngineInfo, String> {
    INFO.get(|| async {
        match dockerapi::get_json("/info").await {
            Ok(json) => info_from_json(&json),
            Err(ApiError::Unsupported) => info_by_program().await,
            Err(other) => Err(describe(other)),
        }
    })
    .await
}

// ---------------------------------------------------------------------------------------
// Images, containers, volumes
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub id: String,
    /// When it was made, as Docker prints it (RFC 3339 in UTC).
    pub created: String,
    pub size: u64,
    pub labels: BTreeMap<String, String>,
}

impl Image {
    pub fn label(&self, name: &str) -> String {
        self.labels.get(name).cloned().unwrap_or_default()
    }
}

pub fn image_from_json(image: &Value) -> Option<Image> {
    let labels = image
        .get("Config")
        .and_then(|c| c.get("Labels"))
        .and_then(Value::as_object)
        .map(|l| l.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    Some(Image { id: text(image, "Id"), created: text(image, "Created"), size: image.get("Size")?.as_u64()?, labels }).filter(|i| !i.id.is_empty())
}

/// The image with this name, tag or id; `None` when there is none, or Docker cannot be asked.
pub async fn image(reference: &str) -> Option<Image> {
    let json = match dockerapi::find_json(&format!("/images/{}/json", dockerapi::encode(reference, "/:@"))).await {
        Ok(found) => found?,
        Err(ApiError::Unsupported) => inspect_by_program("image", reference).await?,
        Err(_) => return None,
    };
    image_from_json(&json)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerState {
    /// `created`, `running`, `paused`, `restarting`, `exited`, `dead`.
    pub status: String,
    /// The health check's verdict (`starting`, `healthy`, `unhealthy`); `None` without a check.
    pub health: Option<String>,
    pub exit_code: i64,
    /// When the current run began; year 1 when it never did.
    pub started_at: String,
}

pub fn container_from_json(container: &Value) -> Option<ContainerState> {
    let state = container.get("State")?;
    Some(ContainerState {
        status: text(state, "Status"),
        health: state.get("Health").and_then(|h| h.get("Status")).and_then(Value::as_str).map(str::to_string),
        exit_code: state.get("ExitCode").and_then(Value::as_i64).unwrap_or(0),
        started_at: text(state, "StartedAt"),
    })
}

/// What a container is doing right now; `None` when there is no such container or Docker cannot be
/// asked. Never shared or remembered: a server's state is the one thing that must be current.
pub async fn container(name: &str) -> Option<ContainerState> {
    let json = match dockerapi::find_json(&format!("/containers/{}/json", dockerapi::encode(name, ""))).await {
        Ok(found) => found?,
        Err(ApiError::Unsupported) => inspect_by_program("container", name).await?,
        Err(_) => return None,
    };
    container_from_json(&json)
}

pub async fn volume_exists(name: &str) -> bool {
    match dockerapi::find_json(&format!("/volumes/{}", dockerapi::encode(name, ""))).await {
        Ok(found) => found.is_some(),
        Err(ApiError::Unsupported) => inspect_by_program("volume", name).await.is_some(),
        Err(_) => false,
    }
}

/// One container as a list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    /// The id of the image it was made from (`sha256:...`).
    pub image_id: String,
    /// The Compose project it belongs to.
    pub project: Option<String>,
    /// The ports it publishes on this machine.
    pub public_ports: Vec<u16>,
}

/// One entry of the engine's container list.
pub fn listed_from_list(entry: &Value) -> Option<Listed> {
    let names: Vec<&str> = entry.get("Names")?.as_array()?.iter().filter_map(Value::as_str).collect();
    // A container linked under other names lists those too (`/other/alias`); its own has one slash.
    let name = names.iter().find(|n| n.matches('/').count() == 1).or(names.first())?.trim_start_matches('/').to_string();
    let public_ports = entry
        .get("Ports")
        .and_then(Value::as_array)
        .map(|ports| ports.iter().filter_map(|p| u16::try_from(p.get("PublicPort")?.as_u64()?).ok()).collect())
        .unwrap_or_default();
    Some(Listed {
        name,
        image_id: text(entry, "ImageID"),
        project: entry.get("Labels").and_then(|l| l.get(PROJECT_LABEL)).and_then(Value::as_str).map(str::to_string),
        public_ports,
    })
}

/// One entry of `docker container inspect`, for when the list has to be built from that.
pub fn listed_from_inspect(entry: &Value) -> Option<Listed> {
    let name = text(entry, "Name").trim_start_matches('/').to_string();
    let mut public_ports = Vec::new();
    if let Some(ports) = entry.get("NetworkSettings").and_then(|n| n.get("Ports")).and_then(Value::as_object) {
        for bindings in ports.values().filter_map(Value::as_array) {
            public_ports.extend(bindings.iter().filter_map(|b| b.get("HostPort")?.as_str()?.parse::<u16>().ok()));
        }
    }
    (!name.is_empty()).then(|| Listed {
        name,
        image_id: text(entry, "Image"),
        project: entry.get("Config").and_then(|c| c.get("Labels")).and_then(|l| l.get(PROJECT_LABEL)).and_then(Value::as_str).map(str::to_string),
        public_ports,
    })
}

/// A container that comes and goes between the listing and the inspection (other tools make
/// short-lived ones) makes Docker report an error for it, so that read is repeated.
async fn containers_by_program(all: bool) -> Result<Vec<Listed>, String> {
    let mut last = String::new();
    for _ in 0..3 {
        let mut args = vec!["container", "ls", "--no-trunc", "-q"];
        if all {
            args.push("-a");
        }
        let listed = probe("docker", &args).await;
        if !listed.success {
            return Err(format!("Cannot list the containers: {}", listed.text));
        }
        let ids: Vec<&str> = listed.text.lines().map(str::trim).filter(|n| !n.is_empty()).collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut args = vec!["container", "inspect"];
        args.extend(ids);
        let out = probe("docker", &args).await;
        if out.success {
            let entries: Value = serde_json::from_str(&out.text).map_err(|_| "Docker gave a list of containers that could not be read.".to_string())?;
            return Ok(entries.as_array().map(|a| a.iter().filter_map(listed_from_inspect).collect()).unwrap_or_default());
        }
        last = out.text.lines().rev().find(|l| l.contains("Error")).unwrap_or(&out.text).to_string();
    }
    Err(format!("Cannot read the containers: {last}"))
}

static ALL_CONTAINERS: Coalesced<Result<Vec<Listed>, String>> = dockerapi::shared();
static RUNNING_CONTAINERS: Coalesced<Result<Vec<Listed>, String>> = dockerapi::shared();

/// Every container (`all`) or only the ones that are up. Asked by several cards at once, so shared.
pub async fn containers(all: bool) -> Result<Vec<Listed>, String> {
    let cache = if all { &ALL_CONTAINERS } else { &RUNNING_CONTAINERS };
    cache
        .get(|| async {
            let path = if all { "/containers/json?all=1" } else { "/containers/json" };
            match dockerapi::get_json(path).await {
                Ok(json) => Ok(json.as_array().map(|a| a.iter().filter_map(listed_from_list).collect()).unwrap_or_default()),
                Err(ApiError::Unsupported) => containers_by_program(all).await,
                Err(other) => Err(format!("Cannot list the containers: {}", describe(other))),
            }
        })
        .await
}

/// Every name an image with a name of this repository has (`repository:tag`).
pub async fn image_tags(repo: &str) -> Result<Vec<String>, String> {
    let filters = json!({ "reference": [repo] }).to_string();
    match dockerapi::get_json(&format!("/images/json?filters={}", dockerapi::encode(&filters, ""))).await {
        Ok(json) => Ok(json
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|i| i.get("RepoTags").and_then(Value::as_array).into_iter().flatten())
            .filter_map(|t| t.as_str().map(str::to_string))
            .collect()),
        Err(ApiError::Unsupported) => {
            let listed = probe("docker", &["image", "ls", repo, "--format", "{{.Repository}}:{{.Tag}}"]).await;
            if listed.success {
                Ok(listed.text.lines().map(|l| l.trim().to_string()).collect())
            } else {
                Err(format!("Cannot list the Docker images: {}", listed.text))
            }
        }
        Err(other) => Err(format!("Cannot list the Docker images: {}", describe(other))),
    }
}

/// The ids of the images that lost their name and carry `label` (`key=value`).
pub async fn unnamed_images(label: &str) -> Result<Vec<String>, String> {
    let filters = json!({ "dangling": ["true"], "label": [label] }).to_string();
    match dockerapi::get_json(&format!("/images/json?filters={}", dockerapi::encode(&filters, ""))).await {
        Ok(json) => Ok(json.as_array().into_iter().flatten().filter_map(|i| i.get("Id")?.as_str().map(str::to_string)).collect()),
        Err(ApiError::Unsupported) => {
            let filter = format!("label={label}");
            let listed = probe("docker", &["image", "ls", "--filter", "dangling=true", "--filter", &filter, "--format", "{{.ID}}"]).await;
            if listed.success {
                Ok(listed.text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
            } else {
                Err(format!("Cannot list the Docker images: {}", listed.text))
            }
        }
        Err(other) => Err(format!("Cannot list the Docker images: {}", describe(other))),
    }
}

// ---------------------------------------------------------------------------------------
// Inside a container, and inside a volume
// ---------------------------------------------------------------------------------------

/// Runs a command in a running container and returns its output the way [`crate::process::capture`]
/// does (stdout, then stderr; success means exit code 0). `None` when the engine is not on a local
/// pipe, so the caller uses `docker exec` instead. `env` reaches the command through the engine.
pub async fn exec(container: &str, env: &[(&str, &str)], cmd: &[String], timeout: Duration) -> Option<Probe> {
    let env: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    match dockerapi::exec(container, &env, cmd, timeout).await {
        Err(ApiError::Unsupported) => None,
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !err.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            Some(Probe { success: out.code == 0, text })
        }
        Err(ApiError::Http { message, .. }) => Some(Probe { success: false, text: message }),
        Err(other) => Some(Probe { success: false, text: describe(other) }),
    }
}

/// One reader at a time: it has a fixed name.
static READER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn remove_reader(name: &str) -> Result<(), ApiError> {
    let response = dockerapi::request("DELETE", &format!("/containers/{}?force=1", dockerapi::encode(name, "")), None, dockerapi::REQUEST_TIMEOUT).await?;
    if response.status == 404 {
        return Ok(());
    }
    response.ok().map(|_| ())
}

async fn read_files(name: &str, image: &str, volume: &str, mount: &str, files: &[&str]) -> Result<Vec<Option<Vec<u8>>>, ApiError> {
    // A reader an interrupted read left behind would keep the name busy.
    remove_reader(name).await?;
    let body = json!({
        "Image": image,
        "Cmd": ["true"],
        "NetworkDisabled": true,
        "Labels": { "io.worldstone.reader": "true" },
        "HostConfig": { "Binds": [format!("{volume}:{mount}:ro")] },
    });
    dockerapi::request("POST", &format!("/containers/create?name={}", dockerapi::encode(name, "")), Some(&body), dockerapi::REQUEST_TIMEOUT).await?.ok()?;
    let mut found = Vec::new();
    let mut failure = None;
    for file in files {
        match dockerapi::archive_file(name, file).await {
            Ok(content) => found.push(content),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    let _ = remove_reader(name).await;
    match failure {
        Some(e) => Err(e),
        None => Ok(found),
    }
}

/// Reads files out of a volume, shown read-only, without running anything: a container is only
/// created (never started) so Docker can show the volume, read from, and removed at once.
/// `Ok(None)` when the engine is not on a local pipe. Each file is `None` when it does not exist.
pub async fn read_volume_files(name: &str, image: &str, volume: &str, mount: &str, files: &[&str]) -> Result<Option<Vec<Option<Vec<u8>>>>, String> {
    let _one_at_a_time = READER.lock().await;
    match read_files(name, image, volume, mount, files).await {
        Ok(found) => Ok(Some(found)),
        Err(ApiError::Unsupported) => Ok(None),
        Err(other) => Err(describe(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_facts_are_read_from_the_info_answer() {
        let info = json!({ "OSType": "linux", "NCPU": 12, "MemTotal": 16299732992u64, "ServerVersion": "29.8.0", "Other": 1 });
        assert_eq!(
            info_from_json(&info).unwrap(),
            EngineInfo { os_type: "linux".into(), cpus: 12, memory: 16299732992, version: "29.8.0".into() }
        );
        assert!(info_from_json(&json!({})).unwrap_err().contains("empty"));
        assert!(info_from_json(&json!({ "OSType": "linux", "NCPU": "many" })).unwrap_err().contains("resources"));
    }

    #[test]
    fn an_image_is_read_with_its_labels() {
        let image = json!({
            "Id": "sha256:abc", "Created": "2026-09-30T00:14:34.5Z", "Size": 1534518042u64,
            "Config": { "Labels": { "io.worldstone.fingerprint": "f00d", "io.worldstone.playerbots": "true" } }
        });
        let found = image_from_json(&image).unwrap();
        assert_eq!((found.id.as_str(), found.created.as_str(), found.size), ("sha256:abc", "2026-09-30T00:14:34.5Z", 1534518042));
        assert_eq!(found.label("io.worldstone.fingerprint"), "f00d");
        assert_eq!(found.label("io.worldstone.commit"), "");
    }

    #[test]
    fn an_image_without_labels_or_without_an_id_is_handled() {
        let bare = image_from_json(&json!({ "Id": "sha256:abc", "Created": "x", "Size": 5, "Config": { "Labels": null } })).unwrap();
        assert!(bare.labels.is_empty());
        assert_eq!(image_from_json(&json!({ "Created": "x", "Size": 5 })), None);
        assert_eq!(image_from_json(&json!({ "Id": "sha256:abc" })), None);
    }

    #[test]
    fn a_container_is_read_with_its_health_and_exit() {
        let up = json!({ "State": { "Status": "running", "ExitCode": 0, "StartedAt": "2026-09-29T23:14:27.69Z", "Health": { "Status": "healthy", "Log": [] } } });
        assert_eq!(
            container_from_json(&up).unwrap(),
            ContainerState { status: "running".into(), health: Some("healthy".into()), exit_code: 0, started_at: "2026-09-29T23:14:27.69Z".into() }
        );
        let no_check = json!({ "State": { "Status": "exited", "ExitCode": 137, "StartedAt": "0001-01-01T00:00:00Z" } });
        let seen = container_from_json(&no_check).unwrap();
        assert_eq!((seen.health, seen.exit_code), (None, 137));
        assert_eq!(container_from_json(&json!({})), None);
    }

    #[test]
    fn the_list_shape_and_the_inspect_shape_give_the_same_container() {
        let from_list = json!({
            "Names": ["/worldstone-azerothcore-authserver"], "ImageID": "sha256:b402",
            "Labels": { "com.docker.compose.project": "worldstone-azerothcore" },
            "Ports": [{ "IP": "127.0.0.1", "PrivatePort": 3724, "PublicPort": 3724, "Type": "tcp" }, { "PrivatePort": 9000, "Type": "tcp" }]
        });
        let from_inspect = json!({
            "Name": "/worldstone-azerothcore-authserver", "Image": "sha256:b402",
            "Config": { "Labels": { "com.docker.compose.project": "worldstone-azerothcore" } },
            "NetworkSettings": { "Ports": { "3724/tcp": [{ "HostIp": "127.0.0.1", "HostPort": "3724" }], "9000/tcp": null } }
        });
        let expected = Listed {
            name: "worldstone-azerothcore-authserver".into(),
            image_id: "sha256:b402".into(),
            project: Some("worldstone-azerothcore".into()),
            public_ports: vec![3724],
        };
        assert_eq!(listed_from_list(&from_list).unwrap(), expected);
        assert_eq!(listed_from_inspect(&from_inspect).unwrap(), expected);
    }

    #[test]
    fn a_linked_container_is_listed_under_its_own_name() {
        let entry = json!({ "Names": ["/web/db", "/db"], "ImageID": "sha256:1", "Labels": {}, "Ports": [] });
        let listed = listed_from_list(&entry).unwrap();
        assert_eq!((listed.name.as_str(), listed.project), ("db", None));
        assert_eq!(listed_from_list(&json!({ "ImageID": "x" })), None);
    }
}
