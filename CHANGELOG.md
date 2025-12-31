# Changelog

All notable changes to this project will be documented in this file. The format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

- Placeholder for upcoming changes.

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
