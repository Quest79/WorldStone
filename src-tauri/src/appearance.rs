//! How the app looks: the fonts of the interface, each with an optional outline and shadow.
//!
//! These belong to the person, not to a server, so they live in `<root>/Settings/appearance.json`
//! beside the launcher's own files. Every font role is the same shape ([`FontStyle`]), so a new
//! role (a log font, say) is one more field here and one more entry in the UI's role list. A file
//! that is missing, or older than the fields the app now has, gets the defaults for what it lacks;
//! a file that cannot be read is moved aside before anything is saved over it.

use crate::{clock, fonts, paths::Paths, recipe::hex_colour};
use serde::{Deserialize, Serialize};

/// What every font defaults to.
pub const DEFAULT_FONT: &str = "Segoe UI";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FontSource {
    /// A font installed on this Windows.
    Installed,
    /// A Google Fonts family, downloaded once into the app's own folder.
    Google,
}

/// A line drawn round each letter. `width` is how thick it looks, in pixels.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Outline {
    pub enabled: bool,
    pub width: f64,
    /// `#rrggbb`.
    pub color: String,
    /// 0 to 100 percent.
    pub opacity: u32,
}

/// A soft copy of the text laid behind it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Shadow {
    pub enabled: bool,
    pub x: f64,
    pub y: f64,
    pub blur: f64,
    /// `#rrggbb`.
    pub color: String,
    /// 0 to 100 percent.
    pub opacity: u32,
}

/// One font role: which font, and how its letters are dressed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct FontStyle {
    pub source: FontSource,
    pub family: String,
    pub outline: Outline,
    pub shadow: Shadow,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FontSettings {
    /// Everything in the window unless a role below says otherwise.
    pub ui: FontStyle,
    /// The names on the server buttons in the sidebar.
    pub server_name: FontStyle,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Appearance {
    pub fonts: FontSettings,
}

impl Default for Outline {
    fn default() -> Self {
        Self { enabled: false, width: 1.0, color: "#000000".into(), opacity: 80 }
    }
}

impl Default for Shadow {
    fn default() -> Self {
        Self { enabled: false, x: 1.0, y: 1.0, blur: 2.0, color: "#000000".into(), opacity: 60 }
    }
}

impl Default for FontStyle {
    fn default() -> Self {
        Self { source: FontSource::Installed, family: DEFAULT_FONT.into(), outline: Outline::default(), shadow: Shadow::default() }
    }
}

fn within(value: f64, low: f64, high: f64) -> bool {
    value.is_finite() && (low..=high).contains(&value)
}

impl FontStyle {
    fn problem(&self, role: &str) -> Option<String> {
        let name_ok = match self.source {
            FontSource::Installed => fonts::valid_installed_name(&self.family),
            FontSource::Google => fonts::valid_google_family(&self.family),
        };
        if !name_ok {
            return Some(format!("The {role} font name '{}' is not usable.", self.family));
        }
        let (o, s) = (&self.outline, &self.shadow);
        if !within(o.width, 0.0, 8.0) || o.opacity > 100 || !hex_colour(&o.color) {
            return Some(format!("The {role} outline is out of range (width 0 to 8, opacity 0 to 100, colour #rrggbb)."));
        }
        if !within(s.x, -40.0, 40.0) || !within(s.y, -40.0, 40.0) || !within(s.blur, 0.0, 60.0) || s.opacity > 100 || !hex_colour(&s.color) {
            return Some(format!("The {role} shadow is out of range (offsets -40 to 40, blur 0 to 60, opacity 0 to 100, colour #rrggbb)."));
        }
        None
    }
}

impl Appearance {
    pub fn problem(&self) -> Option<String> {
        self.fonts.ui.problem("interface").or_else(|| self.fonts.server_name.problem("server name"))
    }
}

/// The saved appearance, or the defaults when nothing was saved yet.
pub fn load(paths: &Paths) -> Result<Appearance, String> {
    let path = paths.appearance_file();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let appearance: Appearance = serde_json::from_str(&text).map_err(|e| format!("{} cannot be read ({e}).", path.display()))?;
            match appearance.problem() {
                None => Ok(appearance),
                Some(p) => Err(format!("{} is not usable: {p}", path.display())),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Appearance::default()),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

/// Saves the appearance. A file that is there but cannot be read is kept as
/// `appearance.broken-<time>.json` first, so nothing the person wrote is lost silently.
pub fn save(paths: &Paths, appearance: &Appearance) -> Result<(), String> {
    if let Some(problem) = appearance.problem() {
        return Err(problem);
    }
    let path = paths.appearance_file();
    let folder = path.parent().ok_or("The appearance file has no folder")?;
    std::fs::create_dir_all(folder).map_err(|e| format!("Cannot create {}: {e}", folder.display()))?;
    if path.exists() && load(paths).is_err() {
        let aside = path.with_file_name(format!("appearance.broken-{}.json", clock::now()));
        std::fs::rename(&path, &aside).map_err(|e| format!("Cannot keep the unreadable {} as {}: {e}", path.display(), aside.display()))?;
    }
    let text = serde_json::to_string_pretty(appearance).map_err(|e| e.to_string())?;
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, text).map_err(|e| format!("Cannot write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().to_path_buf());
        (dir, paths)
    }

    #[test]
    fn every_font_defaults_to_segoe_ui_with_no_outline_or_shadow() {
        let a = Appearance::default();
        for style in [&a.fonts.ui, &a.fonts.server_name] {
            assert_eq!((style.source, style.family.as_str()), (FontSource::Installed, "Segoe UI"));
            assert!(!style.outline.enabled && !style.shadow.enabled);
        }
        assert_eq!(a.problem(), None);
    }

    #[test]
    fn what_is_saved_comes_back_and_nothing_saved_means_the_defaults() {
        let (_dir, paths) = paths();
        assert_eq!(load(&paths).unwrap(), Appearance::default());
        let mut a = Appearance::default();
        a.fonts.server_name = FontStyle {
            source: FontSource::Google,
            family: "Cinzel".into(),
            outline: Outline { enabled: true, width: 1.5, color: "#112233".into(), opacity: 90 },
            shadow: Shadow { enabled: true, x: 2.0, y: 3.0, blur: 4.0, color: "#445566".into(), opacity: 50 },
        };
        save(&paths, &a).unwrap();
        assert_eq!(load(&paths).unwrap(), a);
        let text = std::fs::read_to_string(paths.appearance_file()).unwrap();
        assert!(text.contains("\"serverName\"") && text.contains("\"google\""), "{text}");
    }

    #[test]
    fn a_file_from_before_a_field_existed_gets_the_defaults_for_it() {
        let (_dir, paths) = paths();
        std::fs::create_dir_all(paths.appearance_file().parent().unwrap()).unwrap();
        std::fs::write(paths.appearance_file(), r#"{ "fonts": { "ui": { "family": "Calibri" } } }"#).unwrap();
        let a = load(&paths).unwrap();
        assert_eq!(a.fonts.ui.family, "Calibri");
        assert_eq!(a.fonts.ui.source, FontSource::Installed);
        assert_eq!(a.fonts.server_name, FontStyle::default());
    }

    #[test]
    fn values_that_could_break_the_css_or_a_path_are_refused() {
        let (_dir, paths) = paths();
        let mut cases: Vec<Appearance> = Vec::new();
        for name in ["", "Segoe\" ; } body { display:none", "a\\b", "line\nbreak", "<x>"] {
            let mut a = Appearance::default();
            a.fonts.ui.family = name.into();
            cases.push(a);
        }
        for name in ["../Cinzel", "Cinzel;x", "Cinzel'"] {
            let mut a = Appearance::default();
            a.fonts.server_name = FontStyle { source: FontSource::Google, family: name.into(), ..FontStyle::default() };
            cases.push(a);
        }
        for tweak in [
            (|a: &mut Appearance| a.fonts.ui.outline.width = 9.0) as fn(&mut Appearance),
            |a| a.fonts.ui.outline.width = f64::NAN,
            |a| a.fonts.ui.outline.opacity = 101,
            |a| a.fonts.ui.outline.color = "red".into(),
            |a| a.fonts.ui.shadow.blur = -1.0,
            |a| a.fonts.ui.shadow.x = 41.0,
            |a| a.fonts.ui.shadow.color = "#12345".into(),
            |a| a.fonts.server_name.shadow.color = "url(x)".into(),
        ] {
            let mut a = Appearance::default();
            tweak(&mut a);
            cases.push(a);
        }
        for a in cases {
            assert!(a.problem().is_some(), "{a:?}");
            assert!(save(&paths, &a).is_err());
        }
        assert!(!paths.appearance_file().exists(), "a refused save writes nothing");
    }

    #[test]
    fn an_unreadable_file_is_kept_aside_before_it_is_replaced() {
        let (_dir, paths) = paths();
        std::fs::create_dir_all(paths.appearance_file().parent().unwrap()).unwrap();
        std::fs::write(paths.appearance_file(), "{ not json").unwrap();
        assert!(load(&paths).unwrap_err().contains("cannot be read"));
        save(&paths, &Appearance::default()).unwrap();
        assert_eq!(load(&paths).unwrap(), Appearance::default());
        let kept: Vec<_> = std::fs::read_dir(paths.appearance_file().parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("appearance.broken-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(kept[0].path()).unwrap(), "{ not json");
    }
}
