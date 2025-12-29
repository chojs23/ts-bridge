//! =============================================================================
//! Open Document Store
//! =============================================================================
//!
//! Tracks the latest text for each open buffer so features that rely on
//! absolute offsets (e.g. inlay hints) can translate between LSP ranges and
//! tsserver’s 1-D spans without round-tripping through the process.

use std::cmp;
use std::collections::HashMap;

use lsp_types::{Position as LspPosition, Range as LspRange, Uri};

use crate::types::{
    Position as PluginPosition, Range as PluginRange, TextDocumentContentChangeEvent,
};

/// Captures the current snapshot for every open text document.
#[derive(Default)]
pub struct DocumentStore {
    docs: HashMap<String, DocumentState>,
}

impl DocumentStore {
    /// Inserts or replaces the document snapshot whenever Neovim fires
    /// textDocument/didOpen.
    pub fn open(&mut self, uri: &Uri, text: &str, version: Option<i32>) {
        let state = DocumentState::new(text, version);
        self.docs.insert(uri.to_string(), state);
    }

    /// Applies incremental text changes using the same ordering LSP specifies.
    pub fn apply_changes(
        &mut self,
        uri: &Uri,
        changes: &[TextDocumentContentChangeEvent],
        version: Option<i32>,
    ) {
        let Some(state) = self.docs.get_mut(uri.as_str()) else {
            log::warn!("received didChange for unopened document {}", uri.as_str());
            return;
        };
        for change in changes {
            state.apply_change(change);
        }
        state.version = version;
    }

    /// Drops the cached snapshot as soon as the client closes the buffer.
    pub fn close(&mut self, uri: &Uri) {
        self.docs.remove(uri.as_str());
    }

    /// Converts a visible LSP range into a tsserver-style text span measured in
    /// UTF-16 code units. Returns `None` when the document has not been opened
    /// yet.
    pub fn span_for_range(&self, uri: &Uri, range: &LspRange) -> Option<TextSpan> {
        self.docs.get(uri.as_str()).map(|doc| doc.text_span(range))
    }
}

/// Represents a tsserver text span using UTF-16 offsets.
#[derive(Debug, Clone, Copy)]
pub struct TextSpan {
    pub start: u32,
    pub length: u32,
}

impl TextSpan {
    pub fn covering_length(len: u32) -> Self {
        Self {
            start: 0,
            length: len,
        }
    }
}

struct DocumentState {
    text: String,
    line_metrics: Vec<LineMetrics>,
    total_utf16: u32,
    version: Option<i32>,
}

impl DocumentState {
    fn new(text: &str, version: Option<i32>) -> Self {
        let mut state = Self {
            text: text.to_string(),
            line_metrics: Vec::new(),
            total_utf16: 0,
            version,
        };
        state.recompute_metrics();
        state
    }

    fn apply_change(&mut self, change: &TextDocumentContentChangeEvent) {
        if let Some(range) = &change.range {
            let lsp_range = convert_range(range);
            let start = self.byte_index(&lsp_range.start);
            let end = self.byte_index(&lsp_range.end);
            if start > end || end > self.text.len() {
                log::warn!(
                    "inlay hint document store received out-of-bounds change ({start}-{end} vs len {})",
                    self.text.len()
                );
                return;
            }
            self.text.replace_range(start..end, &change.text);
        } else {
            self.text = change.text.clone();
        }
        self.recompute_metrics();
    }

    fn text_span(&self, range: &LspRange) -> TextSpan {
        let start = self.utf16_offset(&range.start);
        let end = self.utf16_offset(&range.end);
        if end >= start {
            TextSpan {
                start,
                length: end - start,
            }
        } else {
            TextSpan {
                start: end,
                length: start - end,
            }
        }
    }

    fn utf16_offset(&self, position: &LspPosition) -> u32 {
        let line_idx = self.clamp_line_idx(position.line);
        let line = &self.line_metrics[line_idx];
        let column = cmp::min(position.character, line.content_utf16);
        line.start_utf16 + column
    }

    fn byte_index(&self, position: &LspPosition) -> usize {
        let line_idx = self.clamp_line_idx(position.line);
        let line = &self.line_metrics[line_idx];
        let mut byte_index = line.start_byte;
        let mut remaining = cmp::min(position.character, line.content_utf16);
        let line_text = &self.text[line.start_byte..line.start_byte + line.content_bytes];
        for ch in line_text.chars() {
            if remaining == 0 {
                break;
            }
            let units = ch.len_utf16() as u32;
            if remaining < units {
                break;
            }
            remaining -= units;
            byte_index += ch.len_utf8();
        }
        byte_index
    }

    fn clamp_line_idx(&self, line: u32) -> usize {
        if self.line_metrics.is_empty() {
            return 0;
        }
        cmp::min(line as usize, self.line_metrics.len() - 1)
    }

    fn recompute_metrics(&mut self) {
        let mut metrics = Vec::new();
        let mut cursor = 0;
        let mut utf16_offset = 0u32;
        let bytes = self.text.as_bytes();

        while cursor < bytes.len() {
            let line_start = cursor;
            while cursor < bytes.len() && bytes[cursor] != b'\n' && bytes[cursor] != b'\r' {
                cursor += 1;
            }
            let content_end = cursor;
            let content = &self.text[line_start..content_end];
            let content_utf16 = content.encode_utf16().count() as u32;

            let mut newline_utf16 = 0u32;
            if cursor < bytes.len() {
                match bytes[cursor] {
                    b'\r' => {
                        newline_utf16 += 1;
                        cursor += 1;
                        if cursor < bytes.len() && bytes[cursor] == b'\n' {
                            newline_utf16 += 1;
                            cursor += 1;
                        }
                    }
                    b'\n' => {
                        newline_utf16 += 1;
                        cursor += 1;
                    }
                    _ => {}
                }
            }

            metrics.push(LineMetrics {
                start_byte: line_start,
                start_utf16: utf16_offset,
                content_bytes: content_end - line_start,
                content_utf16,
            });
            utf16_offset = utf16_offset.saturating_add(content_utf16 + newline_utf16);
        }

        if metrics.is_empty() {
            metrics.push(LineMetrics::empty());
        } else if self.text.ends_with('\n') || self.text.ends_with('\r') {
            metrics.push(LineMetrics {
                start_byte: self.text.len(),
                start_utf16: utf16_offset,
                content_bytes: 0,
                content_utf16: 0,
            });
        }

        self.line_metrics = metrics;
        self.total_utf16 = utf16_offset;
    }
}

#[derive(Debug, Clone)]
struct LineMetrics {
    start_byte: usize,
    start_utf16: u32,
    content_bytes: usize,
    content_utf16: u32,
}

impl LineMetrics {
    fn empty() -> Self {
        Self {
            start_byte: 0,
            start_utf16: 0,
            content_bytes: 0,
            content_utf16: 0,
        }
    }
}

fn convert_range(range: &PluginRange) -> LspRange {
    LspRange {
        start: convert_position(&range.start),
        end: convert_position(&range.end),
    }
}

fn convert_position(position: &PluginPosition) -> LspPosition {
    LspPosition {
        line: position.line,
        character: position.character,
    }
}
