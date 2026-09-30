//! The status dashboard (Step 9): for each of the instance's containers its state, port, how long
//! this run has been going, and the processor and memory it uses right now, all read from Docker.
//!
//! Nothing here starts, stops or changes anything, and nothing is guessed: a container that is not
//! running has no usage, and a number Docker did not give is shown as unknown.

use crate::{
    builder, clock, compose,
    process::{capture, CommandSpec},
    recipe::Recipe,
    runtime::{observe_run, ServiceState},
};
use serde::Serialize;
use std::{collections::BTreeMap, time::Duration};

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DashboardRow {
    pub id: String,
    pub label: String,
    pub container: String,
    pub state: ServiceState,
    pub port: u16,
    /// Whether the port is published on this machine. The database is reachable only by the servers.
    pub published: bool,
    /// How long this run has been going, for a container that is up.
    pub uptime_secs: Option<u64>,
    /// Share of one processor core, in percent (a busy container can pass 100).
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    /// The most memory Docker lets the container use.
    pub memory_limit_bytes: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub host: String,
    /// Set when Docker could not be asked, or could not give the usage.
    pub note: Option<String>,
    pub rows: Vec<DashboardRow>,
}

/// What Docker says one container uses.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Usage {
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub memory_limit_bytes: Option<u64>,
}

/// `3.25%` as `3.25`; Docker prints `--` when it has no figure.
pub fn parse_percent(text: &str) -> Option<f64> {
    let value: f64 = text.trim().strip_suffix('%')?.trim().parse().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

/// `512.3MiB` as bytes. Docker uses binary units for memory (`KiB`, `MiB`, `GiB`) and decimal ones
/// in other columns (`kB`, `MB`), so both are read.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (number, unit) = text.split_at(split);
    let number: f64 = number.parse().ok()?;
    let factor = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = (number * factor).round();
    (bytes.is_finite() && bytes >= 0.0 && bytes < u64::MAX as f64).then_some(bytes as u64)
}

/// One line of `docker stats --no-stream --format "{{.Name}}|{{.CPUPerc}}|{{.MemUsage}}"`, such as
/// `worldstone-mangos-zero-mangosd|3.25%|512.3MiB / 7.6GiB`.
pub fn parse_stats_line(line: &str) -> Option<(String, Usage)> {
    let mut parts = line.trim().split('|');
    let (name, cpu, memory) = (parts.next()?.trim(), parts.next()?, parts.next()?);
    if name.is_empty() || parts.next().is_some() {
        return None;
    }
    let (used, limit) = memory.split_once('/').unwrap_or((memory, ""));
    Some((name.to_string(), Usage { cpu_percent: parse_percent(cpu), memory_bytes: parse_size(used), memory_limit_bytes: parse_size(limit) }))
}

/// The usage of the named containers, which must be running. `Err` says why Docker gave none.
async fn usage(containers: &[String]) -> Result<BTreeMap<String, Usage>, String> {
    if containers.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut args = vec!["stats", "--no-stream", "--format", "{{.Name}}|{{.CPUPerc}}|{{.MemUsage}}"];
    args.extend(containers.iter().map(String::as_str));
    let out = capture(&CommandSpec::new("docker", &args).timeout(Duration::from_secs(30))).await;
    if !out.success {
        return Err(format!("Docker could not report processor and memory use: {}", out.text.lines().next().unwrap_or("no answer").trim()));
    }
    Ok(out.text.lines().filter_map(parse_stats_line).collect())
}

fn is_up(state: ServiceState) -> bool {
    matches!(state, ServiceState::Running | ServiceState::Starting | ServiceState::Unresponsive | ServiceState::Paused)
}

pub async fn status(instance: &str, recipe: &Recipe) -> Dashboard {
    let host = compose::HOST_ADDRESS.to_string();
    if let Err(reason) = builder::engine().await {
        return Dashboard { host, note: Some(reason), rows: Vec::new() };
    }
    let mut targets: Vec<(String, String, String, u16, bool)> = vec![(
        compose::DATABASE_SERVICE.to_string(),
        "Database".to_string(),
        compose::database_container(instance),
        compose::DATABASE_PORT,
        false,
    )];
    targets.extend(recipe.run.services.iter().map(|s| (s.id.clone(), s.label.clone(), compose::service_container(instance, &s.id), s.port, true)));

    let mut rows = Vec::new();
    for (id, label, container, port, published) in targets {
        let (seen, began) = observe_run(&container).await;
        let uptime_secs = began.filter(|_| is_up(seen.state)).map(|b| clock::now().saturating_sub(b));
        rows.push(DashboardRow {
            id,
            label,
            container,
            state: seen.state,
            port,
            published,
            uptime_secs,
            cpu_percent: None,
            memory_bytes: None,
            memory_limit_bytes: None,
        });
    }

    let running: Vec<String> = rows.iter().filter(|r| is_up(r.state)).map(|r| r.container.clone()).collect();
    let note = match usage(&running).await {
        Ok(found) => {
            for row in &mut rows {
                if let Some(u) = found.get(&row.container) {
                    row.cpu_percent = u.cpu_percent;
                    row.memory_bytes = u.memory_bytes;
                    row.memory_limit_bytes = u.memory_limit_bytes;
                }
            }
            None
        }
        Err(why) => Some(why),
    };
    Dashboard { host, note, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_are_read_and_a_missing_figure_is_unknown() {
        assert_eq!(parse_percent("3.25%"), Some(3.25));
        assert_eq!(parse_percent(" 0.00% "), Some(0.0));
        assert_eq!(parse_percent("152.8%"), Some(152.8));
        for bad in ["--", "", "%", "abc%", "-3%", "3.25", "NaN%"] {
            assert_eq!(parse_percent(bad), None, "{bad}");
        }
    }

    #[test]
    fn sizes_are_read_in_binary_and_decimal_units() {
        assert_eq!(parse_size("512B"), Some(512));
        assert_eq!(parse_size("1KiB"), Some(1024));
        assert_eq!(parse_size("1.5MiB"), Some(1_572_864));
        assert_eq!(parse_size("7.6GiB"), Some(8_160_437_862));
        assert_eq!(parse_size("2kB"), Some(2000));
        assert_eq!(parse_size("3MB"), Some(3_000_000));
        assert_eq!(parse_size(" 12 MiB "), Some(12 * 1024 * 1024));
    }

    #[test]
    fn a_size_that_is_not_one_is_unknown() {
        for bad in ["", "--", "MiB", "12", "12 parsecs", "-1MiB", "1.2.3MiB"] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_stats_line_gives_the_container_and_what_it_uses() {
        let (name, usage) = parse_stats_line("worldstone-mangos-zero-mangosd|3.25%|512.5MiB / 7.6GiB\n").unwrap();
        assert_eq!(name, "worldstone-mangos-zero-mangosd");
        assert_eq!(usage.cpu_percent, Some(3.25));
        assert_eq!(usage.memory_bytes, Some(537_395_200));
        assert_eq!(usage.memory_limit_bytes, Some(8_160_437_862));
    }

    #[test]
    fn a_stats_line_with_no_figures_keeps_the_container_with_unknowns() {
        let (name, usage) = parse_stats_line("worldstone-mangos-zero-db|--|-- / --").unwrap();
        assert_eq!(name, "worldstone-mangos-zero-db");
        assert_eq!(usage, Usage { cpu_percent: None, memory_bytes: None, memory_limit_bytes: None });
    }

    #[test]
    fn lines_that_are_not_stats_lines_are_ignored() {
        for bad in ["", "no separators", "a|b", "|1%|1MiB / 2MiB", "a|1%|1MiB / 2MiB|extra"] {
            assert_eq!(parse_stats_line(bad), None, "{bad}");
        }
    }

    #[test]
    fn only_a_container_that_exists_and_runs_has_uptime_and_usage() {
        assert!(is_up(ServiceState::Running) && is_up(ServiceState::Starting) && is_up(ServiceState::Paused) && is_up(ServiceState::Unresponsive));
        assert!(!is_up(ServiceState::Stopped) && !is_up(ServiceState::Missing) && !is_up(ServiceState::Crashed));
    }
}
