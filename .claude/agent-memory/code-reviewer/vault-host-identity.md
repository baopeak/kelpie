---
name: vault-host-identity
description: SSH vault host objects are replaced on every saveVault/checkHost (ssh_host_touch); keyed Show/For on them remounts UI and drops local state
metadata:
  type: project
---

Every `checkHost` fires `ssh_host_touch(...).then(setVault)` and `saveVault` sets the backend's fresh copy, so all `SshHost` objects get new identities, even when nothing visible changed.

**Why:** found in the root-user guide review (2026-10-24). `<Show when={hostById(id)} keyed>` remounted the multi-step dialog on every check and wiped its state.

**How to apply:** when reviewing Solid UI driven by `hostById`/`vault()`, flag keyed `Show` or identity-based memos on host objects. Key on the id, or use non-keyed accessors.
