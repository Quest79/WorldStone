//! The fonts a person can pick: those installed on this Windows, and Google Fonts.
//!
//! *Installed* fonts are read from Windows itself (DirectWrite, through PowerShell), so the list is
//! what this machine really has. *Google* fonts come from the public Google Fonts service: the list
//! (with each family's weights) is fetched once and kept, and a family is downloaded only when it is
//! chosen, into `<root>/Fonts/google/<family>/`. After that it loads from disk, so it works offline.
//! Downloads go through `curl.exe`, which ships with Windows 10 and later. A name that reaches a URL
//! or a path is validated first, and a downloaded file has to come from Google's file host.

use crate::{
    clock,
    paths::{valid_component, Paths},
    process::{capture, CommandSpec},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::Mutex, time::Duration};

const CATALOGUE_URL: &str = "https://fonts.google.com/metadata/fonts";
const CSS_URL: &str = "https://fonts.googleapis.com/css2";
/// Every font file Google's stylesheet names is on this host; anything else is refused.
const FILES_HOST: &str = "https://fonts.gstatic.com/";
const CATALOGUE_FILE: &str = "google-catalogue.json";
const FAMILY_FILE: &str = "font.json";

/// What Windows calls a font family, as it may appear in a CSS `font-family: "…"`.
pub fn valid_installed_name(name: &str) -> bool {
    (1..=100).contains(&name.chars().count()) && name == name.trim() && !name.chars().any(|c| c.is_control() || "\"\\;{}<>".contains(c))
}

/// A Google Fonts family name: letters, digits, spaces and hyphens. It reaches a URL and a folder name.
pub fn valid_google_family(name: &str) -> bool {
    (1..=60).contains(&name.len())
        && name == name.trim()
        && !name.contains("  ")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-')
}

fn slug(family: &str) -> String {
    family.to_ascii_lowercase().replace(' ', "-")
}

// ---------------------------------------------------------------------------------------------
// Installed fonts

/// Windows lists these composite fonts, but they are not real families a page can use.
const NOT_FAMILIES: [&str; 4] = ["Global Monospace", "Global Sans Serif", "Global Serif", "Global User Interface"];

static INSTALLED: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// The font families installed on this machine, sorted. Read once per run (it takes about a second)
/// and again when `refresh` is set.
pub async fn installed(refresh: bool) -> Result<Vec<String>, String> {
    if !refresh {
        if let Some(list) = INSTALLED.lock().ok().and_then(|held| held.clone()) {
            return Ok(list);
        }
    }
    let list = read_installed().await?;
    if let Ok(mut held) = INSTALLED.lock() {
        *held = Some(list.clone());
    }
    Ok(list)
}

const LIST_SCRIPT: &str = "$ProgressPreference = 'SilentlyContinue'\r\n\
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)\r\n\
Add-Type -AssemblyName PresentationCore\r\n\
$en = [System.Windows.Markup.XmlLanguage]::GetLanguage('en-us')\r\n\
foreach ($f in [System.Windows.Media.Fonts]::SystemFontFamilies) { $n = $f.FamilyNames[$en]; if (-not $n) { $n = $f.Source }; $n }";

async fn read_installed() -> Result<Vec<String>, String> {
    if !cfg!(windows) {
        return Err("Listing the installed fonts is only supported on Windows.".into());
    }
    let script: Vec<u8> = LIST_SCRIPT.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64(&script);
    let spec = CommandSpec::new("powershell.exe", &["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded]).timeout(Duration::from_secs(45));
    let probe = capture(&spec).await;
    if !probe.success {
        return Err(format!("Cannot list the installed fonts: {}", probe.text));
    }
    let list = parse_installed(&probe.text);
    if list.is_empty() {
        return Err("Windows reported no installed fonts.".into());
    }
    Ok(list)
}

/// One family per line; PowerShell's own progress or error markup, composite pseudo-families and
/// names that could not be used in CSS are dropped.
pub fn parse_installed(text: &str) -> Vec<String> {
    let mut names: Vec<String> = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && !l.starts_with('<') && !NOT_FAMILIES.contains(l) && valid_installed_name(l))
        .map(str::to_string)
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

// ---------------------------------------------------------------------------------------------
// Google Fonts: the list

/// One family in the Google Fonts list.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GoogleFamily {
    pub family: String,
    pub category: String,
    /// Upright weights the family has (400 is regular, 700 bold).
    pub weights: Vec<u16>,
}

/// The Google Fonts list, most popular first, and when it was fetched.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Catalogue {
    pub fetched_at: u64,
    pub fonts: Vec<GoogleFamily>,
    /// Why the list shown is not a fresh one, when a refresh failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Deserialize)]
struct RawCatalogue {
    #[serde(rename = "familyMetadataList")]
    list: Vec<RawFamily>,
}

#[derive(Deserialize)]
struct RawFamily {
    family: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    popularity: u32,
    #[serde(default)]
    fonts: BTreeMap<String, serde::de::IgnoredAny>,
}

/// Reads Google's font metadata down to what the picker needs, ordered by popularity.
pub fn parse_catalogue(text: &str) -> Result<Vec<GoogleFamily>, String> {
    let raw: RawCatalogue = serde_json::from_str(text).map_err(|e| format!("Google's font list has an unexpected shape ({e})."))?;
    let mut list: Vec<(u32, GoogleFamily)> = raw
        .list
        .into_iter()
        .filter(|f| valid_google_family(&f.family))
        .map(|f| {
            // Keys are "400" for upright and "400i" for italic.
            let mut weights: Vec<u16> = f.fonts.keys().filter_map(|k| k.parse().ok()).collect();
            weights.sort_unstable();
            if weights.is_empty() {
                weights.push(400);
            }
            (f.popularity, GoogleFamily { family: f.family, category: f.category, weights })
        })
        .collect();
    list.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.family.cmp(&b.1.family)));
    if list.is_empty() {
        return Err("Google's font list was empty.".into());
    }
    Ok(list.into_iter().map(|(_, f)| f).collect())
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| format!("Cannot create {}: {e}", folder.display()))?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text).map_err(|e| format!("Cannot write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, path).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

fn read_cached_catalogue(paths: &Paths) -> Option<Catalogue> {
    let text = std::fs::read_to_string(paths.fonts().join(CATALOGUE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The list of Google fonts: the saved one, or a fresh one when there is none or `refresh` is set.
/// If a refresh fails and a saved list exists, that list is returned with a note saying why.
pub async fn catalogue(paths: &Paths, refresh: bool) -> Result<Catalogue, String> {
    let cached = read_cached_catalogue(paths);
    if let (Some(saved), false) = (&cached, refresh) {
        return Ok(saved.clone());
    }
    match fetch_catalogue(paths).await {
        Ok(fonts) => {
            let fresh = Catalogue { fetched_at: clock::now(), fonts, note: None };
            let text = serde_json::to_string(&fresh).map_err(|e| e.to_string())?;
            write_atomic(&paths.fonts().join(CATALOGUE_FILE), &text)?;
            Ok(fresh)
        }
        Err(why) => match cached {
            Some(mut saved) => {
                saved.note = Some(format!("The list could not be refreshed ({why}); this is the saved one."));
                Ok(saved)
            }
            None => Err(why),
        },
    }
}

async fn fetch_catalogue(paths: &Paths) -> Result<Vec<GoogleFamily>, String> {
    let folder = paths.fonts();
    std::fs::create_dir_all(&folder).map_err(|e| format!("Cannot create {}: {e}", folder.display()))?;
    let target = folder.join("google-catalogue.download");
    let got = fetch(CATALOGUE_URL, Some(&target), 60).await;
    let parsed = match got {
        Ok(_) => std::fs::read_to_string(&target).map_err(|e| format!("Cannot read the downloaded list: {e}")).and_then(|t| parse_catalogue(&t)),
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&target);
    parsed
}

// ---------------------------------------------------------------------------------------------
// Google Fonts: downloading a family

/// Runs `curl.exe` for one https address. With `into` the body goes to that file, else it is returned.
async fn fetch(url: &str, into: Option<&Path>, seconds: u64) -> Result<String, String> {
    let program = if cfg!(windows) { "curl.exe" } else { "curl" };
    let seconds_text = seconds.to_string();
    let mut args = vec!["-fsSL", "--proto", "=https", "--proto-redir", "=https", "--max-time", &seconds_text];
    let target = into.map(|p| p.to_string_lossy().into_owned());
    if let Some(target) = &target {
        args.extend(["-o", target.as_str()]);
    }
    args.push(url);
    let probe = capture(&CommandSpec::new(program, &args).timeout(Duration::from_secs(seconds + 10))).await;
    if probe.success {
        Ok(probe.text)
    } else {
        Err(explain_fetch_failure(&probe.text))
    }
}

fn explain_fetch_failure(text: &str) -> String {
    let lower = text.to_lowercase();
    if lower.contains("error: 400") || lower.contains("error: 404") {
        "Google Fonts does not have that family.".into()
    } else if lower.contains("could not resolve") || lower.contains("failed to connect") || lower.contains("timed out") || lower.contains("did not respond") {
        "Cannot reach Google Fonts. Check the internet connection.".into()
    } else if lower.contains("cannot start") {
        format!("Downloading needs curl, which Windows 10 and later include. ({text})")
    } else {
        format!("Google Fonts could not be reached: {text}")
    }
}

/// The two weights worth downloading: the ones nearest regular (400) and bold (700).
pub fn pick_weights(available: &[u16]) -> Vec<u16> {
    let nearest = |target: u16| available.iter().copied().min_by_key(|w| (w.abs_diff(target), u16::MAX - w));
    let mut picked: Vec<u16> = [nearest(400), nearest(700)].into_iter().flatten().collect();
    picked.sort_unstable();
    picked.dedup();
    picked
}

/// One `@font-face` from Google's stylesheet.
#[derive(Debug, PartialEq)]
pub struct CssFace {
    pub family: String,
    pub weight: u16,
    pub url: String,
    pub extension: String,
}

/// The upright faces a Google stylesheet names. A file that is not on Google's file host, or whose
/// type is not a font, is left out.
pub fn parse_css(css: &str) -> Vec<CssFace> {
    let mut faces: Vec<CssFace> = Vec::new();
    for block in css.split("@font-face").skip(1) {
        let Some(body) = block.split_once('{').and_then(|(_, rest)| rest.split_once('}')).map(|(body, _)| body) else { continue };
        let property = |name: &str| {
            body.split(';').find_map(|part| {
                let (key, value) = part.split_once(':')?;
                (key.trim() == name).then(|| value.trim().to_string())
            })
        };
        if property("font-style").as_deref().is_some_and(|s| s != "normal") {
            continue;
        }
        let family = property("font-family").map(|f| f.trim_matches(['\'', '"']).to_string()).unwrap_or_default();
        let weight = property("font-weight").and_then(|w| w.split_whitespace().next().and_then(|n| n.parse::<u16>().ok()));
        let url = property("src").and_then(|src| {
            let start = src.find("url(")? + 4;
            let end = src[start..].find(')')? + start;
            Some(src[start..end].trim_matches(['\'', '"']).to_string())
        });
        let (Some(weight), Some(url)) = (weight, url) else { continue };
        let path = url.split(['?', '#']).next().unwrap_or("");
        let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let font_file = ["ttf", "otf", "woff", "woff2"].contains(&extension.as_str());
        if !url.starts_with(FILES_HOST) || url.contains(char::is_whitespace) || !font_file || !valid_google_family(&family) {
            continue;
        }
        if !faces.iter().any(|f| f.weight == weight) {
            faces.push(CssFace { family, weight, url, extension });
        }
    }
    faces
}

#[derive(Serialize, Deserialize, Debug)]
struct StoredFace {
    weight: u16,
    file: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Stored {
    family: String,
    faces: Vec<StoredFace>,
}

/// One downloaded face, ready for the page to load: the font file, base64 encoded.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FaceData {
    pub weight: u16,
    pub data: String,
}

static DOWNLOADING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Downloads a Google family into the app's own folder and returns its exact name. It is staged
/// in a scratch folder and moved into place only when every file arrived, so a failed or
/// cancelled download never damages one that already works.
pub async fn download(paths: &Paths, family: &str) -> Result<String, String> {
    if !valid_google_family(family) {
        return Err(format!("'{family}' is not a Google Fonts family name."));
    }
    let _one_at_a_time = DOWNLOADING.lock().await;
    // The saved list knows the family's exact spelling and its weights.
    let known = read_cached_catalogue(paths).and_then(|c| c.fonts.into_iter().find(|f| f.family.eq_ignore_ascii_case(family)));
    let name = known.as_ref().map_or(family, |f| f.family.as_str());
    let axes: Vec<String> = match &known {
        Some(f) => vec![format!(":wght@{}", pick_weights(&f.weights).iter().map(u16::to_string).collect::<Vec<_>>().join(";")), String::new()],
        None => vec![":wght@400;700".into(), String::new()],
    };
    let mut failure = String::new();
    let mut faces = Vec::new();
    for axis in axes {
        let url = format!("{CSS_URL}?family={}{axis}", name.replace(' ', "+"));
        match fetch(&url, None, 30).await {
            Ok(css) => {
                faces = parse_css(&css);
                if !faces.is_empty() {
                    break;
                }
                failure = "Google Fonts answered, but named no font files.".into();
            }
            Err(e) => failure = e,
        }
    }
    let Some(first) = faces.first() else { return Err(failure) };
    let exact = first.family.clone();

    let root = paths.fonts().join("google");
    let staged = root.join(format!("{}.partial", slug(&exact)));
    let placed = root.join(slug(&exact));
    if staged.exists() {
        std::fs::remove_dir_all(&staged).map_err(|e| format!("Cannot clear {}: {e}", staged.display()))?;
    }
    std::fs::create_dir_all(&staged).map_err(|e| format!("Cannot create {}: {e}", staged.display()))?;
    let result = stage_faces(&staged, &exact, &faces).await;
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(e);
    }
    if placed.exists() {
        std::fs::remove_dir_all(&placed).map_err(|e| format!("Cannot replace {}: {e}", placed.display()))?;
    }
    std::fs::rename(&staged, &placed).map_err(|e| format!("Cannot place {}: {e}", placed.display()))?;
    Ok(exact)
}

async fn stage_faces(folder: &Path, family: &str, faces: &[CssFace]) -> Result<(), String> {
    let mut stored = Vec::new();
    for face in faces {
        let file = format!("{}.{}", face.weight, face.extension);
        let target = folder.join(&file);
        fetch(&face.url, Some(&target), 60).await?;
        let head = std::fs::read(&target).map_err(|e| format!("Cannot read the downloaded {file}: {e}"))?;
        if !looks_like_a_font(&head) {
            return Err(format!("What Google Fonts sent for weight {} is not a font file.", face.weight));
        }
        stored.push(StoredFace { weight: face.weight, file });
    }
    let text = serde_json::to_string_pretty(&Stored { family: family.to_string(), faces: stored }).map_err(|e| e.to_string())?;
    write_atomic(&folder.join(FAMILY_FILE), &text)
}

/// TrueType, OpenType, WOFF or WOFF2 by their first four bytes.
fn looks_like_a_font(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some([0, 1, 0, 0] | b"true" | b"OTTO" | b"wOFF" | b"wOF2"))
}

fn read_stored(paths: &Paths, family: &str) -> Result<(std::path::PathBuf, Stored), String> {
    if !valid_google_family(family) {
        return Err(format!("'{family}' is not a Google Fonts family name."));
    }
    let folder = paths.fonts().join("google").join(slug(family));
    let text = std::fs::read_to_string(folder.join(FAMILY_FILE)).map_err(|_| format!("{family} has not been downloaded yet."))?;
    let stored: Stored = serde_json::from_str(&text).map_err(|e| format!("The saved files for {family} cannot be read ({e})."))?;
    Ok((folder, stored))
}

/// The Google families downloaded so far, sorted.
pub fn downloaded(paths: &Paths) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(paths.fonts().join("google")) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().ends_with(".partial"))
        .filter_map(|e| serde_json::from_str::<Stored>(&std::fs::read_to_string(e.path().join(FAMILY_FILE)).ok()?).ok())
        .map(|s| s.family)
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// The files of a downloaded family, for the page to register as fonts.
pub fn faces(paths: &Paths, family: &str) -> Result<Vec<FaceData>, String> {
    let (folder, stored) = read_stored(paths, family)?;
    stored
        .faces
        .iter()
        .filter(|f| valid_component(&f.file))
        .map(|f| {
            let bytes = std::fs::read(folder.join(&f.file)).map_err(|e| format!("Cannot read the saved {} file of {family}: {e}", f.weight))?;
            Ok(FaceData { weight: f.weight, data: base64(&bytes) })
        })
        .collect()
}

pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSS: &str = "@font-face {\n  font-family: 'Cinzel';\n  font-style: normal;\n  font-weight: 400;\n  src: url(https://fonts.gstatic.com/s/cinzel/v26/a.ttf) format('truetype');\n}\n@font-face {\n  font-family: 'Cinzel';\n  font-style: normal;\n  font-weight: 700;\n  src: url(https://fonts.gstatic.com/s/cinzel/v26/b.ttf) format('truetype');\n}\n";

    #[test]
    fn base64_matches_the_standard_examples() {
        for (plain, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[test]
    fn family_names_that_could_break_a_url_a_path_or_css_are_refused() {
        for ok in ["Cinzel", "Pirata One", "Noto Sans JP", "Rock 3D", "Baloo 2", "Kumar-One"] {
            assert!(valid_google_family(ok), "{ok}");
        }
        for bad in ["", " Cinzel", "Cinzel ", "Cin  zel", "../x", "a/b", "a&b=c", "a:wght@1", "a'b", "Zürich", &"x".repeat(61)] {
            assert!(!valid_google_family(bad), "{bad}");
        }
        for ok in ["Segoe UI", "Yu Gothic UI", "MS PGothic", "Microsoft YaHei UI", "Bookman Old Style", "Arial Nova (Light)"] {
            assert!(valid_installed_name(ok), "{ok}");
        }
        for bad in ["", "a\"b", "a\\b", "a;b", "a{b}", "<b>", "a\nb", " lead"] {
            assert!(!valid_installed_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_installed_list_drops_markup_pseudo_families_and_duplicates() {
        let text = "\u{feff}Segoe UI\r\nArial\r\n#< CLIXML\r\n<Objs Version=\"1.1\">\r\nGlobal Serif\r\nGlobal User Interface\r\ncascadia code\r\nCascadia Code\r\nBad\"name\r\n\r\nCalibri\r\n";
        assert_eq!(parse_installed(text), ["Arial", "Calibri", "cascadia code", "Segoe UI"]);
    }

    #[test]
    fn google_s_font_list_is_read_down_to_family_category_and_upright_weights_by_popularity() {
        let json = r#"{ "axisRegistry": [], "promotedScript": "", "familyMetadataList": [
            { "family": "Rare One", "category": "Display", "popularity": 900, "fonts": { "400": {} } },
            { "family": "Roboto", "category": "Sans Serif", "popularity": 1, "fonts": { "400": {}, "400i": {}, "700": {}, "100": {} } },
            { "family": "Italic Only", "category": "Serif", "popularity": 50, "fonts": { "400i": {} } },
            { "family": "Bad/Name", "category": "Serif", "popularity": 2, "fonts": { "400": {} } }
        ] }"#;
        let list = parse_catalogue(json).unwrap();
        let names: Vec<&str> = list.iter().map(|f| f.family.as_str()).collect();
        assert_eq!(names, ["Roboto", "Italic Only", "Rare One"]);
        assert_eq!(list[0], GoogleFamily { family: "Roboto".into(), category: "Sans Serif".into(), weights: vec![100, 400, 700] });
        assert_eq!(list[1].weights, [400], "a family with no upright weight still downloads its regular");
        assert!(parse_catalogue("{}").is_err() && parse_catalogue("<html>").is_err());
    }

    #[test]
    fn the_weights_downloaded_are_the_ones_nearest_regular_and_bold() {
        assert_eq!(pick_weights(&[100, 200, 300, 400, 500, 600, 700, 800, 900]), [400, 700]);
        assert_eq!(pick_weights(&[400]), [400]);
        assert_eq!(pick_weights(&[700]), [700]);
        assert_eq!(pick_weights(&[300, 400, 500]), [400, 500]);
        assert_eq!(pick_weights(&[100, 300]), [300]);
        assert_eq!(pick_weights(&[500, 600]), [500, 600], "between regular and bold: the nearest of each");
        assert!(pick_weights(&[]).is_empty());
    }

    #[test]
    fn a_stylesheet_gives_its_upright_faces_and_only_files_from_googles_host() {
        let faces = parse_css(CSS);
        assert_eq!(faces.len(), 2);
        assert_eq!((faces[0].family.as_str(), faces[0].weight, faces[0].extension.as_str()), ("Cinzel", 400, "ttf"));
        assert_eq!(faces[1].url, "https://fonts.gstatic.com/s/cinzel/v26/b.ttf");

        let hostile = "@font-face { font-family: 'X'; font-weight: 400; src: url(https://evil.example/x.ttf); }\n\
            @font-face { font-family: 'X'; font-weight: 500; src: url(file:///C:/Windows/win.ini); }\n\
            @font-face { font-family: 'X'; font-weight: 600; src: url(https://fonts.gstatic.com/s/x/a.html); }\n\
            @font-face { font-family: 'X'; font-style: italic; font-weight: 700; src: url(https://fonts.gstatic.com/s/x/i.ttf); }\n\
            @font-face { font-family: '../X'; font-weight: 800; src: url(https://fonts.gstatic.com/s/x/y.ttf); }\n\
            @font-face { font-family: 'X'; font-weight: 100 900; src: url(https://fonts.gstatic.com/s/x/v.woff2?x=1); }";
        let kept = parse_css(hostile);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!((kept[0].weight, kept[0].extension.as_str()), (100, "woff2"), "a variable range starts at its first number");
        assert!(parse_css("<!DOCTYPE html><title>400: Font family not found</title>").is_empty());
    }

    #[test]
    fn only_real_font_files_are_accepted() {
        assert!(looks_like_a_font(&[0, 1, 0, 0, 9]) && looks_like_a_font(b"OTTO....") && looks_like_a_font(b"wOF2....") && looks_like_a_font(b"wOFF..."));
        assert!(!looks_like_a_font(b"<!DO") && !looks_like_a_font(b"") && !looks_like_a_font(&[0, 1]));
    }

    #[test]
    fn a_downloaded_family_is_listed_and_its_files_come_back_encoded() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().to_path_buf());
        assert!(downloaded(&paths).is_empty());
        assert!(faces(&paths, "Cinzel").unwrap_err().contains("not been downloaded"));
        let folder = paths.fonts().join("google").join("cinzel");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("400.ttf"), [0u8, 1, 0, 0, 65]).unwrap();
        std::fs::write(folder.join(FAMILY_FILE), r#"{ "family": "Cinzel", "faces": [ { "weight": 400, "file": "400.ttf" }, { "weight": 700, "file": "../../escape.ttf" } ] }"#).unwrap();
        std::fs::create_dir_all(paths.fonts().join("google").join("half.partial")).unwrap();
        assert_eq!(downloaded(&paths), ["Cinzel"]);
        let got = faces(&paths, "Cinzel").unwrap();
        assert_eq!(got.len(), 1, "a file name that leaves the folder is never read");
        assert_eq!((got[0].weight, got[0].data.as_str()), (400, "AAEAAEE="));
        assert!(faces(&paths, "../x").is_err());
    }

    #[tokio::test]
    async fn a_saved_list_is_used_without_going_online() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().to_path_buf());
        let saved = Catalogue { fetched_at: 5, fonts: vec![GoogleFamily { family: "Cinzel".into(), category: "Serif".into(), weights: vec![400, 700] }], note: None };
        write_atomic(&paths.fonts().join(CATALOGUE_FILE), &serde_json::to_string(&saved).unwrap()).unwrap();
        let got = catalogue(&paths, false).await.unwrap();
        assert_eq!((got.fetched_at, got.fonts.len(), got.note), (5, 1, None));
    }

    #[test]
    fn a_failed_download_says_what_went_wrong_in_words() {
        assert_eq!(explain_fetch_failure("curl: (22) The requested URL returned error: 400"), "Google Fonts does not have that family.");
        assert!(explain_fetch_failure("curl: (6) Could not resolve host: fonts.googleapis.com").contains("internet connection"));
        assert!(explain_fetch_failure("curl.exe did not respond").contains("internet connection"));
        assert!(explain_fetch_failure("Cannot start curl.exe: not found").contains("curl"));
    }

    /// Reads the real machine's font list; run by hand with `--ignored` on Windows.
    #[tokio::test]
    #[ignore]
    async fn windows_lists_its_installed_fonts() {
        let list = installed(true).await.unwrap();
        assert!(list.iter().any(|n| n == "Segoe UI"), "{list:?}");
    }
}
