//! =============================================================================
//! Configuration And Settings
//! =============================================================================
//!
//! It owns every user facing knob (diagnostic strategy, formatting preferences, code lens modes,
//! jsx helpers, tsserver memory limits, etc.) and exposes typed structures that
//! other subsystems borrow.

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
#[derive(Debug, Clone, Copy)]
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

    pub fn plugin(&self) -> &PluginSettings {
        &self.plugin
    }
}
