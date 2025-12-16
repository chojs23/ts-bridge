use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, PublishDiagnosticsParams};
use serde_json::Value;

use crate::utils::{file_path_to_uri, tsserver_range_from_value_lsp};

const DIAG_EVENTS: &[&str] = &["semanticDiag", "syntaxDiag", "suggestionDiag"];

pub fn from_tsserver_event(payload: &Value) -> Option<PublishDiagnosticsParams> {
    if payload.get("type")?.as_str()? != "event" {
        return None;
    }
    let event_name = payload.get("event")?.as_str()?;
    if !DIAG_EVENTS.contains(&event_name) {
        return None;
    }

    let body = payload.get("body")?;
    let file = body.get("file")?.as_str()?;
    let uri = file_path_to_uri(file)?;
    let diagnostics = body.get("diagnostics")?.as_array()?;

    let lsp_diagnostics = diagnostics
        .iter()
        .filter_map(convert_diagnostic)
        .collect::<Vec<_>>();

    Some(PublishDiagnosticsParams {
        uri,
        diagnostics: lsp_diagnostics,
        version: None,
    })
}

fn convert_diagnostic(value: &Value) -> Option<Diagnostic> {
    let range = tsserver_range_from_value_lsp(value)?;
    let message = value.get("text")?.as_str()?.to_string();
    let severity = map_severity(value.get("category").and_then(|v| v.as_str()));
    let code = value
        .get("code")
        .and_then(|c| c.as_i64())
        .map(|code| NumberOrString::Number(code as i32));

    Some(Diagnostic {
        range,
        severity,
        code,
        source: Some("tsserver".to_string()),
        message,
        ..Diagnostic::default()
    })
}

fn map_severity(category: Option<&str>) -> Option<DiagnosticSeverity> {
    match category {
        Some("error") => Some(DiagnosticSeverity::ERROR),
        Some("warning") => Some(DiagnosticSeverity::WARNING),
        Some("suggestion") => Some(DiagnosticSeverity::HINT),
        Some("message") => Some(DiagnosticSeverity::INFORMATION),
        _ => Some(DiagnosticSeverity::WARNING),
    }
}
