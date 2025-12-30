# ts-bridge

[![](https://github.com/user-attachments/assets/7ea6bfbb-4031-44c2-9113-7248c0f1addf)](https://github.com/user-attachments/assets/7ea6bfbb-4031-44c2-9113-7248c0f1addf)

`ts-bridge` is a standalone TypeScript language-server shim written in Rust. In
this context _standalone_ means the Neovim-facing bits ship as a single Rust
binary—_not_ that TypeScript itself has been rewritten. The binary still launches
the official `tsserver` that ships with TypeScript and simply orchestrates the
LSP ↔ TypeScript Server conversations.

`ts-bridge` sits between Neovim's built-in LSP client and `tsserver`, translating
LSP requests into the TypeScript server protocol (and vice‑versa) while offering
a clear, modular architecture (`config`, `provider`, `process`, `protocol`,
etc.) that mirrors how modern JS/TS tooling pipelines are organized.

> **What “standalone” does _not_ mean:** This project does not replace `tsc` or
> `tsserver`. You still need a standard TypeScript installation, and all
> type-checking/completions semantics come from Microsoft's compiler. What you
> gain is a single Rust binary that handles the Neovim side (startup,
> diagnostics/logging, worker orchestration) without additional Lua or Node glue.

## Prerequisites

- Rust toolchain 1.80+ (Rust 2024 edition) to build the binary via Cargo.
- Node.js 18+ with a matching TypeScript/`tsserver` installation discoverable
  via your workspace (local `node_modules` preferred, but global/npm/Nix paths
  are fine too). `ts-bridge` delegates all language intelligence to this
  `tsserver`; it only provides the Rust shim and orchestrator.
- Neovim 0.11+ so the built-in LSP client matches the capabilities advertised
  by `ts-bridge` (semantic tokens, inlay hints, etc.).

## Building

```bash
cargo build --release
```

The resulting binary (`target/release/ts-bridge`) can be pointed to from your
Neovim `lspconfig` setup.

## Downloading prebuilt binaries

Get the latest release artifact from the [GitHub Releases](https://github.com/chojs23/ts-bridge/releases) page.

## LSP Feature Progress

- [x] `initialize`/`initialized` handshake & server capabilities
- [x] `textDocument/didOpen` / `didChange` / `didClose` (`updateOpen` bridging)
- [x] Diagnostics pipeline (`geterr`, semantic/syntax/suggestion batching)
- [x] `textDocument/hover` (`quickinfo`)
- [x] `textDocument/definition` (`definitionAndBoundSpan`)
- [x] `textDocument/typeDefinition` (`typeDefinition`)
- [x] `textDocument/references` (`references`)
- [x] `textDocument/completion` (+ `completionItem/resolve`)
- [x] `textDocument/signatureHelp` (`signatureHelp`)
- [x] `textDocument/publishDiagnostics` streaming
- [x] `workspace/didChangeConfiguration`
- [x] `textDocument/documentHighlight`
- [x] `textDocument/codeAction` / `codeAction/resolve` (quick fixes, organize imports; refactors pending)
- [x] `textDocument/rename` / `workspace/applyEdit` (prepare + execute)
- [x] `textDocument/formatting` / on-type formatting
- [x] `textDocument/implementation`
- [x] `workspace/symbol` / `textDocument/documentSymbol`
- [x] Semantic tokens
- [x] Inlay hints
- [ ] Code lens
- [ ] Custom commands / user APIs (organize imports, fix missing imports, etc.)
- [ ] Dual-process (semantic diagnostics server) feature gating _(experimental)_

## Configuration

`ts-bridge` works out of the box, but here’s a minimal Neovim `lspconfig`
snippet that wires the server up with all default options spelled out so you can
override only what you need later:

```lua
local lspconfig = require("lspconfig")

lspconfig.ts_bridge.setup({
  cmd = { "/path/to/ts-bridge" },
  init_options = {
    ["ts-bridge"] = {
      separate_diagnostic_server = true,      -- launch syntax + semantic tsserver
      publish_diagnostic_on = "insert_leave",
      enable_inlay_hints = true,
      tsserver = {
        locale = nil,
        log_directory = nil,
        log_verbosity = nil,
        max_old_space_size = nil,
        global_plugins = {},
        plugin_probe_dirs = {},
        extra_args = {},
      },
    },
  },
})
```

Because `ts-bridge` delays spawning `tsserver` until the first routed request,
these defaults (or any overrides you make) apply to both syntax and semantic
processes before they boot. Restart your LSP client after changing the snippet
so a fresh tsserver picks up the new arguments.

## Contributing

Every contributions are welcome! Feel free to open issues or submit pull
requests.
