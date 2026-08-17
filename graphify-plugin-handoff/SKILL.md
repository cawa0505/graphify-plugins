---
name: graphify-relay
description: Code Relay state handoff via graphify (CLI) or graphify_relay* MCP tools. Use when initializing, saving, closing, switching, resuming, or querying cross-session handoff state (relay.json / .relay/relay.toon), or when a session needs to read the active baton / repo status before continuing work.
---

# Code Relay — Skill (graphify-relay)

## 1. Summary — dual-track principle

Code Relay hands session state across sessions and repos via a single state contract:

- **State**: `relay.json` (schema 1.0.0) + `.relay/relay.toon` (TOON serialization) at the relay root
- **Two channels**, same operations:
  - **MCP tools** (`graphify_relay*`) — the efficiency layer, when registered
  - **CLI** (`graphify handoff ...`) — the resilience layer, always available
- **Direct file reads** — zero-dependency introspection (read-only)

The CLI is the canonical fallback: if MCP is missing, erroring, or timing out, every relay operation still works through `graphify`. Protocol details live in `PROTOCOL.md`; this skill maps **intent → command**. If this skill and the CLI disagree, **trust the CLI and `PROTOCOL.md`**.

## 2. Channel decision table

| Situation | Use |
|---|---|
| `graphify_relay*` tools registered | MCP tools (fastest) |
| MCP error / timeout / not registered | CLI fallback: `graphify handoff ...` |
| Neither available | Direct file read (section 5) |

## 3. Quick start

```bash
which graphify          # CLI present? (installed via cargo install or GraphifyRust build)
graphify handoff status # show current relay root + active baton
```

If `graphify` is missing, build GraphifyRust and add it to `PATH`. To install this skill into the local agent ecosystem:

```bash
graphify handoff skill install            # detect + install for all found agents
graphify handoff skill install --agent opencode --scope user
graphify handoff skill uninstall
```

## 4. Operations

Each operation lists the MCP tool, the CLI equivalent, the frozen return shape, and failure semantics.

### 4.1 Init

- **MCP**: `relayInit` (`project_context`, `kind?`)
- **CLI**: `graphify handoff init <project> [--kind backend|frontend|infra]`

**Return**:

```
Initialized relay at <cwd>/relay.json
- specs/ and .code-relay/ created
- .gitignore updated (relay.json, RESUME.md, next_step.md)
- run relaySave to register the current repo
```

**Notes**:
- If `relay.json` already exists in any parent directory, the command fails with the legacy error text.
- `.gitignore` updates apply **only** when cwd is a git repo; the line still shows when skipped.

### 4.2 Save

- **MCP**: `relaySave` (`repo?`, `role?`, `active_phase?`, `volatile_state?`, `confidence?`, `next_session_starter?`, `debt_tag?`, `kind?`)
- **CLI**: `graphify handoff save [--repo R] [--role R] [--phase P] [--volatile V] [--conf 0-5] [--next N] [--debt D] [--kind K]`

**Return**:

```
Saved state for "<repo>".
Active baton: <active_baton>

<rendered resume>
```

**Notes**:
- `debt_tag`/`--debt` accepts a comma-separated string; each item becomes a line in the resume.
- `confidence`/`--conf` is clamped to 1–5 and rounded.

### 4.3 Close

- **MCP**: `relayClose` (`repo?`, `next_session_starter?`)
- **CLI**: `graphify handoff close [--repo R] [--next N]`

**Return** (concatenated report lines):

```
Closing ritual for "<repo>".
Consistency: OK|ISSUES
  - <issue>            (per issue, only when ISSUES)
Spec sync: <a:added,b:modified> | no changes

<rendered next_step.md>

committed: <hash> | nothing to commit (files ignored or missing) | not a git repo — skipped commit
```

**Notes**:
- A spec that includes `## REMOVED Requirements` with non-whitespace content after triggers a consistency issue.
- Non-git repos report `committed:` accordingly.
- A `Snapshot: skipped — <err>` line may be appended (best-effort registry snapshot); close never fails because of it.

### 4.4 Switch

- **MCP**: `relaySwitch` (`repo`, `kind?`)
- **CLI**: `graphify handoff switch <repo> [--kind K]`

**Return**:

```
Baton passed to "<repo>".

<rendered resume>
```

**Failure cases**:
- `No relay.json found. Run relayInit first.` (root not discovered)
- `repo "<repo>" not registered. Run relaySave in that repo first.` (repo absent)

### 4.5 Resume

- **MCP**: `relayResume` (`repo?`, `kind?`)
- **CLI**: `graphify handoff resume [--repo R] [--kind K]`

**Return**: rendered resume (same text as legacy plugin). Also writes `RESUME.md` at the relay root.

**Failure cases**:
- `No active baton set and no repo given. Run relaySwitch <repo> first.`
- `repo "<target>" not registered.` (repo specified but absent)

### 4.6 Status

- **MCP**: `relayStatus`
- **CLI**: `graphify handoff status`

**Return** (multiline status report):

```
Relay root: <root>
Project: <project_context | "(unset)">
Active baton: <active_baton | "(none)">
Repos (<n>):
  - <name> [<active_phase | "?">] conf=<confidence_score>[ · <n> handoff(s)][ → <next_session_starter, first 60 chars>]
Specs: <names, comma-joined | "(none)">
Drift: <spec (added), ...> | "(none)"
Updated: <updated_at>
```

### 4.7 Add

- **MCP**: `relayAdd` (`file`, `repo?`)
- **CLI**: `graphify handoff add <file> [--repo R]`

**Return**:

```
Added handoff "<basename(file)>" to "<repo>".
Parsed <n> line(s) into open_threads.
Total open threads: <total>
```

**Note**: `<file>` resolves relative to the agent's current working directory, not the relay root.

## 5. Zero-dependency introspection (read state without tools)

If neither MCP nor the CLI is available, read the state directly — it is plain files:

- `relay.json` — JSON: `repos` (map of name → `{path, role, active_phase, volatile_state, confidence, next_session_starter, debt_tag, updated_at, kind}`), `active_baton`, `project_context`, `specs`, `created_at`, `updated_at`.
- `.relay/relay.toon` — TOON serialization of the same state; MUST carry `format_version: "1.0.0"` and `workspace_key` metadata.

**Read-only rule**: direct file access is for introspection only. **Never mutate** `relay.json` / `.relay/` by hand — writes go through the CLI or MCP so locking, atomic writes, and the TOON mirror stay consistent.

## 6. State discovery & coordination

- **Root resolution** (see `PROTOCOL.md`) is **cached** at bind time:
  - Named-repo operations resolve purely against the registry (`repos[name].path`).
  - No-repo operations use the root cached at bind time.
  - The only write-type walk is `relayInit`; everything else is in-memory lookup.
- **Fail-fast**: a no-repo operation without a cached root returns `No relay.json found. Run relayInit first.`
- **Ownership**: relay state is owned by the plugin/CLI — the agent only reads files directly and mutates via the channels.

## 7. Verification checklist

Run once against a scratch directory (CLI path):

```bash
mkdir -p /tmp/relay-smoke && cd /tmp/relay-smoke
graphify handoff init "smoke test"
graphify handoff save --phase p1 --conf 4 --next "verify status" --debt "a,b"
graphify handoff status            # shows repo registered, baton set
graphify handoff close --next done # Consistency: OK, committed: <hash>
```

MCP path: the same cycle through `graphify_relayInit` → `relaySave` → `relayStatus` → `relayClose`.

## 8. Installing in other agents

`graphify handoff skill install` detects the local agent ecosystem and installs this skill (self-contained copy of this file, tagged with a managed marker so `uninstall` never removes user-created files):

| Agent | Target |
|---|---|
| opencode | `~/.config/opencode/skills/graphify-relay/SKILL.md` |
| Claude | `~/.claude/skills/graphify-relay/SKILL.md` |
| Cursor | `.cursor/rules/graphify-relay.mdc` (managed copy) |
| Cline | `.clinerules` (managed copy) |
| Project | `.opencode/skills/graphify-relay/SKILL.md` (cwd) |

In-repo (this repository): Claude Code auto-discovers the repo-root `SKILL.md`; opencode loads it via the project or global install above.

## 9. Error handling & resilience

- Tool results are returned as-is; frozen error texts are preserved.
- MCP: a tool error result still becomes the agent response (legacy behavior). If MCP times out or is unregistered, retry the same operation through the CLI.
- CLI errors mirror the MCP error texts; the plugin never panics.
- `graphify` missing → section 5 (direct file reads) still gives you the handoff state.

## 10. Privacy & deployment

- No internal network topologies, private hostnames, credentials, or absolute local paths appear in this skill or the plugin.
- The plugin reads only under the workspace root; there are no global scans.

## 11. References

- Relay tool/CLI contract: `PROTOCOL.md`
- Root discovery and caching semantics: `openspec/changes/rust-mcp-migration/specs/stateful-cache/spec.md`
- TOON packet contract: `sync-toon-packet/spec.md` (graphify-core)
