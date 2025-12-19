//! =============================================================================
//! Configuration And Settings
//! =============================================================================
//!
//! It owns every user facing knob (diagnostic strategy, formatting preferences, code lens modes,
//! jsx helpers, tsserver memory limits, etc.) and exposes typed structures that
//! other subsystems borrow.

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
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            separate_diagnostic_server: true,
            publish_diagnostic_on: DiagnosticPublishMode::InsertLeave,
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

/// Global configuration facade that exposes read-only handles to each concrete
/// settings struct.  Eventually this will also provide runtime reloading when
/// Neovim pushes `workspace/didChangeConfiguration`.
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

const POSSIBLE_SETTING_ROOTS: &[&str] = &[
    "ts-bridge",
    "tsBridge",
    "tsbridge",
    "ts_bridge",
    "typescript-tools",
    "typescriptTools",
];

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

        changed
    }
}
