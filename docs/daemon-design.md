# Daemon Mode Design

## Goal
Enable a long-lived `ts-bridge` process that listens once (TCP or stdio) and
accepts multiple LSP clients, while reusing warm `tsserver` instances keyed by
project root. Add a control channel to restart a project’s `tsserver` without
dropping LSP clients.

## Non-goals
- Replace the existing stdio single-client mode.
- Introduce a new protocol for LSP clients; Neovim should connect using normal
  LSP JSON-RPC over a socket.
- Persist caches across daemon restarts.

## Current Architecture Snapshot
- `src/server.rs` owns the LSP loop and uses `Service` for routing.
- `Service` owns config, provider, and `tsserver` process lifecycle.
- `DocumentStore`, diagnostics state, and inlay caches are per-connection.

## Proposed High-Level Architecture
- Add a `daemon` subcommand: `ts-bridge daemon --listen 127.0.0.1:0`.
- Introduce a `Daemon` coordinator that:
  - accepts connections,
  - creates a `Session` per client,
  - shares a `ProjectRegistry` across sessions.
- Split shared vs per-client state:
  - `ProjectService` (shared): `Config`, `Provider`, `tsserver` processes.
  - `Session` (per client): `DocumentStore`, inlay caches, diagnostics queue,
    progress tokens, and LSP connection I/O.

## Project Registry (tsserver Cache)
- Keyed by normalized project root.
- Holds `ProjectService` and last-used timestamp.
- Eviction: configurable max size and idle TTL (default 30 minutes); on
  eviction, stop `tsserver` cleanly.
- Concurrency: `Arc<Mutex<HashMap<..>>>` or `Arc<RwLock<..>>` guarding the
  registry; per-project locking for `tsserver` command dispatch.

## Connection & Transport
- Abstract the transport so the existing `main_loop` can run on stdio or a
  socket:
  - `Transport` trait with `read_message`/`write_message`.
  - `StdioTransport` and `TcpTransport` implementations.
- Each accepted socket runs the same initialization handshake and LSP loop,
  with a `Session` bound to the `ProjectService`.

## Control Channel
- Prefer an LSP-visible control API to keep Neovim integration simple:
  - `workspace/executeCommand` new command: `TSBRestartProject`.
  - Payload: `{ rootUri: string, kind?: "syntax" | "semantic" | "both" }`.
- Optional JSON-RPC notification for internal tools:
  - Method: `ts-bridge/control` with `{ action: "restart", rootUri }`.
- Restart semantics: drain in-flight requests, stop processes, start fresh, and
  notify clients with a progress message.

## Multi-Client Semantics
- Requests are routed per session; responses are sent to the originating client.
- Diagnostics fan out to all sessions that have the file open; keep per-session
  publish state to avoid cross-client noise.
- Config conflicts: first session for a project establishes `ProjectService`
  settings; mismatched later settings log a warning and keep the existing config.

## Failure & Recovery
- If a `tsserver` crashes, restart within the project service and notify all
  sessions (`window/showMessage` warning).
- If a session disconnects, its `DocumentStore` and inlay cache are dropped
  without affecting the shared `tsserver`.

## CLI & Configuration
- New CLI entrypoint: `ts-bridge daemon [--listen HOST:PORT] [--socket PATH] [--idle-ttl SECONDS|off]`.
- Default bind to `127.0.0.1` only; explicitly opt-in for remote binding.
- Idle TTL defaults to 30 minutes; set `--idle-ttl off` or
  `TS_BRIDGE_DAEMON_IDLE_TTL=off` to disable eviction.
- Future config flags for cache size and control API enablement.

## Implementation Steps (Draft)
1. Introduce transport abstraction and refactor `main_loop` to accept it.
2. Create `ProjectService` and `ProjectRegistry` (shared state).
3. Add `daemon` command with TCP listener and session threads.
4. Implement `TSBRestartProject` control command.
5. Add metrics/logging for active sessions and project cache size.
