//! =============================================================================
//! Tsserver Process Management
//! =============================================================================
//!
//! Tracks child Node processes, implements the `Content-Length` framed protocol,
//! and exposes cancellation pipes just like the Lua `process.lua`.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde_json::Value;
use tempfile::TempDir;

use crate::provider::TsserverBinary;

/// Represents an owned tsserver instance (syntax or semantic).
pub struct TsserverProcess {
    kind: ServerKind,
    binary: TsserverBinary,
    child: Option<ChildHandles>,
}

impl TsserverProcess {
    pub fn new(kind: ServerKind, binary: TsserverBinary) -> Self {
        Self {
            kind,
            binary,
            child: None,
        }
    }

    /// Spawns the tsserver child process and starts the reader thread.
    pub fn start(&mut self) -> Result<(), ProcessError> {
        if self.child.is_some() {
            return Ok(());
        }

        let mut command = Command::new("node");
        let server_label = match self.kind {
            ServerKind::Syntax => "syntax",
            ServerKind::Semantic => "semantic",
        };
        command.env("TS_LSP_RS_SERVER_KIND", server_label);
        command.arg(&self.binary.executable);
        command.arg("--stdio");
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::inherit());

        let mut child = command.spawn().map_err(ProcessError::Spawn)?;
        let stdout = child.stdout.take().ok_or(ProcessError::MissingStdout)?;
        let stdin = child.stdin.take().ok_or(ProcessError::MissingStdin)?;

        let cancellation_dir = TempDir::new().map_err(ProcessError::CreateCancellationDir)?;
        let (tx, rx) = unbounded();
        let reader_handle = spawn_reader(stdout, tx);

        self.child = Some(ChildHandles {
            child,
            stdin,
            cancellation_dir,
            response_rx: rx,
            reader_handle: Some(reader_handle),
        });

        Ok(())
    }

    /// Sends a JSON payload to tsserver using the Content-Length framing.
    pub fn write(&mut self, payload: &Value) -> Result<(), ProcessError> {
        let child = self.child.as_mut().ok_or(ProcessError::NotStarted)?;
        let serialized = serde_json::to_string(payload).map_err(ProcessError::Serialize)?;
        let header = format!("Content-Length: {}\r\n\r\n", serialized.len());
        child
            .stdin
            .write_all(header.as_bytes())
            .map_err(ProcessError::Write)?;
        child
            .stdin
            .write_all(serialized.as_bytes())
            .map_err(ProcessError::Write)?;
        child.stdin.flush().map_err(ProcessError::Write)?;
        Ok(())
    }

    /// Signals cancellation by touching `seq_{id}` inside the cancellation pipe directory.
    pub fn cancel(&self, seq: u64) -> Result<(), ProcessError> {
        let child = self.child.as_ref().ok_or(ProcessError::NotStarted)?;
        let path = child.cancellation_dir.path().join(format!("seq_{}", seq));
        OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)
            .map(|_| ())
            .map_err(ProcessError::Write)
    }

    pub fn response_rx(&self) -> Option<Receiver<Value>> {
        self.child
            .as_ref()
            .map(|handles| handles.response_rx.clone())
    }
}

impl Drop for TsserverProcess {
    fn drop(&mut self) {
        if let Some(mut handles) = self.child.take() {
            let _ = handles.child.kill();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerKind {
    Syntax,
    Semantic,
}

struct ChildHandles {
    child: Child,
    stdin: ChildStdin,
    cancellation_dir: TempDir,
    response_rx: Receiver<Value>,
    reader_handle: Option<thread::JoinHandle<()>>,
}

impl Drop for ChildHandles {
    fn drop(&mut self) {
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_reader(stdout: ChildStdout, tx: Sender<Value>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_message(&mut reader) {
                Ok(message) => {
                    let _ = tx.send(message);
                }
                Err(ProcessError::Eof) => break,
                Err(_) => continue,
            }
        }
    })
}

fn read_message<T: Read>(reader: &mut BufReader<T>) -> Result<Value, ProcessError> {
    let mut header = String::new();
    loop {
        header.clear();
        let bytes = reader.read_line(&mut header).map_err(ProcessError::Read)?;
        if bytes == 0 {
            return Err(ProcessError::Eof);
        }
        if header == "\r\n" {
            continue;
        }
        if header.to_ascii_lowercase().starts_with("content-length:") {
            let len: usize = header["Content-Length:".len()..]
                .trim()
                .parse()
                .map_err(|_| ProcessError::InvalidHeader)?;
            // consume blank line
            let mut blank = [0; 2];
            reader.read_exact(&mut blank).map_err(ProcessError::Read)?;
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).map_err(ProcessError::Read)?;
            return serde_json::from_slice(&body).map_err(ProcessError::Deserialize);
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ProcessError {
    #[error("process not started")]
    NotStarted,
    #[error("failed to spawn tsserver: {0}")]
    Spawn(std::io::Error),
    #[error("failed to create cancellation directory: {0}")]
    CreateCancellationDir(std::io::Error),
    #[error("tsserver stdout missing (stdio must be piped)")]
    MissingStdout,
    #[error("tsserver stdin missing (stdio must be piped)")]
    MissingStdin,
    #[error("failed to serialize payload: {0}")]
    Serialize(serde_json::Error),
    #[error("failed to write to tsserver stdin: {0}")]
    Write(std::io::Error),
    #[error("failed to parse response json: {0}")]
    Deserialize(serde_json::Error),
    #[error("unexpected EOF while reading tsserver output")]
    Eof,
    #[error("invalid Content-Length header")]
    InvalidHeader,
    #[error("io error while reading tsserver stdout: {0}")]
    Read(std::io::Error),
}
