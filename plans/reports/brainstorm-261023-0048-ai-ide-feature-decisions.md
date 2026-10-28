# AI IDE features → Kelpie: danh sách đầy đủ & quyết định từng chức năng

Ngày: 2026-10-23 00:48 · Loại: brainstorm / decision record · Plan: [261023-0048-multi-agent-ide-roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md)
Đầu vào: [gap analysis](research-261023-0036-vibe-coder-gap-analysis.md) · [AI IDE survey](researcher-261023-0048-ai-ide-features.md) · [multi-agent workbenches + ACP](researcher-261023-0048-multi-agent-workbenches.md)

## 1. Nguyên tắc chấm điểm

Kelpie = **workbench cho NHIỀU agent CLI khác nhau**, không phải IDE một agent. Một chức năng chỉ được nhận nếu qua cả 4 câu hỏi:

1. **Generic?** Làm được cho mọi agent qua PTY / hook / transcript / ACP / git — không phụ thuộc API riêng một vendor. (Vendor-specific → chỉ nhận khi agent đó là Claude/Codex và chi phí nhỏ.)
2. **Đau thật?** Giải quyết việc người dùng chạy 5–20 agent làm hằng ngày.
3. **Rẻ khi không dùng?** Tắt / không mở = 0 CPU, 0 wakeup, 0 KB JS tải lúc khởi động (lazy import).
4. **Không thêm chrome cố định?** Gắn vào bề mặt sẵn có: chip trên row/tab, tab của right panel, mục palette, menu ngữ cảnh. Bàn phím đi tới được mọi thứ.

Ký hiệu: ✅ có rồi · ➕ thêm · ➖ thêm bản nhẹ · ❌ không làm.

## 2. Danh sách đầy đủ & quyết định

### A. Điều phối & phiên
| # | Chức năng (nguồn) | Quyết định | Lý do | Gắn vào Kelpie thế nào | Chi phí hiệu năng |
|---|---|---|---|---|---|
| A1 | Danh sách phiên hợp nhất mọi vendor (Cursor Agents Window, VS Code Sessions, Antigravity) | ✅ | Sidebar + dashboard đã có | — | — |
| A2 | **Hàng đợi chú ý theo trạng thái** (Claude Code Agent View, Warp panel) | ➕ P0 | Inbox là theo sự kiện; người dùng cần "ai đang chờ tôi, theo thứ tự" | Nâng dashboard thành *Attention*: nhóm Needs you → Failed → Done unread → Working; phím `Ctrl+Shift+.` nhảy phiên chờ kế tiếp; **Quick answer** popover: 12 dòng cuối màn hình + nút 1/2/3/y/n/Enter/Esc (dùng lại logic trả lời của phone access) | Dữ liệu status đã có ở 250 ms ticker; popover đọc `session.read` theo yêu cầu |
| A3 | Best-of-N: 1 prompt × N agent × N worktree (Cursor `/best-of-n`, Warp Oz) | ➕ P1 | Vendor IDE chỉ so model của mình; Kelpie so được *giữa các CLI* — khác biệt rõ nhất | Template Runs (không deps) + trang so sánh: file, +/−, check pass, thời gian, **cost** | Chạy trong daemon; trang so sánh lazy |
| A4 | Chế độ thực thi Local / Worktree / Cloud mỗi phiên (Codex app) | ➖ | Cloud cần API vendor | Chỉ Local / **New worktree** / Host (SSH) trong new-session menu | 0 |
| A5 | **Tự tạo worktree khi tạo phiên** (Claude Code Agent View) | ➕ P0 | Loại lỗi "quên cô lập" khi chạy song song | Setting: ask / always / never trong repo git; tên worktree gợi ý từ prompt đầu | `git worktree add` ~100–300 ms, chỉ khi tạo |
| A6 | Agent hand-off / chuyển agent | ✅ | Có hand-off | Thêm redaction (xem F4) | — |
| A7 | Background / cloud agents (Cursor, Codex cloud, Jules, Devin) | ❌ | Cần hạ tầng + API vendor; SSH host đã là "remote agent" generic | — | — |
| A8 | Orchestration DAG + approval | ✅ | Runs | — | — |
| A9 | Layout tự theo loại việc (Claude Code desktop) | ➖ P2 | Hữu ích nhưng đoán sai gây phiền | Chỉ 1 quy tắc: khi phiên mở port HTTP → gợi ý "Mở preview cạnh bên" (chip, không tự mở) | 0 thêm (dùng dữ liệu port) |

### B. Lập kế hoạch & spec
| # | Chức năng | Quyết định | Lý do | Gắn vào | Chi phí |
|---|---|---|---|---|---|
| B1 | **Spec workspace** req → design → tasks (Kiro, Antigravity plan artifacts) | ➕ P1 | Spec là markdown → mọi agent đọc được; đúng cách "agent A lập plan, agent B làm" | Right panel tab *Plan*: nhận diện `plans/*/plan.md`, `specs/`, `.kiro/specs`; render phase + checkbox; nút **Run task with…** (agent, worktree tùy chọn) → tạo phiên với prompt trỏ tới file; tiến độ cập nhật từ watcher | Watcher sẵn có; parse markdown khi file đổi (debounce 300 ms) |
| B2 | Plan mode riêng từng agent (`/plan`, shift+tab) | ❌ | Vendor-specific, agent tự có | Quick command "Plan first" (prompt template) là đủ | — |
| B3 | Artifacts có phản hồi inline (Antigravity) | ➖ P1 | PTY không nhận comment inline, nhưng **file** agent viết thì xem được | Turn timeline (F-phase checkpoints) hiển thị md/ảnh được tạo trong lượt, render bằng rich viewers; comment → review notes gửi agent | Chỉ render khi mở |

### C. Context & bộ nhớ
| # | Chức năng | Quyết định | Lý do | Gắn vào | Chi phí |
|---|---|---|---|---|---|
| C1 | **Composer với @file / @selection / @diff / @terminal / @issue / ảnh** (mọi IDE) | ➕ P0 | Đưa context vào agent là thao tác nhiều nhất; với TUI hiện phải tự gõ path | Composer nổi `Ctrl+Shift+I` trên pane: fuzzy @ dùng `nucleo` sẵn có; TUI nhận text đã ghép (path tương đối, khối code, path ảnh tạm); chat nhận như content block. "Send selection to agent" từ editor/diff/terminal | Composer lazy; fuzzy chạy Rust core |
| C2 | Dán ảnh vào agent TUI | ➕ P0 (trong C1) | Windows terminal không truyền ảnh; Claude/Codex/Gemini đọc ảnh theo path | Clipboard image → `%TEMP%/kelpie-img/<hash>.png` → chèn path | 1 lần ghi file |
| C3 | Rules / steering / instructions sync (CLAUDE.md, AGENTS.md, GEMINI.md, `.kiro/steering`, copilot-instructions) | ➕ P1 | Đau thật khi dùng 23 CLI | Trang *Agent setup* (xem E1) | Chỉ khi mở |
| C4 | Memories (Cursor, Windsurf, Warp Oz) | ❌ | Vendor-internal; Kelpie không nên làm kho nhớ thứ hai | C3 + MCP memory server do người dùng chọn | — |
| C5 | Codebase indexing / embeddings | ❌ | Tốn CPU/disk lớn, agent tự grep/search; trái mục tiêu hiệu năng | — | — |
| C6 | Additional workspace roots (ACP) | ➖ P2 | Đa repo | Chỉ cho ACP chat session: thêm thư mục | 0 |

### D. Review, checkpoint, merge
| # | Chức năng | Quyết định | Lý do | Gắn vào | Chi phí |
|---|---|---|---|---|---|
| D1 | **Checkpoint mỗi lượt + diff + undo lượt** (VS Code, Cursor, Zed) | ➕ P0 | Nhiều agent chung folder → không biết ai sửa gì | Daemon snapshot bằng temp index → hidden ref; Turn timeline trong pane | < 150 ms p95 / lượt, nền |
| D2 | **Review queue liên worktree** (Codex app, Conductor) | ➕ P0 | Việc xong của 10 agent cần một chỗ duyệt | Dựng trên trạng thái `review` của Board (agent tự set bằng `kelpied worktree set --status review`): thẻ trong cột Review + tab lọc ở right panel; Approve → Finish; Request changes → review notes → agent | Liệt kê từ git, cache theo HEAD + mtime |
| D3 | **Finish worktree**: checks → squash/merge/rebase → dọn | ➕ P0 | Vòng đời worktree đứt ở bước cuối | Dialog Finish (xem gap G3) | Nền |
| D4 | Overlap watch giữa worktree | ➕ P0 | Xung đột ngữ nghĩa giữa agent | Chip cảnh báo | `git diff --name-only` mỗi 30 s, chỉ khi ≥ 2 worktree active |
| D5 | Cross-review bằng agent khác | ➕ P1 | Agent tự review bỏ sót lỗi như lúc viết | "Review with…" trong Review queue | 0 |
| D6 | PR review bot (Bugbot, Copilot review) | ❌ | Vendor; D5 làm bản local | — | — |
| D7 | Automations giao kết quả vào Review queue (Codex app) | ➕ P1 (trong D2) | Kết quả lịch chạy cần duyệt chứ không chỉ thông báo | Automation có `deliver: review` | 0 |

### E. Công cụ, MCP, giao thức
| # | Chức năng | Quyết định | Lý do | Gắn vào | Chi phí |
|---|---|---|---|---|---|
| E1 | **MCP manager + sync** sang mọi CLI | ➕ P1 | Mỗi CLI 1 định dạng | Trang Agent setup | Chỉ khi mở |
| E2 | Skills / commands / subagents liệt kê (`.claude/commands`, `.claude/agents`, skills) | ➖ P2 | Hữu ích để biết agent có gì; sửa thì để editor | Read-only list trong Agent setup, mở file | Chỉ khi mở |
| E3 | **ACP client cho mọi agent hỗ trợ** (Zed, JetBrains; 46 agent) | ➕ P0 | Giao thức chung → chat view có cấu trúc (diff, permission, usage, title, elicitation) cho Gemini, OpenCode, Copilot, Goose, Qwen, Kimi, Cline, Droid, Kiro, Mistral Vibe, Pi… thay vì screen scraping | `acp` args do app quyết định (`agents.ts`, custom agent def); `chat.rs` phân loại trạng thái theo loại update; chat view dùng `acp-adapter.ts` tổng quát; Claude/Codex giữ adapter native (giữ hook) | Relay JSON-lines sẵn có trong daemon |
| E4 | ACP Registry để thêm agent | ➖ P2 | Danh sách preset tay dễ cũ | Nút "Find more agents" đọc registry (1 request khi bấm) → tạo custom agent def | Mạng chỉ khi bấm |
| E5 | Agent-driven browser (Antigravity, Cursor) | ❌ | Vendor; agent dùng Playwright MCP | E1 gợi ý cài Playwright MCP | — |
| E6 | Event hooks (Kiro hooks: on save / pre-post task) | ➕ P1 | Automations hiện chỉ theo giờ | Trigger: file save (glob), agent Stop, commit, PR check fail → prompt/shell/review; chống vòng lặp | Watcher sẵn; debounce; rate limit |
| E7 | Plugin / extension | ✅ | Plugins khai báo | — | — |

### F. Chi phí, an toàn, thông báo
| # | Chức năng | Quyết định | Lý do | Gắn vào | Chi phí |
|---|---|---|---|---|---|
| F1 | **Context % / token / cost mỗi phiên** | ➕ P0 | Quyết định compact/hand-off; tiền | ACP `usage_update` cho phiên chat ACP; transcript tail cho Claude/Codex/OMP TUI | Tail theo offset, chỉ phiên visible/working |
| F2 | Cost cap mỗi phiên (Warp governance) | ➖ P1 | Bản solo: cảnh báo khi vượt ngân sách | Setting $/phiên; vượt → inbox + chip đỏ; tùy chọn gửi Esc (Claude/Codex) | 0 |
| F3 | Push ra ngoài máy (ntfy / Telegram / webhook) + limit actions | ➕ P0 | Task dài, không ngồi máy | Daemon gửi | Chỉ khi có sự kiện |
| F4 | **Redact secret** trong text gửi ra (Copy Context, hand-off, cross-review, notify) | ➕ P0 | An toàn rẻ | `kelpie_proto::redact` + lệnh Tauri cho TS, làm ở phase 1 (trước composer) | Regex trên text gửi đi |
| F5 | Sandbox / autonomy levels (Antigravity strict, containers) | ❌ (now) | Non-goal; Manual/Yolo + worktree đủ cho solo | Xem lại khi có yêu cầu devcontainer | — |
| F6 | Auto-update | ➕ P0 | Phát hành | Updater ký số | Check mỗi 6 h |

### G. Editor & tiện ích
| # | Chức năng | Quyết định | Lý do | Gắn vào | Chi phí |
|---|---|---|---|---|---|
| G1 | Tab / next-edit / inline edit | ❌ | Cần model riêng, cạnh tranh với agent, Kelpie không phải editor chính; đã có *Open in* VS Code/Cursor/Zed | — | — |
| G2 | LSP diagnostics | ❌ | Nặng (mỗi ngôn ngữ một server) | Thay bằng *checks* (D3): output lỗi → "Fix with…" | — |
| G3 | Processes & ports & preview | ➕ P1 | Ghost process, port bận | Chip `:5173`, panel Processes | Scan 5 s chỉ khi có tiến trình con |
| G4 | Session export (Markdown) / share | ➖ P2 | Chia sẻ cho team, viết PR | Export từ History panel (Claude/Codex/OMP/ACP) | Chỉ khi bấm |
| G5 | PTY recording / replay | ❌ | Ghi mọi byte = disk + CPU; ít dùng | — | — |
| G6 | Multiplayer / collab realtime | ❌ | Cần server | — | — |
| G7 | Git log / branch / stash | ➖ P2 | Cơ bản còn thiếu | Changes panel | Khi mở |
| G8 | Linux | ➕ P1 | Nhiều dev | CI AppImage/.deb | — |
| G9 | Export/import cấu hình | ➖ P2 | Nhiều máy | Settings → System | — |

## 3. Bề mặt UI sau khi thêm (không thêm thanh cố định nào)

```
┌ Sidebar ─────────┐┌ Tabs (ring context %, chip :port, chip cost) ──────────┐┌ Right panel tabs ─────────────┐
│ project          ││ pane toolbar: [Turns] [Compose] [chat↔TUI]            ││ Files · Changes · Review(new) │
│  ├ session ●72%  ││                                                       ││ Plan(new) · Processes(new)    │
│  └ worktree ⚠ovl ││ Composer (Ctrl+Shift+I, floating, @ mentions, image)  ││ Issues · History · Browser    │
└──────────────────┘└───────────────────────────────────────────────────────┘└───────────────────────────────┘
Status bar: … [bell inbox] [⬆ update] [usage] — Attention: Ctrl+Shift+M (dashboard nâng cấp), next waiting Ctrl+Shift+.
Settings mới: Agent setup (MCP · Instructions · Skills), Notifications channels, Updates, Checkpoints, Cost caps.
```

## 4. Hiệu năng — cổng chung cho mọi chức năng
- Phase nền tạo `scripts/perf/perf_gate.py` + `baseline.json`: cold start, project switch p50/p95, keystroke echo p95, idle CPU (app+daemon, 20 phiên, 60 s), RSS, daemon bench 20×2 MiB, JS bundle (entry + lazy chunks), long tasks > 50 ms trong kịch bản chuẩn.
- Mỗi phase: chạy gate, ghi `plans/reports/perf-<phase>.json`; vượt ngân sách → profile (WebView2 DevTools Performance, `tracing` spans, `cargo flamegraph`) và tối ưu trước khi đóng phase.
- Quy tắc thiết kế: lazy import cho mọi panel mới; không polling ở UI; daemon đẩy delta; việc nặng ở tokio blocking pool; tắt = 0 wakeup.

## 5. Bị loại và vì sao (tóm tắt)
Tab/next-edit, LSP, indexing/embeddings, memories, cloud agents, sandbox/container, PR bot, agent-driven browser, PTY replay, multiplayer: hoặc vendor-specific, hoặc tốn tài nguyên thường trực, hoặc cần server — đều trái 4 nguyên tắc ở mục 1.

## Unresolved questions
1. ACP cho Claude/Codex qua adapter (`claude-agent-acp`, `codex-acp`)? Đề xuất: không — giữ adapter native (giữ hook/status), xem lại khi adapter đạt parity.
2. Gemini/OpenCode/Copilot: ACP native hay adapter? Cần chạy thử trên máy (spike đầu phase ACP).
3. Checkpoint bật mặc định? Phụ thuộc số đo trên repo lớn.
4. Phím tắt `Ctrl+Shift+.` / `Ctrl+Shift+I` có đụng phím TUI agent nào không — kiểm bằng shortcut-conflict checker sẵn có.

## Cập nhật sau red-team (2026-10-23)
Red-team 4 góc nhìn (40 phát hiện → 15) đã sửa plan; tóm tắt ảnh hưởng tới các quyết định trên:
- Thêm nền tảng bắt buộc: **phân quyền daemon theo phiên** (token mỗi phiên, lệnh quản trị chỉ cho app), **lọc ký tự điều khiển** khi dán vào PTY, **redaction** gọi được từ TS — tất cả ở phase 1.
- Review queue (D2) dựng trên trạng thái `review` của Board; review notes chuyển về daemon; best-of-N là Run thường với N task.
- Checkpoint (D1) dùng snapshot nối chuỗi (cuối lượt N = đầu lượt N+1), undo bị chặn khi lượt chồng thời gian với phiên khác.
- Finish (D3) commit WIP trước, chạy check trong worktree tạm, `check:` chỉ đọc từ nhánh base qua bước duyệt hash.
- Quyết định người dùng: bỏ tách `store.ts`; chuông giữ số chưa đọc + chấm "chờ bạn"; checkpoint bật khi đạt chuẩn đo.
Chi tiết: [plan.md § Red Team Review](../261023-0048-multi-agent-ide-roadmap/plan.md#red-team-review).
