use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Uri};
use serde_json::{Value, json};

use crate::protocol::NotificationSpec;
use crate::rpc::{Priority, Route};
use crate::utils::{file_path_to_uri, tsserver_range_from_value_lsp};

const REQUEST_COMPLETED: &str = "requestCompleted";

#[derive(Debug, Clone, Copy)]
pub enum DiagnosticsKind {
    Syntax,
    Semantic,
    Suggestion,
}

impl DiagnosticsKind {
    fn from_event_name(name: &str) -> Option<Self> {
        match name {
            "syntaxDiag" => Some(DiagnosticsKind::Syntax),
            "semanticDiag" => Some(DiagnosticsKind::Semantic),
            "suggestionDiag" => Some(DiagnosticsKind::Suggestion),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum DiagnosticsEvent {
    Report {
        kind: DiagnosticsKind,
        request_seq: Option<u64>,
        uri: Uri,
        diagnostics: Vec<Diagnostic>,
    },
    Completed {
        request_seq: u64,
    },
}

pub fn request_for_file(file: &str) -> NotificationSpec {
    let payload = json!({
        "command": "geterr",
        "arguments": {
            "files": [file],
            "delay": 0,
        }
    });

    NotificationSpec {
        route: Route::Both,
        payload,
        priority: Priority::Low,
    }
}

pub fn parse_tsserver_event(payload: &Value) -> Option<DiagnosticsEvent> {
    if payload.get("type")?.as_str()? != "event" {
        return None;
    }
    let event_name = payload.get("event")?.as_str()?;
    if event_name == REQUEST_COMPLETED {
        let seq = payload
            .get("body")
            .and_then(|body| body.get("request_seq"))
            .and_then(|value| value.as_u64())?;
        return Some(DiagnosticsEvent::Completed { request_seq: seq });
    }
    let kind = DiagnosticsKind::from_event_name(event_name)?;

    let body = payload.get("body")?;
    let file = body.get("file")?.as_str()?;
    let uri = file_path_to_uri(file)?;
    let request_seq = body.get("request_seq").and_then(|value| value.as_u64());
    let diagnostics = body
        .get("diagnostics")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let lsp_diagnostics = diagnostics
        .into_iter()
        .filter_map(convert_diagnostic)
        .collect::<Vec<_>>();

    Some(DiagnosticsEvent::Report {
        request_seq,
        kind,
        uri,
        diagnostics: lsp_diagnostics,
    })
}

fn convert_diagnostic(value: Value) -> Option<Diagnostic> {
    let range = tsserver_range_from_value_lsp(&value)?;
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
