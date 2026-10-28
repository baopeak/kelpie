# Repository Guidelines

## Project Overview

Kelpie is a Tauri 2 desktop workbench for coding-agent CLIs and shells across projects. SolidJS renders the interface; a separate Rust daemon, `kelpied`, keeps sessions running when the window closes. Agents are external programs, not bundled model implementations.

## Architecture & Data Flow

- **Terminal path:** SolidJS → `app/src/lib/ipc.ts` → Tauri commands → `DaemonClient` → local named pipe (Windows) or Unix socket → `kelpied` → PTY. Output returns as a snapshot plus sequenced binary updates over a Tauri `Channel` to xterm.js. Keep terminal traffic binary and preserve replay offsets/backpressure; slow views must not block PTY readers.
- **Ownership:** the daemon owns processes, terminal buffers, session lifecycle, automations and runs. The desktop bridge owns native integrations; the frontend owns presentation and reactive UI state. Do not move persistent session work into the window process.
- **Chat path:** `app/src/chat/chat-model.ts` normalizes agent protocol events through Claude, Codex and ACP adapters. Preserve the replay/live boundary: history replay must never send agent commands. `mode-switch.ts` hands a conversation between terminal and chat; do not create two writers for one conversation.
- **Boundary changes:** update the shared protocol, daemon dispatcher, Tauri bridge and TypeScript wrappers/types together. Preserve authentication roles, frame limits and serialized compatibility defaults. See `docs/architecture.md` for transport and security details.

## Key Directories

- `app/src/components/`: SolidJS views; `app/src/lib/`: shared state, IPC and application services; `app/src/chat/`: normalized chat state and agent adapters.
- `app/src-tauri/src/`: native commands, daemon connection, filesystem/Git operations and agent configuration/history.
- `crates/kelpied/src/`: daemon/session runtime; `crates/kelpie-proto/src/`: shared framing, endpoints and authentication helpers; `crates/kelpie-stt/`: optional native speech helper.
- `scripts/e2e/` and `scripts/perf/`: real-app QA and performance gates. `docs/architecture.md` and `docs/user-guide.md` describe current contracts; `plans/` and the large Orca design assets are historical context, not implementation authority.

## Development Commands

Run Cargo and root scripts from the repository root; run pnpm commands from `app/`.

| Purpose | Command |
| --- | --- |
| Install JS dependencies reproducibly | `cd app && pnpm install --frozen-lockfile` |
| Start/rebuild isolated daemon (foreground) | `bash scripts/dev-daemon.sh run` |
| Start desktop against that daemon, separate Bash shell | `cd app && KELPIE_ENDPOINT=kelpie-dev pnpm tauri dev` |
| Frontend dev server only | `cd app && pnpm dev` |
| Frontend production build, including typecheck | `cd app && pnpm build` |
| Rust default-member build | `cargo build` |
| Rust suite, including speech helper | `cargo test --workspace` |
| Frontend checks | `cd app && pnpm typecheck && pnpm test && pnpm i18n:check` |
| Windows installer / macOS bundle | `bash scripts/build-release.sh --bundles nsis` / `bash scripts/build-release.sh --bundles app,dmg` |

No dedicated lint script is configured; do not assume `pnpm lint` exists. Vite alone does not provide the native Tauri APIs needed to exercise the desktop app.

The release script **bumps patch versions** in the workspace manifest, frontend manifest and Tauri config unless `KELPIE_KEEP_VERSION=1` is set. It builds/copies daemon and speech sidecars before bundling; even `--no-bundle` is not a lightweight validation command.

## Code Conventions & Common Patterns

- Follow adjacent formatting: TypeScript uses double quotes, semicolons and two-space indentation; Rust uses conventional four-space formatting. No repository-wide ESLint/Prettier policy is configured. Keep TypeScript strict and unused-local/parameter checks passing.
- Use Solid signals, memos, effects and stores, not React state assumptions. App-wide state lives in `app/src/lib/store.ts`; chat state changes go through `ChatController`/`AdapterContext`. Keep protocol translation inside adapters.
- TypeScript identifiers generally use camelCase; component exports use PascalCase and many filenames are kebab-case. Preserve snake_case fields where they mirror Rust/wire contracts; do not rename one side alone.
- Keep Tauri calls in the typed IPC wrappers. Native commands commonly return `Result<T, String>`/`CmdResult<T>`; internal Rust code may use `anyhow::Result`. Surface actionable errors without exposing credentials.
- Use Tokio for asynchronous transport and `spawn_blocking`/`fs_ops::blocking` for substantial filesystem/config/history work. PTY reading, writing/resizing and exit observation have dedicated threads; preserve that ownership.
- Reuse Tauri managed services (`app.manage`, `State`, shared `Arc<DaemonClient>`) rather than adding a DI framework. Chat adapters receive an explicit context, also used by tests.
- Preserve path/input guards, compare-before-write checks and atomic state persistence. A failed state read is not permission to overwrite the state with defaults.

## Important Files

- **Entry points:** `app/src/main.tsx`, `app/src/components/app.tsx`, `app/src-tauri/src/lib.rs`, `crates/kelpied/src/main.rs`.
- **Cross-process changes:** `app/src/lib/ipc.ts`, `app/src/lib/types.ts`, `app/src-tauri/src/daemon_client.rs`, `crates/kelpie-proto/src/lib.rs`, `crates/kelpied/src/server.rs`, `crates/kelpied/src/daemon.rs`, `crates/kelpied/src/session.rs`.
- **Build/config owners:** `Cargo.toml`, `app/package.json`, `app/tsconfig.json`, `app/vite.config.ts`, `app/vitest.config.ts`, `app/src-tauri/tauri.conf.json`, `app/src-tauri/tauri.bundle.json` and `.github/workflows/ci.yml`.

## Runtime/Tooling Preferences

- Use Rust stable (edition 2021), Node.js and pnpm 11. README requires Node 22+; CI uses Node 24, the safest matching development choice. Bun/npm are not the repository's install/run workflow. Preserve `Cargo.lock` and `app/pnpm-lock.yaml`.
- Windows needs MSVC build tools; macOS builds need Apple tooling. Shell examples above use Bash (e.g. Git Bash on Windows). In PowerShell, set `$env:KELPIE_ENDPOINT = 'kelpie-dev'` before running the desktop command.
- The dev-daemon helper defaults to endpoint `kelpie-dev` and a temporary data directory, and stops its previous dev PID before rebuilding. Use isolated `KELPIE_ENDPOINT` and `KELPIE_DATA_DIR` for test daemons; do not stop or mutate the user's default daemon.
- `kelpie-stt` is excluded from default Cargo members because it downloads/links a large native library; `--workspace` tests and release builds include it.
- For inspection, use `kelpied call session.list` or `python scripts/kelpie-call.py session.list` against the intended endpoint. Do not bypass the Python helper's default-daemon destructive-call guard.

## Testing & QA

- Vitest discovers colocated `app/src/**/*.test.ts`, runs in Node, and resolves Solid's browser build. Use `cd app && pnpm exec vitest run src/lib/layout.test.ts` for a focused file, adding `-t "test name"` to filter. Follow existing adapter tests' injected contexts and in-memory stores.
- Rust tests use ordinary Cargo tests and colocated test modules. Focus with `cargo test -p kelpied <test_filter>`; CI explicitly runs `cargo test -p kelpie-proto -p kelpied -p kelpie -p kelpie-stt` on Windows and macOS.
- No enforced coverage percentage is configured. Add behavior-level regressions for changed contracts; unit tests do not verify WebView/native interactions.
- Exercise affected desktop paths through the real app. Python scripts in `scripts/e2e/` document their own arguments and prerequisites; GUI scenarios generally need `tauri-driver` and matching `msedgedriver`. Some invoke real signed-in agents, incur model calls, manipulate SSH fixtures or restart daemons: inspect setup/cleanup before running.
- Performance procedure and budgets live in `scripts/perf/README.md` and `scripts/perf/baseline.json`. Example: `python scripts/perf/perf_gate.py --label my-change --compare scripts/perf/baseline.json`. Full GUI metrics require a desktop/WebDriver environment and `psutil`; CI gates bundle size and daemon throughput. Do not update the baseline merely to hide a regression.
