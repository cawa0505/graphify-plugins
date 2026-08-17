# Handoff Plugin: Memory-Integrated Use Cases

## Status

This document describes the intended application architecture. It is a design
concept, not a claim that every capability is currently implemented.

## Core Positioning

`graphify-plugin-handoff` transfers structured development context between
sessions, agents, and human work periods.

A handoff should combine:

1. Focused Graphify graph context
2. `.toon` subgraph data
3. Pinned symbols
4. Task state
5. Decisions and unresolved questions
6. Reconstructable references to memory
7. Plugin-owned handoff records

The snapshot must remain useful even when semantic memory or Qdrant is
temporarily unavailable.

## Intended Handoff Flow

```text
current task state
  + focused graph subgraph
  + pinned symbols
  + decisions
  + reconstructable memory references
  → HandoffSnapshot
  → next session or agent
  → structural context restoration
  → optional memory rehydration
Use Case 1: Cross-Session Context Hydration
Situation
A session is approaching its context limit during a large refactor or
investigation.
Intended Flow
1. Identify the current workspace using workspace_key.
2. Capture the focused symbols and relevant graph neighborhood.
3. Serialize the structural context as .toon.
4. Store task state, decisions, blockers, and next steps.
5. Store reconstructable memory references instead of Qdrant point IDs.
6. Export a versioned HandoffSnapshot.
7. Restore the snapshot in a new session.
Expected Result
The new session can recover:
- the active workspace
- the task objective
- focused symbols
- relevant graph relationships
- previous decisions
- unresolved questions
- optional semantic context
The snapshot must not claim zero information loss. Any unavailable or expired
memory reference must be explicitly reported.
Use Case 2: Multi-Agent Workflow
Situation
A planning agent prepares work for a coding agent, which then prepares work for
a review agent.
Intended Flow
Planning Agent
  → creates HandoffSnapshot
  → Coding Agent restores snapshot
  → implements within focused context
  → Review Agent restores updated snapshot
  → validates graph impact and task intent
The snapshot should carry structured state rather than a large Markdown
transcript.
Recommended fields include:
- task objective
- acceptance criteria
- focused node IDs
- source file references
- pinned symbols
- architectural decisions
- unresolved decisions
- completed work
- remaining work
- verification commands
- memory query references
Use Case 3: Human Work-State Persistence
Situation
A developer stops work at the end of the day and wants to continue later
without repeating repository exploration.
Intended Flow
1. Capture the current task and progress.
2. Save focused .toon context.
3. Save pinned symbols and source references.
4. Save decisions and known risks.
5. Save reconstructable semantic-memory queries.
6. Restore the snapshot when work resumes.
The snapshot should support manual review and editing. It must not make the
developer dependent on a live Qdrant service for basic restoration.
Use Case 4: Memory Rehydration
Situation
A restored snapshot contains references to previous semantic context.
Required Behavior
1. Attempt to resolve each reference within the same workspace_key.
2. Apply bounded query limits.
3. Verify that referenced source files and node IDs still exist.
4. Mark unavailable, stale, or changed references.
5. Preserve the structural .toon context regardless of memory availability.
Memory rehydration is best-effort enrichment. Structural context is the durable
fallback.
Reconstructable Memory References
HandoffSnapshot must not rely on Qdrant point IDs as its only identifier.
A reference should contain enough information to rebuild a query, for example:
workspace_key
node_ids
source_files
query_text
record_kind
created_at
The exact schema remains versioned and must be validated by the handoff plugin.
Point IDs may be recorded as diagnostics, but they are not the public identity
of a handoff memory reference.
Memory Ownership
Graphify Core Memory
Core memory provides:
- read-only semantic queries
- workspace-scoped results
- bounded result limits
- storage-independent result types
The handoff plugin may query it but may not write to it.
Handoff Domain Memory
Handoff domain memory owns:
- snapshot identity
- task state
- pinned symbols
- decisions
- unresolved questions
- progress and verification state
- snapshot history
- session and agent metadata
Records should use a versioned common envelope:
format_version
workspace_key
plugin_id
record_id
record_kind
created_at
source_refs
payload
Handoff records should use an isolated plugin collection or storage namespace
managed by the Graphify memory service.
Failure Handling
If core semantic memory is unavailable:
- restore the .toon graph context
- restore task state and pinned symbols
- mark semantic references as unavailable
- do not return an empty successful result that hides the outage
If source files or symbols have changed:
- mark affected references as stale
- preserve the original references for auditability
- require a fresh graph query before treating the restored context as current
If the handoff domain store is unavailable, the plugin should fail explicitly
rather than claiming that a snapshot was persisted.
.toon Integration
Plugin-specific handoff data must be stored in the reserved plugin_data
container.
Example:
metadata:
  format_version: "1"
  workspace_key: "..."
  plugin_data:
    handoff:
      snapshot_id: "..."
      pinned_symbols: ["..."]
      task_status: "in_progress"
      memory_queries:
        - query_text: "..."
          node_ids: ["..."]
The handoff plugin must not add arbitrary top-level fields to the core .toon
schema.
