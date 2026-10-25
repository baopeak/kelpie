---
name: auth-param-scoping-traps
description: Kelpie daemon auth rewrites session params in place; non-object params (Null/array) skip the rewrite and serde still deserializes structs from JSON arrays
metadata:
  type: feedback
---

When reviewing any per-role param rewriting in `crates/kelpied/src/auth.rs` (e.g. forcing a session's `path`, stripping fields like `materialize`), check what happens when `params` is missing or not an object: `server.rs` passes missing params as `Value::Null`, and a helper that does `let Some(obj) = params.as_object_mut() else { return Ok(()) }` lets the request through unscoped. Handlers that read `params.get("x")` then see empty values (e.g. `note.list` without path returned every worktree's notes), and `serde_json::from_value::<DerivedStruct>` accepts a JSON array in field order (verified 2026-10-25 with a scratch crate), so stripped fields survive.

**Why:** Found in the phase 8 review (2026-10-25); the same pattern is likely in future session-scoped RPCs.
**How to apply:** For every new session-scoped method, probe Null and array params; also check that broadcasts (`Daemon::broadcast` goes to every client, even pre-auth) do not undo the scoping. Related: [[agent-cli-protocol-verification]].
