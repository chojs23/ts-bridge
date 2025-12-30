# Changelog

All notable changes to this project will be documented in this file. The format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

- Placeholder for upcoming changes.

## [0.1.0] - 2025-12-30

- First public release of `ts-bridge`, a standalone shim that translates Neovim LSP traffic to the TypeScript server protocol while mirroring modern tooling layers (`config`, `provider`, `process`, `protocol`).
- Ships full diagnostics bridging (`didOpen`/`didChange` sync, semantic + syntax `geterr` batching) and live streaming of `publishDiagnostics`.
- Implements most day‑to‑day LSP features: hover, definition/typeDefinition/implementation, references, rename + workspace edits, document/workspace symbols, semantic tokens, inlay hints, formatting/on-type formatting, completion with resolve, signature help, code actions (quick fixes, organize imports) and document highlights.
- Provides workspace/didChangeConfiguration propagation, tsserver dual-process bootstrapping, and delayed tsserver spawn so init options apply to syntax + semantic servers.
