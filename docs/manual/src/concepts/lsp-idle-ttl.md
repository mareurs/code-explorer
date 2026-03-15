# LSP Idle TTL Eviction

codescout starts LSP servers on demand and keeps them running for fast symbol lookups.
A server that has been idle beyond its timeout is shut down automatically to reclaim memory.

## Default timeouts

| Language | Idle TTL |
|---|---|
| Kotlin | 2 hours |
| All others | 30 minutes |

Kotlin gets a longer TTL because its LSP server has a long startup time — evicting it
aggressively would cause noticeable latency on the next query.

## Behaviour

When an LSP server's idle TTL expires:

1. codescout sends a `shutdown` request and `exit` notification to the server.
2. The server process is removed from the pool.
3. On the next symbol request for that language, a new server is started automatically.

There is no user-visible interruption — the eviction and restart are transparent.

## Configuration

TTL eviction is not yet configurable via `project.toml`. This is planned for a future release.
