---
name: markdown-slicing-and-remote-write-traps
description: Lexing markdown in slices breaks HTML comments, nested fences and end-of-file ref defs; ssh `cat > tmp && mv` commits truncated content on EOF and replaces symlinks
metadata:
  type: feedback
---

Two traps found in the phase 10 re-review (2026-10-25), both in "fixes" for earlier findings:

1. **Sliced markdown lexing** (cut a doc between list items, lex slices with one shared marked `Lexer`) is not equivalent to a whole-file parse. Probe recipe: compare `parseSpec(text, chunkLines=1|2|3|7)` with `chunkLines=1e9` on fixtures containing an HTML comment `<!-- … -->` around `- [ ]` lines, a `<details>` block, a ```` ````md ```` fence containing a ```` ``` ```` fence, and `[text][ref]` with `[ref]: url` at the end. All four diverged (phantom tasks, changed task text/ids). Also measure the longest async turn on docs whose lists have one top-level item each (no cut points → one 130–190 ms turn at 256 KB).

2. **Remote atomic write over ssh** (`cat > "$f.tmp" && mv -f "$f.tmp" "$f"`): when stdin ends early (sshd closes a dropped channel's stdin) `cat` exits 0 and the truncated temp is committed. Reproduced by running the script under Git Bash and closing stdin a third of the way. And `mv` over a symlink replaces the link instead of writing through it, while the local `fs_write_file` writes through links (CLAUDE.md -> AGENTS.md breaks). Fix pattern: check the size or cksum of the temp file before `mv`; resolve `readlink -f` first.

3. **Shell script templates** filled with chained `.replace("__X__", …)` where an attacker-influenced value (a path) is substituted first: later placeholders get replaced inside the quoted path and break quoting (`a__NEW__b.md` ran a command `8b.md`). And `t="$f.tmp.$$"` + `: > "$t"` is a predictable non-O_EXCL temp (symlink plant on multi-user hosts, umask exposure): ask for `mktemp "$f.tmp.XXXXXX"`. Probe recipe: regex-extract the `r#"…"#` template from the .rs file, fill it in Python exactly like the Rust, run with Git Bash `sh -c`.

**Why:** The developer's equivalence tests used generated lists only, and the atomic-write claim was untested against EOF.
**How to apply:** When a diff adds chunked parsing or temp-and-rename writes, run these probes before accepting "identical results" or "never truncated". Related: [[auth-param-scoping-traps]], [[gui-e2e-isolation-wrapper]].
