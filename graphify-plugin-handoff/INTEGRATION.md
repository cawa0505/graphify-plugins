# Integration — Code Relay with Opendoc-mcp and Graphify

## 1. Summary

This document describes **optional coordination strategies** between Code Relay (the embedded `graphify-plugin-handoff` crate, exposed as relay tools by GraphifyMCP) and two existing MCP servers:

- **`opendoc-mcp`**: Document RAG and knowledge retrieval over specs, research PDFs, and web sources.
- **`graphify`**: Structural code graph traversals (callers, dependencies, component maps).

**Core philosophy**: Code Relay stays focused on **session state handover** (`relay.json`, `RESUME.md`, `next_step.md`). Context enrichment is **optional** and **explicit**, not automatic or default. The slim skill (`SKILL.md`) is the sole client surface; agents can interleave other MCP tools manually when richer search or code awareness is needed. Cross-plugin alignment (including `graphify-plugin-opendoc`) is keyed by the `workspace_key` injected by Graphify (graphify-core v1 contract).

## 2. Use cases

| Scenario | How it works | When to use |
|----------|--------------|-------------|
| **A. Document augmentation** | Agent runs `opendoc-mcp.search` over `specs/*.md` or uploaded research PDFs; inline results into `volatile_state` via `relaySave`. | User is refining a feature and needs to reference design docs, spec intents, or external research that lives in the doc store. |
| **B. Code dependency mapping** | Agent invokes `graphify.query_graph` (e.g., callers of `relaySave`) and captures key insights into `volatile_state` or `next_session_starter`. | The user needs to understand impact of refactoring, check test coverage, or map handoffs. |
| **C. Combined search** | First `opendoc-mcp.search` for specs, then `graphify` for functions mentioned in those specs, finally save a consolidated handoff. | Complex tasks requiring both documentation context and structural code knowledge (e.g., “implement X based on spec Y and refactor related callers”). |
| **D. Research persistence** | Agent uploads PDFs/screencaps, uses `opendoc-mcp` to extract and index them, and saves the extracted intent into `specs/research.md` via `relayAdd` (or manual file edit then `relaySave`). | When the user provides raw data (government PDFs, terminal output, etc.) and expects a structured, searchable artifact. |

## 3. Integration patterns

### 3.1 Session start enrichment

**Command**: `!relayResume` (as usual) + **optional** call to `opendoc-mcp.search` and/or `graphify` before `!relaySave`.

**Sequence**:

```
!relayResume         ← initial handover from previous session
opendoc-mcp.search --query "spec intent for auth"   ← optional context enrichment
<<agent inlines spec intent into volatile_state>>
!relaySave --volatile-state "$(cat merged_context)"   ← save with enriched context
```

**Result**: The new session inherits both state and up-to-date domain knowledge.

### 3.2 Research workflow (user pastes PDFs, terminal output)

**Flow**:

1. **Paste raw data** (agent saves to `docs/research/...` via `write` tool).
2. **Index to opendoc**:
   ```bash
   # From the agent's perspective: call opendoc-mcp.indexer with the saved file path
   ```
3. **Search for extracted intent**:
   ```bash
   opendoc-mcp.search --query "auth requirements"
   ```
4. **Save structured artifact**:
   ```bash
   # Agent writes a spec from search results to `specs/auth-requirements.md`
   write specs/auth-requirements.md "# Auth requirements\n..."
   !relayAdd specs/auth-requirements.md
   !relaySave
   ```

**Result**: Research becomes a searchable spec and feeds into the next session's resume.

### 3.3 Code graph traversal

**Command**: `!relayResume` → **optional** `graphify.query_graph` → **optional** `!relaySave` with code graph insights.

**Example** (find all callers of `relaySave`):

```
!relayResume
# Agent queries graphify for callers of relaySave
# and captures main entry points into next_session_starter
!relaySave --next-session-starter "Callers: loginHandler, userProfileHandler, apiGateway"
```

**Notes**:
- graphify queries respect the current project root (the Graphify session's workspace root); agent may need to use `graphify` from the correct repo directory.
- Agents may store graph results in `specs/code-graph.md` and `relayAdd` it for later sessions.

## 4. Implementation notes

### 4.1 No filesystem dependencies between systems

- Code Relay operates under the **workspace root** injected by Graphify's `WorkspaceContext`. It never touches the doc store; `opendoc-mcp` never edits `relay.json`.
- Coordination is purely through **text** (the agent's output, `volatile_state`, `next_session_starter`). No shared state.

### 4.2 Tool ownership

| System | Owns | Client can read | Client can write |
|--------|------|-----------------|------------------|
| Code Relay (via GraphifyMCP) | `relay.json`, `RESUME.md`, `next_step.md`, `specs/*.md` | N/A (read via `relayStatus`/`relayResume`) | via its own tools (`relaySave`, `relayAdd`, etc.) |
| `opendoc-mcp` | Indexed documents, search indices | Yes (search results) | Yes (via its own tools, e.g., `index` if supported) |
| `graphify` | Code graph data (not exposed as files) | Yes (query tools) | N/A (no write tools) |

### 4.3 Error handling and isolation

- If `opendoc-mcp` or `graphify` are unavailable, the relay skill commands remain functional. The agent can decide to skip enrichment.
- Failures in enrichment tools are captured in the agent’s response; `!relaySave` proceeds normally with the original `volatile_state`.

### 4.4 Privacy and data residency

- **opendoc-mcp** only indexes documents uploaded by the user; no internal network or hostnames appear in uploaded content.
- **graphify** only reads the project’s source tree; no credentials or private configs are included.
- Code Relay (`relay.json`) never includes external URLs or private service endpoints unless the user explicitly adds them.

## 5. How to enable integration for a user

1. **Ensure Graphify embeds this plugin and GraphifyMCP is running** (relay tools registered automatically). `opendoc-mcp` and `graphify` are independent MCP servers registered per the user's MCP client configuration:

   ```json
   "mcp": {
     "stdio": [
       {
         "command": "opendoc-mcp",
         "args": ["--data-dir", "/tmp/opendoc"],
         "cwd": "${workspaceFolder}"
       },
       {
         "command": "graphify",
         "args": [],
         "cwd": "${workspaceFolder}"
       }
     ]
   }
   ```

2. **Documentation**: Add a short note in the project README or a top-level `INTEGRATION.md` (you already have this file) so users understand they can invoke the other servers for richer context.

3. **Optional skill enhancement**: The slim skill can be extended with helper shortcuts, but keep it thin; most integration logic belongs in the agent’s instructions or custom tooling.

## 6. Future extension points

| Area | Current state | Possible extension |
|------|---------------|--------------------|
| **Automatic enrichment** | Optional (agent decides) | Auto-ping `opendoc` for specs on every `relayResume` (configurable via skill params) |
| **Graph-based handoff** | Manual query + inline | On `relaySave`, automatically append `graphify.trace_path(<repo_name>)` insights to `next_session_starter` (configurable) |
| **Spec ↔ doc sync** | Manual `opendoc.index` + `relayAdd` | Watch `specs/*.md` changes and auto-index into opendoc for cross-search |

For now, keep it simple; each extension can be added as a separate agent capability or CLI helper, not baked into the core skill.

## 7. Checklist for integration

- [ ] Graphify embeds `graphify-plugin-handoff`; GraphifyMCP exposes the `relay*` tools.
- [ ] Both `opendoc-mcp` and `graphify` are registered in the user's MCP client configuration.
- [ ] The agent is instructed to use `opendoc-mcp.search` / `opendoc-mcp.read` for spec intent and research.
- [ ] The agent is instructed to use `graphify.query_graph` for code dependency mapping.
- [ ] Example workflows (see above) are documented in `INTEGRATION.md`.
- [ ] The skill (`SKILL.md`) notes optional nature and provides one-line commands for enrichment.
- [ ] Privacy constraints (no internal hostnames, no private config) are respected by all systems.
