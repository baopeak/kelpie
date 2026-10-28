# Research Report: Chat view parity với CLI — Claude / Codex / OMP-ACP

Timestamp: 2026-10-23 ~01:00 (Asia/Saigon) · Nhánh: `rebrand/kelpie` · Loại: research (chưa code) · ~22 lượt search/fetch

## Executive Summary

- **Claude headless gần parity hơn tưởng.** Slash command gửi như user text qua stream-json được CLI tự xử lý. Kelpie chưa đọc `system/init` / chưa gọi `control_request{initialize}` → bỏ sót danh sách command, model, permissionMode, skills, agents.
- **Danh sách command có sẵn trong `system/init`:** `slash_commands` gộp built-in + bundled skills + user skills + `.claude/commands/` + MCP prompts; `terminal_slash_commands` đánh dấu lệnh gắn terminal. Lệnh TUI-only (`/theme`, `/terminal-setup`, `/login`…) trả `"/x isn't available in this environment."`.
- **Codex app-server có RPC riêng cho từng command quan trọng:** `thread/compact/start`, `review/start`, `model/list`, `skills/list`, `config/read`, `config/value/write`, `command/exec`. `$skill` = text `$name` + input item `{type:"skill", name, path}`.
- **ACP chuẩn hoá nhất:** `available_commands_update` (name, description, input.hint), gửi `/cmd args` dạng text trong `session/prompt`; `session/set_config_option` cho model/mode/thought_level (`modes` cũ sắp bị gỡ).
- **Parity 100% không phải mục tiêu kể cả của chính hãng** (Codex IDE extension: 22 lệnh vs ~50 lệnh CLI). Hybrid: lệnh nào không có headless → nút "Open in TUI" (Kelpie đã có `mode-switch.ts`).
- **Render dài:** `@tanstack/solid-virtual` 3.13.49 + markdown theo block có memo + collapse tool output. `streamdown` là React → không dùng được.

## Phương pháp

- Đọc code: `app/src/chat/claude-adapter.ts`, `codex-adapter.ts`, `acp-adapter.ts`, `app/src/components/chat-pane.tsx`, `app/package.json`.
- Docs: Claude Code slash-commands, Agent SDK TS reference, Agent SDK slash-commands; Codex app-server, Codex CLI + IDE slash commands; ACP slash-commands + session-config-options; TanStack Virtual chat guide. `npm view` lấy version.
- Hạn chế: phần cuối trang Codex app-server (~43k ký tự) và TS reference (~96k ký tự) chưa đọc; GUI bên thứ ba và OMP chưa xác minh bằng nguồn chính.

## Hiện trạng chat view Kelpie (gaps)

- Claude: spawn với `--permission-prompt-tool stdio`; chỉ dùng control_request `interrupt` + `can_use_tool`. Không đọc `slash_commands`, `permissionMode`.
- Codex: chỉ initialize, thread/turn, approvals, interrupt. ACP: chỉ initialize + request_permission. Không xử lý `available_commands_update` / `config_option_update`.
- Không có UI đổi model / permission mode / effort.
- Composer là `<textarea>` thường: không popover `/`, `$`, `@`; không `!` bash mode.
- Render: `<For each={items()}>` không virtualize; markdown bằng `marked` + DOMPurify, memo theo item. Item đang stream là `<div>` pre-wrap, chỉ parse markdown khi xong → có thể giật layout lúc finalize.
- Có thể dùng event `compact_boundary` để vẽ divider khi `/compact`.

## Parity matrix

OK = headless chạy được · Fallback = đẩy sang TUI (mode-switch).

| Feature | Claude (`-p` stream-json) | Codex (app-server) | OMP / ACP |
|---|---|---|---|
| Slash built-in | OK cho lệnh có trong `slash_commands` (`/compact`, `/clear`, `/context`, `/usage`…) gửi như text. TUI-only → "isn't available in this environment" → Fallback | Không có "gõ /": map UI → RPC. Fallback TUI | Agent khai báo qua `available_commands_update`, gửi text `/cmd` |
| Custom commands (`.claude/commands`) | OK, cùng bề mặt với skills, nằm trong `slash_commands` (cần setting sources user/project — mặc định có) | Không thấy tương đương (chưa xác minh) | Tuỳ agent (chưa xác minh OMP) |
| Plugin commands / skills | OK, `/plugin:skill`; chỉ skill đầu tiên trong message được nạp | `skills/list` + `$name` + item `skill` | Qua available commands |
| MCP prompts | OK (là commands khi MCP đã cấu hình) | Không thấy RPC (chưa xác minh) | Chưa xác minh |
| `@file` | Text `@path`, CLI tự expand (cần kiểm thực nghiệm); GUI tự làm autocomplete bằng file index | Item `text`/`localImage`; GUI tự index file (chưa xác minh) | Content block `resource_link` (chưa đọc kỹ) |
| `!bash` | Không có bash mode headless rõ ràng → GUI tự chạy shell hoặc Fallback | `command/exec` (sandbox, không cần turn) | Cần client capability `terminal` (Kelpie đang tắt) |
| Model | `control_request{set_model}` (SDK `setModel`); list từ `supportedModels()` | `model/list` + override `model` ở `turn/start` | `set_config_option` category `model` |
| Effort / thinking | Option `thinking`; `updateSettings({effortLevel})` | Override `effort` ở `turn/start` | Category `thought_level` |
| Permission mode | `set_permission_mode`: default / acceptEdits / bypassPermissions / plan / dontAsk / auto | `approvalPolicy` + sandbox ở `turn/start` | Category `mode` (hoặc `modes` cũ) |
| `/compact` | OK, phát `compact_boundary` (`pre_tokens`) | `thread/compact/start` | Chỉ nếu agent khai báo command |
| `/clear` | OK (reset context; session cũ còn trên disk) | `thread/start` mới | `session/new` |
| Resume | `--resume <id>` (đã có); picker TUI-only | `thread/resume` (chưa kiểm) | `session/load` nếu có capability (chưa xác minh) |
| MCP status | `mcpServerStatus`, `toggleMcpServer`, `reconnectMcpServer`, `setMcpServers` | `mcpServerStatus/list`, `config/mcpServer/reload`, `mcpServer/oauth/login` | Chưa xác minh |
| Config | `applyFlagSettings`, `updateSettings` (allowlist hẹp: `outputStyle`, `effortLevel`); `/config` TUI-only | `config/read`, `config/value/write` (ghi `config.toml`) | Chỉ option agent khai báo |
| Login | `/login` TUI-only; `accountInfo()` để hiển thị | Chưa thấy trong phần đã đọc | `authenticate` của agent (chưa đọc) |
| Review | `/code-review` bundled skill chạy như prompt | `review/start` (uncommittedChanges / baseBranch / commit / custom) | Qua command nếu có |

## Pattern từ các GUI khác

- **Có bằng chứng:** SDK docs mô tả đọc `slash_commands` từ `system/init` để dựng autocomplete; ACP mô tả popover từ `available_commands_update` + `input.hint`; Codex IDE extension chỉ có bộ lệnh con (22) — `/theme`, `/vim`, `/keymap` không có.
- **Chưa xác minh** (search không ra source): opcode/Claudia, Conductor, Crystal, Happy, CodexMonitor, claude-code-webui, Nimbalyst, Cursor/Windsurf, Zed. Cần đọc source từng repo nếu muốn dùng làm bằng chứng.
- Hybrid fallback sang TUI là hướng hợp lý; Kelpie đã có `mode-switch.ts`.

## Kỹ thuật render hội thoại dài

Version (`npm view`, 2026-10-23): `@tanstack/solid-virtual` 3.13.49, `@tanstack/virtual-core` 3.19.7, `virtua` 0.53.6, `streamdown` 2.7.1 (React-only), `marked` 18.1.1, `solid-js` 1.9.17.

- **Virtualization:** `@tanstack/solid-virtual`. TanStack chat guide: key ổn định (`getItemKey`), container cao cố định, `measureElement` cho height động, `anchorTo: 'end'` cho stream/prepend, `isAtEnd()` để hiện "Jump to latest". Ví dụ là React → tự chuyển sang Solid. `virtua` có Solid nhưng chưa xác minh chất lượng cho chat/reverse.
- **Streaming markdown theo block:** `marked.lexer` tách block; memo block đã xong, chỉ re-parse block cuối → thay `pre-wrap` raw khi đang stream, hết giật lúc finalize.
- **Giảm tải item:** collapse tool output mặc định (như `diff.slice(0, 400)` hiện có), "load earlier", hoặc `content-visibility: auto` + `contain-intrinsic-size` cho item cũ nếu chưa muốn virtualize.
- **Web worker** cho parse/highlight: chưa có nguồn, chỉ làm sau khi đo.
- Rủi ro: virtualization + tool card auto-expand + height đổi giữa stream → nhảy scroll. Cần e2e (`scripts/e2e`).

## Đề xuất kiến trúc cho Kelpie

```
Composer (Solid)
  ├─ "/"  -> CommandCatalog
  ├─ "$"  -> SkillCatalog (Codex)
  ├─ "@"  -> FileIndex (daemon)
  └─ "!"  -> shell exec (GUI-owned)

CommandCatalog.fill():
  Claude : control_request initialize / system.init (slash_commands, model, permissionMode)
  Codex  : model/list, skills/list, config/read
  ACP    : available_commands_update, config_options

Routing mỗi command:
  có RPC riêng?            -> structured call (compact / review / model / mode)
  nằm trong slash_commands? -> gửi text "/cmd args"
  terminal-only / unknown  -> "Open in TUI" (mode-switch)

ChatList: solid-virtual + markdown memo theo block + tool output collapse
```

| # | Hạng mục | Impact | Effort | Risk |
|---|---|---|---|---|
| 1 | Claude: đọc `system/init` (+ `initialize`), popover `/` từ `slash_commands`, chặn `terminal_slash_commands` | Cao | Thấp | Thấp (CLI cũ có thể thiếu field) |
| 2 | Header controls model / permission mode / effort cho 3 adapter | Cao | TB | TB (3 giao thức) |
| 3 | ACP: `available_commands_update` + `set_config_option` | TB | Thấp | Thấp |
| 4 | Codex: compact/review/model/skills RPC + `$skill` | TB | TB | TB |
| 5 | `@file` popover từ file index daemon | TB | TB | Thấp |
| 6 | Virtualize + streaming markdown theo block | Cao với phiên dài | Cao | TB (scroll anchoring) |
| 7 | Nút "Open in TUI" cho command không hỗ trợ | TB | Thấp | Thấp |

Thứ tự khuyến nghị: 1 → 3 → 2 → 4 → 7 → 5 → 6.

## Nguồn tham khảo

- Claude Code slash commands / skills: https://code.claude.com/docs/en/slash-commands
- Agent SDK TS reference: https://code.claude.com/docs/en/agent-sdk/typescript
- Agent SDK slash commands: https://code.claude.com/docs/en/agent-sdk/slash-commands
- Codex app-server: https://learn.chatgpt.com/docs/app-server
- Codex CLI slash commands: https://learn.chatgpt.com/docs/developer-commands.md?surface=cli
- Codex IDE slash commands: https://learn.chatgpt.com/docs/developer-commands.md?surface=ide
- ACP slash commands: https://agentclientprotocol.com/protocol/slash-commands
- ACP session config options: https://agentclientprotocol.com/protocol/session-config-options
- TanStack Virtual chat guide: https://tanstack.com/virtual/latest/docs/chat

## Câu hỏi chưa giải quyết

1. Wire format chính xác của `control_request{initialize | set_model | set_permission_mode}` trong CLI 2.1.x — cần đọc source SDK hoặc bắt traffic thật.
2. `@file` có được expand trong `-p` stream-json không?
3. OMP có phát `available_commands_update` / config options không?
4. Codex: có RPC login / MCP prompts không (phần docs chưa đọc)?
5. Các GUI khác (opcode, Happy, CodexMonitor…) làm thế nào — cần đọc source.
