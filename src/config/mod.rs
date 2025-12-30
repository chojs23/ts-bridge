//! =============================================================================
//! Configuration And Settings
//! =============================================================================
//!
//! It owns every user facing knob (diagnostic strategy, formatting preferences, code lens modes,
//! jsx helpers, tsserver memory limits, etc.) and exposes typed structures that
//! other subsystems borrow.

use std::path::PathBuf;

use serde_json::{Map, Value};

/// Settings that are evaluated once during plugin setup (analogous to the Lua
/// `settings` table).  Additional fields will be introduced as we port features.
#[derive(Debug, Clone)]
pub struct PluginSettings {
    /// Whether we spin up a paired semantic tsserver dedicated to diagnostics.
    pub separate_diagnostic_server: bool,
    /// Determines when diagnostics are requested (`"insert_leave"` vs
    /// `"change"` originally); kept simple for now.
    pub publish_diagnostic_on: DiagnosticPublishMode,
    /// Launch arguments and logging preferences forwarded to tsserver.
    pub tsserver: TsserverLaunchOptions,
    /// Gate for tsserver-backed inlay hints; allows users to disable the feature entirely.
    pub enable_inlay_hints: bool,
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            separate_diagnostic_server: true,
            publish_diagnostic_on: DiagnosticPublishMode::InsertLeave,
            tsserver: TsserverLaunchOptions::default(),
            enable_inlay_hints: true,
        }
    }
}

/// Diagnostic scheduling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticPublishMode {
    InsertLeave,
    Change,
}

impl DiagnosticPublishMode {
    /// Parses a string-based setting (e.g. loaded via serde/JSON) into the enum.
    pub fn from_str(value: &str) -> Self {
        match value {
            "change" => Self::Change,
            _ => Self::InsertLeave,
        }
    }
}

/// Global configuration facade that exposes read-only handles to each settings struct.
#[derive(Debug, Default)]
pub struct Config {
    plugin: PluginSettings,
}

impl Config {
    pub fn new(plugin: PluginSettings) -> Self {
        Self { plugin }
    }

    pub fn plugin_mut(&mut self) -> &mut PluginSettings {
        &mut self.plugin
    }

    pub fn plugin(&self) -> &PluginSettings {
        &self.plugin
    }

    /// Applies workspace/didChangeConfiguration payloads to the cached
    /// settings. Returns `true` when any recognized option changed.
    pub fn apply_workspace_settings(&mut self, settings: &Value) -> bool {
        apply_settings_tree(settings, &mut self.plugin)
    }
}

fn apply_settings_tree(value: &Value, plugin: &mut PluginSettings) -> bool {
    let mut changed = false;
    if let Some(map) = value.as_object() {
        changed |= plugin.update_from_map(map);

        for key in POSSIBLE_SETTING_ROOTS {
            if let Some(candidate) = map.get(*key) {
                changed |= apply_settings_tree(candidate, plugin);
            }
        }

        if let Some(plugin_section) = map.get("plugin") {
            changed |= apply_settings_tree(plugin_section, plugin);
        }
    }
    changed
}

const POSSIBLE_SETTING_ROOTS: &[&str] = &["ts-bridge", "tsBridge", "tsbridge", "ts_bridge"];

impl PluginSettings {
    fn update_from_map(&mut self, map: &Map<String, Value>) -> bool {
        let mut changed = false;

        if let Some(value) = map
            .get("separate_diagnostic_server")
            .and_then(|v| v.as_bool())
        {
            if self.separate_diagnostic_server != value {
                self.separate_diagnostic_server = value;
                changed = true;
            }
        }

        if let Some(value) = map.get("publish_diagnostic_on").and_then(|v| v.as_str()) {
            let mode = DiagnosticPublishMode::from_str(value);
            if self.publish_diagnostic_on != mode {
                self.publish_diagnostic_on = mode;
                changed = true;
            }
        }

        if let Some(tsserver) = map.get("tsserver") {
            changed |= self.tsserver.update_from_value(tsserver);
        }

        if let Some(value) = map.get("enable_inlay_hints").and_then(|v| v.as_bool()) {
            if self.enable_inlay_hints != value {
                self.enable_inlay_hints = value;
                changed = true;
            }
        }

        changed
    }
}

/// Launch-related knobs for the underlying `tsserver` Node process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TsserverLaunchOptions {
    pub locale: Option<String>,
    pub log_directory: Option<PathBuf>,
    pub log_verbosity: Option<TsserverLogVerbosity>,
    pub max_old_space_size: Option<u32>,
    pub global_plugins: Vec<String>,
    pub plugin_probe_dirs: Vec<PathBuf>,
    pub extra_args: Vec<String>,
}

impl TsserverLaunchOptions {
    fn update_from_value(&mut self, value: &Value) -> bool {
        let map = match value.as_object() {
            Some(map) => map,
            None => return false,
        };
        let mut changed = false;

        if map.contains_key("locale") {
            let next = map
                .get("locale")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if self.locale != next {
                self.locale = next;
                changed = true;
            }
        }

        if map.contains_key("log_directory") {
            let next = map
                .get("log_directory")
                .and_then(|v| v.as_str())
                .map(PathBuf::from);
            if self.log_directory != next {
                self.log_directory = next;
                changed = true;
            }
        }

        if map.contains_key("log_verbosity") {
            let next = map
                .get("log_verbosity")
                .and_then(|v| v.as_str())
                .and_then(TsserverLogVerbosity::from_str);
            if self.log_verbosity != next {
                self.log_verbosity = next;
                changed = true;
            }
        }

        if map.contains_key("max_old_space_size") {
            let next = map
                .get("max_old_space_size")
                .and_then(|v| v.as_u64())
                .and_then(|v| v.try_into().ok());
            if self.max_old_space_size != next {
                self.max_old_space_size = next;
                changed = true;
            }
        }

        if let Some(list) = map
            .get("global_plugins")
            .and_then(|value| string_list(value))
        {
            if self.global_plugins != list {
                self.global_plugins = list;
                changed = true;
            }
        }

        if let Some(list) = map
            .get("plugin_probe_dirs")
            .and_then(|value| string_list(value))
            .map(|entries| entries.into_iter().map(PathBuf::from).collect::<Vec<_>>())
        {
            if self.plugin_probe_dirs != list {
                self.plugin_probe_dirs = list;
                changed = true;
            }
        }

        if let Some(list) = map.get("extra_args").and_then(|value| string_list(value)) {
            if self.extra_args != list {
                self.extra_args = list;
                changed = true;
            }
        }

        changed
    }
}

fn string_list(value: &Value) -> Option<Vec<String>> {
    let array = value.as_array()?;
    let mut result = Vec::with_capacity(array.len());
    for entry in array {
        let Some(text) = entry.as_str() else {
            continue;
        };
        result.push(text.to_string());
    }
    Some(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsserverLogVerbosity {
    Terse,
    Normal,
    RequestTime,
    Verbose,
}

impl TsserverLogVerbosity {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "terse" => Some(Self::Terse),
            "normal" => Some(Self::Normal),
            "requestTime" | "request_time" => Some(Self::RequestTime),
            "verbose" => Some(Self::Verbose),
            _ => None,
        }
    }

    pub fn as_cli_flag(&self) -> &'static str {
        match self {
            Self::Terse => "terse",
            Self::Normal => "normal",
            Self::RequestTime => "requestTime",
            Self::Verbose => "verbose",
        }
    }
}
