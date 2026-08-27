//! Config nach Spec §8: Defaults, Laden, drei Validierungsklassen, atomares Schreiben.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_MODEL: &str = "parakeet-tdt-0.6b-v3-int8";

/// Inhalt der Default-Datei, kommentiert wie in Spec §8.
pub const DEFAULT_TOML: &str = r#"[hotkey]
key = "F9"              # z. B. "F9", "ScrollLock", "Pause"
modifiers = []
mode = "push_to_talk"   # v1 nur dieser Wert

[audio]
device = "default"
sample_rate = 16000     # Engine-Zielrate, nur 16000
max_duration_secs = 60

[engine]
model = "parakeet-tdt-0.6b-v3-int8"
threads = 0             # 0 = Runtime-Default

[output]
mode = "paste"          # "paste" | "type"
paste_shortcut = "auto"
leading_space = true
restore_clipboard = true
restore_clipboard_delay_ms = 200

[tray]
show_notifications_on_error = true
"#;

const HOTKEY_KEYS: &[&str] = &["key", "modifiers", "mode"];
const AUDIO_KEYS: &[&str] = &["device", "sample_rate", "max_duration_secs"];
const ENGINE_KEYS: &[&str] = &["model", "threads"];
const OUTPUT_KEYS: &[&str] = &[
    "mode",
    "paste_shortcut",
    "leading_space",
    "restore_clipboard",
    "restore_clipboard_delay_ms",
];
const TRAY_KEYS: &[&str] = &["show_notifications_on_error"];

const NAMED_KEYS: &[&str] = &[
    "Space",
    "Tab",
    "Enter",
    "Escape",
    "Backspace",
    "Insert",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Left",
    "Right",
    "Up",
    "Down",
    "ScrollLock",
    "Pause",
];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("TOML-Syntaxfehler: {0}")]
    Syntax(String),
    #[error("{0}")]
    Fatal(String),
    #[error("Config-Datei: {0}")]
    Io(#[from] io::Error),
    #[error("Config-Pfad: {0}")]
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: Config,
    pub warnings: Vec<String>,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    pub hotkey: HotkeyConfig,
    pub audio: AudioConfig,
    pub engine: EngineConfig,
    pub output: OutputConfig,
    pub tray: TrayConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyConfig {
    pub key: String,
    pub modifiers: Vec<Modifier>,
    pub mode: HotkeyMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Super,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyMode {
    PushToTalk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioConfig {
    pub device: String,
    pub sample_rate: u32,
    pub max_duration_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    pub model: String,
    pub threads: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputConfig {
    pub mode: OutputMode,
    pub paste_shortcut: PasteShortcut,
    pub leading_space: bool,
    pub restore_clipboard: bool,
    pub restore_clipboard_delay_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Paste,
    Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteShortcut {
    Auto,
    CtrlV,
    CtrlShiftV,
    ShiftInsert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayConfig {
    pub show_notifications_on_error: bool,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            key: "F9".into(),
            modifiers: Vec::new(),
            mode: HotkeyMode::PushToTalk,
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device: "default".into(),
            sample_rate: 16_000,
            max_duration_secs: 60,
        }
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.into(),
            threads: 0,
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            mode: OutputMode::Paste,
            paste_shortcut: PasteShortcut::Auto,
            leading_space: true,
            restore_clipboard: true,
            restore_clipboard_delay_ms: 200,
        }
    }
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            show_notifications_on_error: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    hotkey: RawHotkey,
    audio: RawAudio,
    engine: RawEngine,
    output: RawOutput,
    tray: RawTray,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RawHotkey {
    key: String,
    modifiers: Vec<String>,
    mode: String,
}

impl Default for RawHotkey {
    fn default() -> Self {
        Self {
            key: "F9".into(),
            modifiers: Vec::new(),
            mode: "push_to_talk".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RawAudio {
    device: String,
    sample_rate: i64,
    max_duration_secs: i64,
}

impl Default for RawAudio {
    fn default() -> Self {
        Self {
            device: "default".into(),
            sample_rate: 16_000,
            max_duration_secs: 60,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RawEngine {
    model: String,
    threads: i64,
}

impl Default for RawEngine {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.into(),
            threads: 0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RawOutput {
    mode: String,
    paste_shortcut: String,
    leading_space: bool,
    restore_clipboard: bool,
    restore_clipboard_delay_ms: i64,
}

impl Default for RawOutput {
    fn default() -> Self {
        Self {
            mode: "paste".into(),
            paste_shortcut: "auto".into(),
            leading_space: true,
            restore_clipboard: true,
            restore_clipboard_delay_ms: 200,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RawTray {
    show_notifications_on_error: bool,
}

impl Default for RawTray {
    fn default() -> Self {
        Self {
            show_notifications_on_error: true,
        }
    }
}

/// Linux: `~/.config/diktier/config.toml`. Windows: `%APPDATA%\diktier\config.toml`.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| ConfigError::Path("Umgebungsvariable HOME ist nicht gesetzt".into()))?;
        Ok(PathBuf::from(home)
            .join(".config")
            .join("diktier")
            .join("config.toml"))
    }
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA").ok_or_else(|| {
            ConfigError::Path("Umgebungsvariable APPDATA ist nicht gesetzt".into())
        })?;
        Ok(PathBuf::from(appdata).join("diktier").join("config.toml"))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        compile_error!("diktier unterstützt nur Linux und Windows");
    }
}

pub fn load() -> Result<LoadedConfig, ConfigError> {
    load_from(&config_path()?)
}

pub fn load_from(path: &Path) -> Result<LoadedConfig, ConfigError> {
    if !path.exists() {
        write_atomic(path, DEFAULT_TOML)?;
        return Ok(LoadedConfig {
            config: Config::default(),
            warnings: Vec::new(),
            created: true,
        });
    }
    let text = fs::read_to_string(path)?;
    parse_toml(&text)
}

pub fn parse_toml(text: &str) -> Result<LoadedConfig, ConfigError> {
    if text.trim().is_empty() {
        return Ok(LoadedConfig {
            config: Config::default(),
            warnings: Vec::new(),
            created: false,
        });
    }

    let value: toml::Value =
        toml::from_str(text).map_err(|e| ConfigError::Syntax(e.to_string()))?;
    let mut warnings = Vec::new();
    collect_unknown_keys(&value, &mut warnings);

    let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError::Fatal(e.to_string()))?;
    let config = validate_and_clamp(raw, &mut warnings)?;
    Ok(LoadedConfig {
        config,
        warnings,
        created: false,
    })
}

fn collect_unknown_keys(value: &toml::Value, warnings: &mut Vec<String>) {
    let Some(table) = value.as_table() else {
        return;
    };
    for (key, val) in table {
        match key.as_str() {
            "hotkey" => collect_unknown_table(val, "hotkey", HOTKEY_KEYS, warnings),
            "audio" => collect_unknown_table(val, "audio", AUDIO_KEYS, warnings),
            "engine" => collect_unknown_table(val, "engine", ENGINE_KEYS, warnings),
            "output" => collect_unknown_table(val, "output", OUTPUT_KEYS, warnings),
            "tray" => collect_unknown_table(val, "tray", TRAY_KEYS, warnings),
            other => warnings.push(format!(
                "Unbekannter Config-Schlüssel wird ignoriert: {other}"
            )),
        }
    }
}

fn collect_unknown_table(
    value: &toml::Value,
    prefix: &str,
    known: &[&str],
    warnings: &mut Vec<String>,
) {
    let Some(table) = value.as_table() else {
        return;
    };
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            warnings.push(format!(
                "Unbekannter Config-Schlüssel wird ignoriert: {prefix}.{key}"
            ));
        }
    }
}

fn validate_and_clamp(raw: RawConfig, warnings: &mut Vec<String>) -> Result<Config, ConfigError> {
    let key = canonical_key(&raw.hotkey.key)
        .ok_or_else(|| ConfigError::Fatal(format!("ungültiges hotkey.key {:?}", raw.hotkey.key)))?;

    if raw.hotkey.mode != "push_to_talk" {
        return Err(ConfigError::Fatal(format!(
            "ungültiges hotkey.mode {:?} (v1 nur push_to_talk)",
            raw.hotkey.mode
        )));
    }

    let mut modifiers = Vec::with_capacity(raw.hotkey.modifiers.len());
    for item in &raw.hotkey.modifiers {
        modifiers.push(parse_modifier(item).ok_or_else(|| {
            ConfigError::Fatal(format!("ungültiger hotkey.modifiers-Eintrag {item:?}"))
        })?);
    }

    if raw.audio.sample_rate != 16_000 {
        return Err(ConfigError::Fatal(format!(
            "ungültiges audio.sample_rate {} (v1 nur 16000)",
            raw.audio.sample_rate
        )));
    }

    if raw.engine.model != DEFAULT_MODEL {
        return Err(ConfigError::Fatal(format!(
            "ungültiges engine.model {:?}",
            raw.engine.model
        )));
    }

    let output_mode = match raw.output.mode.as_str() {
        "paste" => OutputMode::Paste,
        "type" => OutputMode::Type,
        other => {
            return Err(ConfigError::Fatal(format!(
                "ungültiges output.mode {other:?}"
            )));
        }
    };

    let paste_shortcut = match raw.output.paste_shortcut.as_str() {
        "auto" => PasteShortcut::Auto,
        "ctrl_v" => PasteShortcut::CtrlV,
        "ctrl_shift_v" => PasteShortcut::CtrlShiftV,
        "shift_insert" => PasteShortcut::ShiftInsert,
        other => {
            return Err(ConfigError::Fatal(format!(
                "ungültiges output.paste_shortcut {other:?}"
            )));
        }
    };

    let max_cpus = logical_cpus() as i64;
    let max_duration_secs = clamp_i64(
        raw.audio.max_duration_secs,
        1,
        60,
        "max_duration_secs",
        warnings,
    ) as u32;
    let restore_clipboard_delay_ms = clamp_i64(
        raw.output.restore_clipboard_delay_ms,
        0,
        5000,
        "restore_clipboard_delay_ms",
        warnings,
    ) as u32;
    let threads = clamp_i64(raw.engine.threads, 0, max_cpus, "threads", warnings) as u32;

    Ok(Config {
        hotkey: HotkeyConfig {
            key,
            modifiers,
            mode: HotkeyMode::PushToTalk,
        },
        audio: AudioConfig {
            device: raw.audio.device,
            sample_rate: 16_000,
            max_duration_secs,
        },
        engine: EngineConfig {
            model: raw.engine.model,
            threads,
        },
        output: OutputConfig {
            mode: output_mode,
            paste_shortcut,
            leading_space: raw.output.leading_space,
            restore_clipboard: raw.output.restore_clipboard,
            restore_clipboard_delay_ms,
        },
        tray: TrayConfig {
            show_notifications_on_error: raw.tray.show_notifications_on_error,
        },
    })
}

fn clamp_i64(value: i64, min: i64, max: i64, name: &str, warnings: &mut Vec<String>) -> i64 {
    let clamped = value.clamp(min, max);
    if clamped != value {
        warnings.push(format!(
            "{name}={value} liegt außerhalb {min}..={max}, auf {clamped} begrenzt"
        ));
    }
    clamped
}

fn logical_cpus() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

fn parse_modifier(raw: &str) -> Option<Modifier> {
    match raw.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(Modifier::Ctrl),
        "shift" => Some(Modifier::Shift),
        "alt" => Some(Modifier::Alt),
        "super" | "win" | "meta" => Some(Modifier::Super),
        _ => None,
    }
}

fn canonical_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let upper = trimmed.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix('F')
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        if let Ok(n) = rest.parse::<u8>()
            && (1..=24).contains(&n)
        {
            return Some(format!("F{n}"));
        }
        return None;
    }
    if upper.len() == 1 {
        let c = upper.chars().next()?;
        if c.is_ascii_alphanumeric() {
            return Some(upper);
        }
    }
    // Deutsche Tastaturbeschriftung: „Rollen" ist ScrollLock.
    if trimmed.eq_ignore_ascii_case("Rollen") {
        return Some("ScrollLock".to_string());
    }
    for name in NAMED_KEYS {
        if name.eq_ignore_ascii_case(trimmed) {
            return Some((*name).to_string());
        }
    }
    None
}

/// Schreibweise der Modifier in `config.toml` — das, was [`parse_modifier`]
/// wieder annimmt.
#[cfg(windows)]
pub fn modifier_config_name(modifier: Modifier) -> &'static str {
    match modifier {
        Modifier::Ctrl => "ctrl",
        Modifier::Shift => "shift",
        Modifier::Alt => "alt",
        Modifier::Super => "super",
    }
}

/// §4.4 + „Hotkey ändern…": `hotkey.key` und `hotkey.modifiers` ersetzen und
/// **sonst nichts** anfassen.
///
/// Deshalb `toml_edit` statt `toml::to_string_pretty`: Die Datei ist zum
/// Selbstschreiben gedacht und trägt die Kommentare aus [`DEFAULT_TOML`] —
/// ein Roundtrip über `toml::Value` würde sie alle verlieren. Die Dekoration
/// der beiden geänderten Werte (der Kommentar hinter `key = "F9"`) wird
/// mitgenommen, alles andere bleibt Zeichen für Zeichen stehen.
///
/// Fehlt die Datei, entsteht sie aus [`DEFAULT_TOML`] — derselbe Weg wie in
/// [`load_from`], nur mit gesetztem Hotkey.
#[cfg(windows)]
pub fn save_hotkey(path: &Path, key: &str, modifiers: &[Modifier]) -> Result<(), ConfigError> {
    use toml_edit::{DocumentMut, Item, Table, Value, value};

    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => DEFAULT_TOML.to_string(),
        Err(err) => return Err(ConfigError::Io(err)),
    };
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|e| ConfigError::Syntax(e.to_string()))?;

    let hotkey = doc.entry("hotkey").or_insert(Item::Table(Table::new()));
    let hotkey = hotkey
        .as_table_like_mut()
        .ok_or_else(|| ConfigError::Fatal("[hotkey] ist keine Tabelle".into()))?;

    // Wert tauschen, Dekoration (Whitespace + Zeilenkommentar) behalten.
    let mut set = |name: &str, new: Value| {
        let decor = hotkey
            .get(name)
            .and_then(Item::as_value)
            .map(|old| old.decor().clone());
        hotkey.insert(name, value(new));
        if let Some(decor) = decor
            && let Some(slot) = hotkey.get_mut(name).and_then(Item::as_value_mut)
        {
            *slot.decor_mut() = decor;
        }
    };

    set("key", Value::from(key));
    set(
        "modifiers",
        Value::Array(
            modifiers
                .iter()
                .copied()
                .map(modifier_config_name)
                .collect(),
        ),
    );

    write_atomic(path, &doc.to_string())?;
    Ok(())
}

fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp = {
        let mut name = path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("config.toml"))
            .to_os_string();
        name.push(".tmp");
        path.with_file_name(name)
    };
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_warning_contains(warnings: &[String], needle: &str) {
        assert!(
            warnings.iter().any(|w| w.contains(needle)),
            "erwartete Warnung mit {needle:?}, got {warnings:?}"
        );
    }

    #[test]
    fn default_toml_matches_struct_defaults() {
        let loaded = parse_toml(DEFAULT_TOML).unwrap();
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.warnings.is_empty());
        assert!(!loaded.created);
    }

    #[test]
    fn fatal_toml_syntax() {
        let err = parse_toml("[[[not toml").unwrap_err();
        assert!(matches!(err, ConfigError::Syntax(_)), "got {err:?}");
    }

    /// Die „tut sonst nichts"-Tasten sind gültige Hotkeys; „Rollen" ist die
    /// deutsche Beschriftung von ScrollLock.
    #[test]
    fn lock_keys_are_valid_hotkeys() {
        for (raw, expected) in [
            ("ScrollLock", "ScrollLock"),
            ("scrolllock", "ScrollLock"),
            ("Rollen", "ScrollLock"),
            ("pause", "Pause"),
        ] {
            let loaded = parse_toml(&format!("[hotkey]\nkey = \"{raw}\"\n")).unwrap();
            assert_eq!(loaded.config.hotkey.key, expected, "{raw}");
            assert!(loaded.warnings.is_empty(), "{raw}: {:?}", loaded.warnings);
        }
    }

    #[test]
    fn fatal_invalid_hotkey_key() {
        let err = parse_toml(
            r#"
[hotkey]
key = "F99"
"#,
        )
        .unwrap_err();
        match err {
            ConfigError::Fatal(msg) => assert!(msg.contains("hotkey.key"), "{msg}"),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn fatal_invalid_output_mode() {
        let err = parse_toml(
            r#"
[output]
mode = "clipboard"
"#,
        )
        .unwrap_err();
        match err {
            ConfigError::Fatal(msg) => assert!(msg.contains("output.mode"), "{msg}"),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn fatal_invalid_engine_model() {
        let err = parse_toml(
            r#"
[engine]
model = "whisper-medium"
"#,
        )
        .unwrap_err();
        match err {
            ConfigError::Fatal(msg) => assert!(msg.contains("engine.model"), "{msg}"),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn unknown_keys_are_ignored_with_warning() {
        let loaded = parse_toml(
            r#"
typo = 1
[audio]
samplee_rate = 48000
max_duration_secs = 30
"#,
        )
        .unwrap();
        assert_eq!(loaded.config.audio.max_duration_secs, 30);
        assert_eq!(loaded.config.audio.sample_rate, 16_000);
        assert_warning_contains(&loaded.warnings, "typo");
        assert_warning_contains(&loaded.warnings, "audio.samplee_rate");
    }

    #[test]
    fn clamp_out_of_range_numbers() {
        let loaded = parse_toml(
            r#"
[audio]
max_duration_secs = 90
[output]
restore_clipboard_delay_ms = 9000
[engine]
threads = 999999
"#,
        )
        .unwrap();
        assert_eq!(loaded.config.audio.max_duration_secs, 60);
        assert_eq!(loaded.config.output.restore_clipboard_delay_ms, 5000);
        assert_eq!(loaded.config.engine.threads, logical_cpus() as u32);
        assert_warning_contains(&loaded.warnings, "max_duration_secs");
        assert_warning_contains(&loaded.warnings, "restore_clipboard_delay_ms");
        assert_warning_contains(&loaded.warnings, "threads");
    }

    #[test]
    fn clamp_bounds_are_inclusive() {
        let at_bounds = parse_toml(
            r#"
[audio]
max_duration_secs = 1
[output]
restore_clipboard_delay_ms = 0
[engine]
threads = 0
"#,
        )
        .unwrap();
        assert_eq!(at_bounds.config.audio.max_duration_secs, 1);
        assert_eq!(at_bounds.config.output.restore_clipboard_delay_ms, 0);
        assert_eq!(at_bounds.config.engine.threads, 0);
        assert!(at_bounds.warnings.is_empty());

        let other_bounds = parse_toml(
            r#"
[audio]
max_duration_secs = 60
[output]
restore_clipboard_delay_ms = 5000
"#,
        )
        .unwrap();
        assert_eq!(other_bounds.config.audio.max_duration_secs, 60);
        assert_eq!(other_bounds.config.output.restore_clipboard_delay_ms, 5000);
        assert!(other_bounds.warnings.is_empty());

        let below = parse_toml(
            r#"
[audio]
max_duration_secs = 0
[output]
restore_clipboard_delay_ms = -1
[engine]
threads = -5
"#,
        )
        .unwrap();
        assert_eq!(below.config.audio.max_duration_secs, 1);
        assert_eq!(below.config.output.restore_clipboard_delay_ms, 0);
        assert_eq!(below.config.engine.threads, 0);
        assert_warning_contains(&below.warnings, "max_duration_secs");
        assert_warning_contains(&below.warnings, "restore_clipboard_delay_ms");
        assert_warning_contains(&below.warnings, "threads");
    }

    #[test]
    fn atomic_write_creates_parent_dirs_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("nested")
            .join("diktier")
            .join("config.toml");
        let loaded = load_from(&path).unwrap();
        assert!(loaded.created);
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.warnings.is_empty());
        let on_disk = fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, DEFAULT_TOML);
        assert!(!path.with_file_name("config.toml.tmp").exists());
    }

    #[test]
    fn existing_file_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[audio]
max_duration_secs = 15
"#,
        )
        .unwrap();
        let loaded = load_from(&path).unwrap();
        assert!(!loaded.created);
        assert_eq!(loaded.config.audio.max_duration_secs, 15);
        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("max_duration_secs = 15"));
    }

    /// „Hotkey ändern…" darf die Datei nicht umschreiben: Kommentare,
    /// Reihenfolge und alle übrigen Abschnitte bleiben stehen, und das
    /// Ergebnis lädt ohne Warnung wieder.
    #[cfg(windows)]
    #[test]
    fn saving_a_hotkey_keeps_comments_and_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, DEFAULT_TOML).unwrap();

        save_hotkey(&path, "F12", &[Modifier::Ctrl, Modifier::Alt]).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains(r#"# z. B. "F9", "ScrollLock", "Pause""#),
            "Kommentar der geänderten Zeile verloren:
{text}"
        );
        assert!(text.contains("# v1 nur dieser Wert"), "{text}");
        assert!(
            text.contains("show_notifications_on_error = true"),
            "{text}"
        );
        assert!(!path.with_file_name("config.toml.tmp").exists());

        let loaded = parse_toml(&text).unwrap();
        assert_eq!(loaded.config.hotkey.key, "F12");
        assert_eq!(
            loaded.config.hotkey.modifiers,
            vec![Modifier::Ctrl, Modifier::Alt]
        );
        assert_eq!(loaded.config.hotkey.mode, HotkeyMode::PushToTalk);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        // Alles außerhalb von [hotkey] ist unverändert.
        assert_eq!(loaded.config.audio, AudioConfig::default());
        assert_eq!(loaded.config.output, OutputConfig::default());
    }

    /// Fehlt die Datei, entsteht sie mit den Defaults **und** dem neuen
    /// Hotkey — derselbe Weg wie in `load_from`.
    #[cfg(windows)]
    #[test]
    fn saving_a_hotkey_creates_the_file_from_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        save_hotkey(&path, "ScrollLock", &[]).unwrap();

        let loaded = load_from(&path).unwrap();
        assert!(!loaded.created);
        assert_eq!(loaded.config.hotkey.key, "ScrollLock");
        assert!(loaded.config.hotkey.modifiers.is_empty());
        assert_eq!(loaded.config.engine.model, DEFAULT_MODEL);
    }

    #[test]
    fn config_path_matches_spec() {
        let path = config_path().unwrap();
        #[cfg(target_os = "linux")]
        assert!(path.ends_with(".config/diktier/config.toml"));
        #[cfg(windows)]
        assert!(path.ends_with("diktier\\config.toml") || path.ends_with("diktier/config.toml"));
    }
}
