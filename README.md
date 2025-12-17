# ts-bridge

`ts-bridge` is a standalone TypeScript language-server shim written in Rust. It
sits between Neovim's built-in LSP client and `tsserver`, translating LSP
requests into the TypeScript server protocol (and vice‑versa) while offering a
clear, modular architecture (`config`, `provider`, `process`, `protocol`, etc.)
that mirrors how modern JS/TS tooling pipelines are organized.

## Building

```bash
cargo build --release
```

The resulting binary (`target/release/ts-bridge`) can be pointed to from your
Neovim `lspconfig` setup.

## LSP Feature Progress

| Feature                                                                   | Status |
| ------------------------------------------------------------------------- | ------ |
| `initialize`/`initialized` handshake & server capabilities                | ✅     |
| `textDocument/didOpen` / `didChange` / `didClose` (`updateOpen` bridging) | ✅     |
| Diagnostics pipeline (`geterr`, semantic/syntax/suggestion batching)      | ✅     |
| `textDocument/hover` (`quickinfo`)                                        | ✅     |
| `textDocument/definition` (`definitionAndBoundSpan`)                      | ✅     |
| `textDocument/typeDefinition` (`typeDefinition`)                          | ✅     |
| `textDocument/references` (`references`)                                  | ✅     |
| `textDocument/completion` (+ `completionItem/resolve`)                    | ✅     |
| `textDocument/signatureHelp` (`signatureHelp`)                            | ✅     |
| `textDocument/publishDiagnostics` streaming                               | ✅     |
| `workspace/didChangeConfiguration`                                        | ❌     |
| `textDocument/documentHighlight`                                          | ❌     |
| `textDocument/codeAction` / `codeAction/resolve`                          | ❌     |
| `textDocument/rename` / `workspace/applyEdit`                             | ❌     |
| `textDocument/formatting` / on-type formatting                            | ❌     |
| `textDocument/implementation`                                             | ❌     |
| `workspace/symbol` / `textDocument/documentSymbol`                        | ❌     |
| Semantic tokens                                                           | ❌     |
| Inlay hints                                                               | ❌     |
| Code lens                                                                 | ❌     |
| Custom commands / user APIs (organize imports, fix missing imports, etc.) | ❌     |
| Dual-process (semantic diagnostics server) feature gating                 | 🚧     |
| Test harness (port of busted/Plenary suite)                               | ❌     |
