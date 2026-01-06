# Changelog

All notable changes to this project will be documented in this file. The format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

- Placeholder for upcoming changes.

## [0.2.3] - 2026-01-06

### Added
- Added `--version`/`-V` CLI support so `ts-bridge` prints its crate version without starting the LSP server.
- Introduced an integration test (`tests/version_cli.rs`) that spawns the binary via `assert_cmd` to guard the version flag end-to-end.
- Expanded `DocumentStore` unit coverage (range/span accounting, change application, close semantics) to harden LSP ↔ tsserver text conversions.

### Changed
- Bumped README/agent docs to clarify contributor workflows and emphasize logging incidental bugs in `SESSION.md`.

## [0.2.2] - 2026-01-04

### Added
- Added a `ts-bridge/status` request for inspecting daemon projects, sessions, and tsserver PIDs.
- Added test coverage for tsserver configure argument construction, workspace root selection, and status snapshots.

### Fixed
- Ensure `tsserver` child processes are waited after shutdown to avoid zombie instances.

## [0.2.1] - 2026-01-04

### Added
- Forward `tsserver.preferences` and `tsserver.format_options` through the `configure` request.
- Added a PR CI workflow with `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and a Rust cache.

### Changed
- Ensure `configure` is sent before routing tsserver-bound requests/notifications to keep preferences in sync.
- Expanded README configuration examples for Neovim’s built-in LSP and daemon wait logic.

## [0.2.0] - 2025-12-31

### Added
- Introduced daemon mode with TCP/Unix listeners, shared per-project `tsserver` instances, and a restart control command.
- Added idle TTL eviction for daemon project caches with a 30‑minute default and CLI/env configuration.
- Added install scripts for Linux/macOS (`scripts/install.sh`) and Windows PowerShell (`scripts/install.ps1`).
- Documented daemon mode, auto-start patterns, and install scripts in the README.

### Changed
- Updated README `nvim-lspconfig` examples to use `cmd = { "ts-bridge" }` by default.

### Fixed
- Updated adapter tests to unwrap `AdapterResult` before deserializing responses.

## [0.1.0] - 2025-12-30

- First public release of `ts-bridge`, a standalone shim that translates Neovim LSP traffic to the TypeScript server protocol while mirroring modern tooling layers (`config`, `provider`, `process`, `protocol`).
- Ships full diagnostics bridging (`didOpen`/`didChange` sync, semantic + syntax `geterr` batching) and live streaming of `publishDiagnostics`.
- Implements most day‑to‑day LSP features: hover, definition/typeDefinition/implementation, references, rename + workspace edits, document/workspace symbols, semantic tokens, inlay hints, formatting/on-type formatting, completion with resolve, signature help, code actions (quick fixes, organize imports) and document highlights.
- Provides workspace/didChangeConfiguration propagation, tsserver dual-process bootstrapping, and delayed tsserver spawn so init options apply to syntax + semantic servers.
