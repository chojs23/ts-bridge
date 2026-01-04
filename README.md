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

- Node.js 18+ with a matching TypeScript/`tsserver` installation discoverable
  via your workspace (local `node_modules` preferred, but global/npm/Nix paths
  are fine too). `ts-bridge` delegates all language intelligence to this
  `tsserver`; it only provides the Rust shim and orchestrator.
- Neovim 0.11+ so the built-in LSP client matches the capabilities advertised
  by `ts-bridge` (semantic tokens, inlay hints, etc.).

## Building

You need Rust and Cargo installed. Then clone the repo and run:

```bash
cargo build --release
```

The resulting binary (`target/release/ts-bridge`) can be pointed to from your
Neovim LSP configuration (built-in `vim.lsp.config` or `nvim-lspconfig`).

## Downloading prebuilt binaries

Get the latest release artifact from the [GitHub Releases](https://github.com/chojs23/ts-bridge/releases) page.

## Install script (Linux/macOS)

The install script downloads the latest release archive from GitHub and places
`ts-bridge` in `~/.local/bin` (override with `--install-dir`).

```bash
curl -fsSL https://raw.githubusercontent.com/chojs23/ts-bridge/main/scripts/install.sh | bash
```

If you already cloned the repo:

```bash
./scripts/install.sh
```

To install a specific version:

```bash
./scripts/install.sh --version v0.4.0
```

The script requires `curl` or `wget` plus `tar`. Checksum verification uses
`sha256sum` (Linux) or `shasum` (macOS) when available.

GitHub’s `/releases/latest` points to the newest non‑pre‑release tag, so you do
not need to create a separate “latest” tag. Use `--version` to pin a specific
release (including pre‑releases).

## Install script (Windows PowerShell)

The PowerShell script downloads the Windows release archive and installs
`ts-bridge.exe` into `%LOCALAPPDATA%\Programs\ts-bridge\bin` by default.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "Invoke-RestMethod -Uri 'https://raw.githubusercontent.com/chojs23/ts-bridge/main/scripts/install.ps1' | Invoke-Expression"
```

If you already cloned the repo:

```powershell
.\scripts\install.ps1
```

To install a specific version:

```powershell
.\scripts\install.ps1 -Version v0.4.0
```

Pass `-InstallDir` to override the destination or `-NoVerify` to skip checksum
verification.

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

`ts-bridge` works out of the box. For Neovim 0.11+ (built-in LSP config), use:

```lua
vim.lsp.config("ts_bridge", {
  cmd = { "ts-bridge" },
  filetypes = { "typescript", "typescriptreact", "javascript", "javascriptreact" },
  root_markers = { "tsconfig.json", "jsconfig.json", "package.json", ".git" },
  settings = {
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

vim.lsp.enable("ts_bridge")
```

If you're using `nvim-lspconfig`, the equivalent registration is:

```lua
local configs = require("lspconfig.configs")
local util = require("lspconfig.util")

if not configs.ts_bridge then
  configs.ts_bridge = {
    default_config = {
      cmd = { "ts-bridge" },
      filetypes = { "typescript", "typescriptreact", "javascript", "javascriptreact" },
      root_dir = util.root_pattern("tsconfig.json", "jsconfig.json", "package.json", ".git"),
    },
  }
end

local lspconfig = require("lspconfig")

lspconfig.ts_bridge.setup({
  cmd = { "ts-bridge" },
  settings = {
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

If you built locally instead of installing, swap `cmd = { "ts-bridge" }` for the
absolute binary path (for example, `cmd = { "/path/to/ts-bridge" }`).

Because `ts-bridge` delays spawning `tsserver` until the first routed request,
these defaults (or any overrides you make) apply to both syntax and semantic
processes before they boot. Restart your LSP client after changing the snippet
so a fresh tsserver picks up the new arguments.

## Daemon mode

Daemon mode keeps a single `ts-bridge` process alive and reuses warm `tsserver`
instances across LSP clients. It listens on a TCP address or a Unix socket and
accepts normal LSP JSON-RPC connections.

<details>
<summary>How daemon mode works (click to expand)</summary>

At a high level, the daemon accepts many LSP connections and routes each
project's requests through a shared `tsserver` service keyed by project root.

```text
             ┌─────────────────────────────────────────────────┐
             │                 ts-bridge daemon                │
             │                                                 │
LSP client 1 ─┤ session (per client) ─┐                        │
LSP client 2 ─┤ session (per client) ─┼── Project registry ─┐   │
LSP client 3 ─┤ session (per client) ─┘                      │  │
             │                                               │  │
             │               project root A ── tsserver (A)  │  │
             │               project root B ── tsserver (B)  │  │
             └─────────────────────────────────────────────────┘
```

Lifecycle summary:

- A client connects over TCP or a Unix socket and completes the normal LSP
  `initialize` handshake.
- The daemon selects (or creates) a project entry based on the client’s
  workspace root and reuses the warm `tsserver` for that project.
- Each session keeps its own open document state and diagnostics routing, but
  requests and responses go through the shared `tsserver` process.
- Idle project entries (no active sessions) are evicted after the idle TTL and
their `tsserver` processes are shut down.
</details>

### Neovim (auto-start the daemon)

If you prefer not to start the daemon manually, you can spawn it from Neovim and
still connect via `vim.lsp.rpc.connect`. This runs once per session and reuses
the existing daemon if it is already running:

```lua
local function ensure_ts_bridge_daemon()
  if vim.g.ts_bridge_daemon_started then
    return
  end
  vim.g.ts_bridge_daemon_started = true
  vim.fn.jobstart({
    "ts-bridge",
    "daemon",
    "--listen",
    "127.0.0.1:7007", -- choose your port
  }, {
    detach = true,
    env = { RUST_LOG = "info", TS_BRIDGE_DAEMON_IDLE_TTL = "30m" },
  })
end

local function wait_for_daemon(host, port, timeout_ms)
  local addr = string.format("%s:%d", host, port)
  local function is_ready()
    local ok, chan = pcall(vim.fn.sockconnect, "tcp", addr, { rpc = false })
    if not ok then
      return false
    end
    if type(chan) == "number" and chan > 0 then
      vim.fn.chanclose(chan)
      return true
    end
    return false
  end
  return vim.wait(timeout_ms, is_ready, 50)
end

local function daemon_cmd(dispatchers)
  ensure_ts_bridge_daemon()
  -- Built-in LSP has no `on_new_config`, and `before_init` runs after `cmd`, so
  -- start + wait here to avoid a first-attach connection refusal.
  wait_for_daemon("127.0.0.1", 7007, 2000)
  return vim.lsp.rpc.connect("127.0.0.1", 7007)(dispatchers)
end

vim.lsp.config("ts_bridge", {
  cmd = daemon_cmd,
  filetypes = { "typescript", "typescriptreact", "javascript", "javascriptreact" },
  root_markers = { "tsconfig.json", "jsconfig.json", "package.json", ".git" },
})

vim.lsp.enable("ts_bridge")
```

The `cmd` wrapper ensures the daemon is running before the TCP connection is
attempted (since `before_init` runs after the transport is created).

If you're using `nvim-lspconfig`, use:

```lua
require("lspconfig").ts_bridge.setup({
  cmd = vim.lsp.rpc.connect("127.0.0.1", 7007),
  on_new_config = function()
    ensure_ts_bridge_daemon()
  end,
})
```

Note: `cmd_env` does not apply when using `vim.lsp.rpc.connect`, so any daemon
settings and logging must be passed in the `jobstart` environment or CLI args.

### Running the daemon manually

```bash
ts-bridge daemon --listen 127.0.0.1:7007 # choose your port
```

Optional knobs:

- `--socket /path/to/ts-bridge.sock` (Unix only)
- `--idle-ttl 1800` (seconds) or `--idle-ttl 30m` (suffix `s`, `m`, `h`)
- `--idle-ttl off` to disable idle eviction

Environment variable equivalents:

- `TS_BRIDGE_DAEMON=1` to start daemon mode when running `ts-bridge` without args
- `TS_BRIDGE_DAEMON_LISTEN=127.0.0.1:7007`
- `TS_BRIDGE_DAEMON_SOCKET=/path/to/ts-bridge.sock`
- `TS_BRIDGE_DAEMON_IDLE_TTL=30m` (or `off`)

The default idle TTL is 30 minutes; idle projects (no sessions) are evicted and
their `tsserver` processes are shut down once they exceed the TTL.

### Neovim (daemon connection)

When connecting to a running daemon, use `vim.lsp.rpc.connect` and register the custom
server config:

```lua
vim.lsp.config("ts_bridge", {
  cmd = vim.lsp.rpc.connect("127.0.0.1", 7007),  -- match daemon address
  filetypes = { "typescript", "typescriptreact", "javascript", "javascriptreact" },
  root_markers = { "tsconfig.json", "jsconfig.json", "package.json", ".git" },
  settings = {
    ["ts-bridge"] = {
      separate_diagnostic_server = true,
      publish_diagnostic_on = "insert_leave",
      enable_inlay_hints = true,
      tsserver = {
        global_plugins = {},
        plugin_probe_dirs = {},
        extra_args = {},
      },
    },
  },
})

vim.lsp.enable("ts_bridge")
```

If you're using `nvim-lspconfig` instead of the built-in config, use:

```lua
local configs = require("lspconfig.configs")
local util = require("lspconfig.util")

if not configs.ts_bridge then
  configs.ts_bridge = {
    default_config = {
      cmd = vim.lsp.rpc.connect("127.0.0.1", 7007),  -- match daemon address
      filetypes = { "typescript", "typescriptreact", "javascript", "javascriptreact" },
      root_dir = util.root_pattern("tsconfig.json", "jsconfig.json", "package.json", ".git"),
      settings = {
        ["ts-bridge"] = {
          separate_diagnostic_server = true,
          publish_diagnostic_on = "insert_leave",
          enable_inlay_hints = true,
          tsserver = {
            global_plugins = {},
            plugin_probe_dirs = {},
            extra_args = {},
          },
        },
      },
    },
  }
end

require("lspconfig").ts_bridge.setup({})
```

Daemon settings (listen address, idle TTL, etc.) must be configured on the
daemon process itself; they are not part of LSP `settings`.

## Contributing

Every contributions are welcome! Feel free to open issues or submit pull
requests.
