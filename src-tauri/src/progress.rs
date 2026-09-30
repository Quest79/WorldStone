//! Reads real progress out of tool output. Nothing is estimated: a percentage exists only
//! when the tool printed one, otherwise the UI shows indeterminate progress.

use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub label: String,
    pub percent: f32,
}

/// Recognises git's `Receiving objects:  45% (450/1000), 1.2 MiB | 2.3 MiB/s` family,
/// with or without the `remote: ` prefix.
pub fn parse(line: &str) -> Option<Progress> {
    let line = line.trim();
    let percent_at = line.find('%')?;
    let digits_start = line[..percent_at]
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |i| i + 1);
    let percent: f32 = line[digits_start..percent_at].parse().ok()?;
    // Git prints `<label>: <n>% (done/total)…`: a colon before the percentage, and either
    // nothing or a parenthesised count after it. Other output containing a `%` is not progress.
    let after = line[percent_at + 1..].trim_start();
    if !(after.is_empty() || after.starts_with('(')) {
        return None;
    }
    let before = line[..digits_start].trim_end();
    let label = before.strip_suffix(':')?.trim();
    let label = label.strip_prefix("remote:").unwrap_or(label).trim();
    let plausible = !label.is_empty()
        && label.starts_with(|c: char| c.is_ascii_alphabetic())
        && label.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
        && (0.0..=100.0).contains(&percent);
    plausible.then(|| Progress {
        label: label.to_string(),
        percent,
    })
}

/// Recognises the `[ 45%] Building CXX object …` lines CMake's Makefile generator prints
/// (the percentage is make's own count of finished targets), including when BuildKit prefixes
/// them with `#12 45.6 `.
pub fn parse_build(line: &str) -> Option<Progress> {
    let mut rest = line.trim();
    if let Some(after_hash) = rest.strip_prefix('#') {
        let (step, tail) = after_hash.split_once(' ')?;
        let (seconds, tail) = tail.trim_start().split_once(' ')?;
        let numeric = |s: &str, extra: char| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == extra);
        if !numeric(step, '\0') || !numeric(seconds, '.') {
            return None;
        }
        rest = tail.trim_start();
    }
    let (percent, action) = rest.strip_prefix('[')?.split_once("%]")?;
    let percent: f32 = percent.trim().parse().ok()?;
    let has_action = action.trim().starts_with(|c: char| c.is_ascii_alphabetic());
    ((0.0..=100.0).contains(&percent) && has_action).then(|| Progress {
        label: "Building".to_string(),
        percent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(label: &str, percent: f32) -> Option<Progress> {
        Some(Progress { label: label.into(), percent })
    }

    #[test]
    fn parses_cmake_build_progress_with_or_without_the_buildkit_prefix() {
        assert_eq!(parse_build("[ 45%] Building CXX object src/game/CMakeFiles/game.dir/Player.cpp.o"), p("Building", 45.0));
        assert_eq!(parse_build("[100%] Built target mangosd"), p("Building", 100.0));
        assert_eq!(parse_build("#14 312.7 [  3%] Building CXX object dep/x.cpp.o"), p("Building", 3.0));
    }

    #[test]
    fn build_progress_ignores_lines_that_only_look_similar() {
        for line in [
            "#14 312.7 apt-get install libssl-dev",
            "#14 DONE 12.0s",
            "[ 45%]",
            "[abc%] Building",
            "[150%] Building",
            "Building CXX object 12% src/game",
            "[Player] 12% done",
            "#x 1.0 [ 5%] Building",
            "",
        ] {
            assert_eq!(parse_build(line), None, "{line:?} is not build progress");
        }
    }

    #[test]
    fn parses_git_transfer_progress() {
        assert_eq!(
            parse("Receiving objects:  45% (450/1000), 1.20 MiB | 2.30 MiB/s"),
            p("Receiving objects", 45.0)
        );
        assert_eq!(parse("Resolving deltas: 100% (60/60), done."), p("Resolving deltas", 100.0));
        assert_eq!(parse("remote: Compressing objects:  12% (3/25)"), p("Compressing objects", 12.0));
    }

    #[test]
    fn ignores_lines_that_merely_contain_a_percent_sign() {
        for line in [
            "Cloning into 'mangoszero'...",
            "50% of the objects were skipped",
            "Building CXX object 12% src/game",
            "error: 100% failure",
            "",
            "%",
            "  42%",
        ] {
            assert_eq!(parse(line), None, "{line:?} is not progress");
        }
    }
}
