# Multi-Project Workspace Support

> ⚠ Experimental — may change without notice.

codescout can manage multiple related projects from a single server instance.
This is useful for monorepos or closely related repositories where you want
cross-project navigation without running separate MCP servers.

## Registering projects

Projects are registered in `.codescout/workspace.toml` under a `[[project]]` table:

```toml
[[project]]
id = "backend"
root = "services/backend"

[[project]]
id = "frontend"
root = "apps/frontend"
```

Each entry requires `id` (unique name) and `root` (path relative to the workspace root).
The `languages` and `depends_on` fields are optional: `languages` restricts which LSP
servers are started for the project; `depends_on` lists project IDs whose symbols are
visible during cross-project navigation.

Each project gets its own LSP servers, memory store, and semantic index.

## Using project scope

Most tools accept a `project` parameter to scope the operation:

```
find_symbol("MyStruct", project: "backend")
semantic_search("authentication flow", project: "frontend")
memory(action: "read", project: "backend", topic: "architecture")
```

Omitting `project` uses the workspace-level context.

## Onboarding

Run onboarding once after registering projects:

```
Run codescout onboarding
```

codescout generates a per-project Navigation Strategy section in the system prompt so the
agent knows which files and entry points belong to each project. It also generates
cross-project semantic search scope guidance.

## Cross-project semantic search

The system prompt includes guidance on which `scope=` values to use for semantic search
across projects, so the agent does not need to guess project boundaries.
