# graphify-plugin-handoff

[繁體中文 (Traditional Chinese)](README.zh-TW.md)

A Graphify **embedded plugin** for the Code Relay subsystem: cross-session and cross-repository AI agent state handoff, implemented as a native Rust crate that implements the `GraphifyPlugin` trait and integrates directly with Graphify Core.

The concept of Code Relay is inspired by and based on the original [code-relay](https://github.com/yan5xu/code-relay) project. We highly respect the original author's design and initiative. This repository evolves that concept into a stateful, high-performance embedded plugin architecture for the Graphify ecosystem.

## Key Features

- **Embedded, not a separate server**: Ships as a single Rust crate (`lib.rs`) that Graphify Core embeds and loads at startup. No stdio JSON-RPC process, no extra binary to deploy. The `relay*` tools are auto-registered by GraphifyMCP when Graphify starts.
- **Dual-track client surface**: The same 7 relay operations work through MCP tools (`graphify_relay*`, efficiency layer) **and** the `graphify handoff ...` CLI (resilience layer) — if MCP is unavailable, the CLI and direct state files keep the handoff fully usable. A self-installing agent skill (`SKILL.md`) ships with the crate: `graphify handoff skill install` registers it for opencode / Claude / Cursor / Cline.
- **Stateful Memory Caching**: Eliminates the slow walk-up disk search penalty from the legacy Node.js implementation. The workspace root is resolved exactly once during `GraphifyPlugin::bind` (via the injected `WorkspaceContext`) and cached in memory. Subsequent operations complete under 1ms.
- **Hybrid Double-track Memory**:
  - **Short-Term Memory**: Fast, deterministic state handoff using the token-efficient TOON (Token-Oriented Object Notation) format in `.relay/relay.toon`.
  - **Long-Term Memory**: Semantic vector search utilizing Qdrant for RAG-assisted retrieval of historical session decisions.
- **Workspace-aligned with the plugin ecosystem**: Plugins (handoff, review, opendoc, …) are aligned by `workspace_key` injected by Graphify (graphify-core v1 contract) — no per-plugin walk-up, no divergent root discovery.
- **Safe & Atomic Operations**: Implements transactional file-writing using temp-swapping (via `fs2` locks) to prevent state corruption across concurrent sessions.

## Relationship with OpencodeCodeRelayPlugin

This crate is the Rust-native evolution of the legacy Node.js plugin. Backward compatibility (the `npx opencode-code-relay-plugin <command>` flow) is **[待討論 / under discussion]** — see `openspec/changes/rust-mcp-migration/tasks.md`.

## Developer & Verification Commands

```bash
# Build the project
cargo build

# Run quality checks
cargo check
cargo clippy

# Run unit tests
cargo test
```

## Setup

No standalone server configuration is required. Graphify Core depends on this crate, loads it as a plugin, and GraphifyMCP registers the `relay*` tools at startup. Configuration is fully dynamic and relative — no environment-level secrets, no hardcoded paths.

## Architecture Design

See detailed requirements, specifications, and architecture decisions in the `openspec/` directory. The Graphify plugin contract (`GraphifyPlugin` trait, `WorkspaceContext`) is defined in Graphify Core and coordinated with the GraphifyRust project.

## License

MIT
