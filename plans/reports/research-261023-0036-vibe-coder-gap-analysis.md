# Research Report: Kelpie qua mắt một vibe coder — kiểm kê tính năng, lỗ hổng, hướng bổ sung

Ngày: 2026-10-23 00:36 (Asia/Saigon) · Nhánh: `rebrand/kelpie` · Loại: research + gap analysis (chưa code)
Plan triển khai: [261023-0036-vibe-coder-gaps](../261023-0036-vibe-coder-gaps/plan.md)

## Mục lục
1. [Executive Summary](#executive-summary)
2. [Phương pháp](#research-methodology)
3. [Kiểm kê tính năng hiện có](#1-kiểm-kê-tính-năng-hiện-có)
4. [Bối cảnh thị trường 2026](#2-bối-cảnh-thị-trường-2026)
5. [Nỗi đau thực tế của vibe coder](#3-nỗi-đau-thực-tế-của-vibe-coder-chạy-nhiều-agent)
6. [Gap analysis](#4-gap-analysis--kelpie-còn-thiếu-gì)
7. [Đề xuất ưu tiên](#5-đề-xuất-ưu-tiên)
8. [Hiệu năng & bảo mật](#6-ràng-buộc-hiệu-năng--bảo-mật)
9. [Không làm](#7-không-làm-non-goals)
10. [Nguồn](#resources--references) · [Câu hỏi mở](#unresolved-questions)

## Executive Summary

Kelpie đã **rất đầy đủ ở tầng "chạy & quản lý phiên"**: daemon giữ phiên bất tử, 23 agent preset, status từ hook, worktree, board, inbox, automations, runs, SSH, phone, voice, plugin, i18n. So với đối thủ 2026 (Conductor, Claude Squad, Vibe Kanban, Parallel Code), Kelpie mạnh hơn ở độ bền phiên, Windows-first, số agent và SSH.

Lỗ hổng thật nằm ở **tầng "sau khi agent chạy xong"** — đúng chỗ vibe coder mất thời gian nhất: (1) *agent này đã sửa gì, lượt nào, hoàn tác thế nào*; (2) *còn bao nhiêu context / tốn bao nhiêu tiền*; (3) *đưa worktree về nhánh chính* (chỉ có PR, không có merge/cleanup cục bộ, không cảnh báo chồng file giữa các agent); (4) *báo tôi khi tôi không ngồi trước máy* (phone access cần mở trang, không có push thật); (5) *hết quota thì sao* (đã phát hiện limit, nhưng không failover / hẹn resume). Thêm: **không có auto-update**, không có Linux, không quản lý MCP / file hướng dẫn agent, không thấy tiến trình/port mà agent bỏ lại.

Khuyến nghị: 8 phase, ưu tiên P0 = checkpoint & diff theo lượt, context/cost theo phiên, worktree finish flow, push notification + limit resume, auto-update. Tất cả bám kiến trúc sẵn có (daemon hook, transcript scanner, proc_tree, inbox, accounts, runs) — không thêm dịch vụ mạng, không telemetry.

## Research Methodology
- Nguồn: codebase (README, `docs/user-guide.md`, `docs/architecture.md`, 9 plan + 20 report cũ, grep mã nguồn) + 5 lượt web search (≈45 kết quả).
- Khoảng thời gian tài liệu: 2025 → 10/2026.
- Từ khóa: parallel coding agents desktop 2026, Conductor / Vibe Kanban / Claude Squad, worktree pain points, Claude Code statusline context/cost, ccusage, MCP/skills manager, ntfy/Telegram hooks remote approval.
- Lưu ý: phần lớn nguồn so sánh là blog của vendor/affiliate → chỉ dùng làm tín hiệu xu hướng, không làm bằng chứng tuyệt đối.

## Key Findings

### 1. Kiểm kê tính năng hiện có

| Nhóm | Tính năng (đã có, đã có e2e) |
|---|---|
| Phiên & agent | Daemon `kelpied` giữ PTY/agent khi app đóng/crash; resume hội thoại sau restart; status working / needs-you / done / idle / failed qua hook (Claude, Codex) hoặc output; 23 preset CLI + args/env; Manual/Yolo; prompt đầu khi tạo phiên; hand-off sang agent khác; hibernation; keep-awake |
| Terminal | xterm WebGL, ConPTY bundled, OSC 52, links, find, drag path; split/maximize/tab kéo-thả; pill dòng mới + nhảy giữa prompt; context menu (Copy Context…); 9 theme; floating terminal; shortcut tránh phím TUI |
| Layout | Grid rows×cols / per-row; kéo pane; equalize; focus theo hướng; broadcast; layout lưu; Tile all / Spread |
| Chat view | Claude/Codex/OMP chuyển TUI ↔ chat (tool card, diff, Allow/Deny, cost cuối lượt) |
| Lịch sử | Danh sách hội thoại cũ (Claude/Codex/OMP), tìm, resume ở terminal/chat; scan 1.2 GB trong 0.5 s |
| Dự án & worktree | Sidebar (branch, dirty, ahead/behind, PR chip), filter/sort/pin; worktree từ mọi ref; copy/link file ignore; `.kelpie.yaml` setup; xóa an toàn; worktree comment (agent set qua CLI); board Todo→Done |
| Source control | Stage/discard/commit/amend; push/pull/sync/force-with-lease; banner merge/rebase + Abort + Resolve with AI; AI commit message, Fix with AI; GitHub PR (gh): tạo PR AI, checks, Fix broken checks; GitLab MR/pipeline; review notes trên diff → gửi agent |
| File | File tree, find in folder, CodeMirror 6 editor, viewer Markdown+Mermaid / CSV / PDF / Jupyter / ảnh; palette tìm mọi thứ |
| Hoạt động | Inbox (finished / needs you / automation / worktree mới); desktop notification + âm; automations theo lịch do daemon chạy |
| Usage | Meter Claude/Codex 5h + tuần, cảnh báo limit; RAM Kelpie; nhiều account Claude/Codex |
| Điều phối | Runs: task + dependency + approval gate + worktree/task, AI lập kế hoạch, agent báo cáo bằng `kelpied run report` |
| Tích hợp | Linear, Jira, GitLab (token trong keyring); browser pane + design mode |
| Remote | Remote project qua SSH (tmux); SSH pane trong mọi project; SSH hosts kiểu Termius (group/tag/key/known hosts/port forward/SFTP/multi-host command) |
| Khác | Phone access (QR, LAN/Tailscale, HTTPS tùy chọn); voice Whisper offline; plugin khai báo; EN/VI; scripting `kelpied terminal/worktree/run/call` |

### 2. Bối cảnh thị trường 2026

| Tool | Điểm đáng chú ý | So với Kelpie |
|---|---|---|
| Conductor | macOS native, **diff review panel** mỗi workspace, worktree/agent; Pro $50/th có cloud + multiplayer; Windows chỉ waitlist | Kelpie thắng Windows, độ bền phiên; **thua ở review/merge flow theo workspace** |
| Claude Squad | TUI trên tmux, nhanh cho solo 1 repo | Kelpie vượt trội |
| Vibe Kanban | Board đa agent; maker đóng cửa 04/2026, community duy trì | Cơ hội: người dùng VK cần chỗ mới; Kelpie đã có board |
| Crystal | Trạng thái deprecated/không rõ | — |
| Parallel Code, Nimbalyst, Superset, Paseo | Native app macOS/Linux | Kelpie thiếu **Linux** |

Xu hướng chung: isolation = git worktree gần như tuyệt đối; khác biệt nằm ở **review, merge, chi phí, và độ tin cậy**.

### 3. Nỗi đau thực tế của vibe coder chạy nhiều agent
1. Worktree không cô lập `.env`, `node_modules` → Kelpie **đã giải** (copy/link).
2. **Xung đột ngữ nghĩa**: merge sạch nhưng mâu thuẫn ý nghĩa → cần chạy check trên kết quả merge, không chỉ từng nhánh.
3. **Stash dùng chung giữa worktree** → agent này pop stash của agent kia.
4. Tài nguyên: RAM nghẽn ở ~4–5 agent, CPU khi build/test 6–10, context ở 10+.
5. **Hết usage limit giữa chừng** → việc dừng nửa vời.
6. Agent tự review code của mình bỏ sót lỗi giống lúc viết → **cross-review bằng agent khác**.
7. Hiệu suất thấp mỗi agent (1/4 lời giải dùng được) → cần **so sánh nhanh nhiều lời giải** (best-of-N).
8. MCP state lẫn giữa các agent dùng chung server.
9. Chi phí tăng gần bậc hai theo độ dài hội thoại → cần **thấy context % liên tục** để compact/hand-off đúng lúc.
10. Không ngồi trước máy → cộng đồng tự chế ntfy/Telegram hook cho "done" và Allow/Deny từ xa.

### 4. Gap analysis — Kelpie còn thiếu gì

Bằng chứng code: grep toàn repo (app/src, app/src-tauri/src, crates) cho các khái niệm dưới đây.

| # | Thiếu | Bằng chứng | Mức đau | Ưu tiên |
|---|---|---|---|---|
| G1 | **Checkpoint / diff / hoàn tác theo lượt agent** | 0 kết quả `checkpoint\|rollback`; Changes panel chỉ diff toàn worktree | Rất cao: nhiều agent cùng folder, không biết ai sửa gì; chỉ Claude có `/rewind` | P0 |
| G2 | **Context % / token / cost theo phiên + tổng hợp ngày/tuần** | 0 `context window\|input_tokens`; cost chỉ hiện ở chat view cuối lượt | Cao: quyết định compact/hand-off, kiểm soát tiền API | P0 |
| G3 | **Finish worktree**: check → merge/squash/rebase cục bộ → dọn; cảnh báo file chồng giữa worktree | `git_sync.rs` chỉ phục vụ PR; không có merge-back | Cao: vòng đời worktree đứt ở bước cuối | P0 |
| G4 | **Push ra ngoài máy** (ntfy / Telegram / webhook), trả lời "needs you" từ xa | 0 `ntfy\|telegram\|webhook`; phone access cần mở trang web | Cao với task dài | P0 |
| G5 | **Limit reached → hẹn resume lúc reset / chuyển account / hand-off** | Meter đã báo limit ([usage-meter.tsx:144](../../app/src/components/usage-meter.tsx#L144)), status `failed` có lý do rate limit ([status.rs:45](../../crates/kelpied/src/status.rs#L45)) nhưng không hành động | Cao | P0 |
| G6 | **Auto-update** | Không có `tauri-plugin-updater`; installer chỉ là CI artifact | Rất cao cho app phát hành | P0 |
| G7 | **Cross-review & best-of-N** (1 click gửi diff cho agent khác review; 1 prompt → N agent/N worktree → so sánh) | Có review notes, hand-off, runs nhưng không có flow ghép | Trung bình-cao | P1 |
| G8 | **Quản lý MCP server & file hướng dẫn** (CLAUDE.md / AGENTS.md / GEMINI.md) cho nhiều agent | mcp chỉ xuất hiện ở adapter chat/accounts | Trung bình-cao (cấu hình rời rạc theo agent) | P1 |
| G9 | **Tiến trình & port do phiên sinh ra** (dev server, process mồ côi, RAM/CPU theo phiên) | 0 phát hiện port; `proc_tree.rs` có sẵn cây tiến trình | Cao (ghost process, port bận) | P1 |
| G10 | Git cơ bản: log/graph, đổi branch, stash list | 0 `stash\|blame`, không có log/checkout | Trung bình | P2 |
| G11 | **Linux build** | README chỉ Windows/macOS | Trung bình (dev Linux đông) | P1 |
| G12 | Export/import cấu hình (settings, quick commands, hosts không secret, layouts, automations) | Chỉ có backup tự động `app-state.json` | Thấp-trung bình (nhiều máy) | P2 |
| G13 | Chất lượng mã: `store.ts` 1407 dòng; không unit test FE (chỉ e2e Python) | wc, không có `*.test.*` | Rủi ro bảo trì | P2 |
| G14 | SSH: đóng pane rò tmux, chưa auto-reconnect, thiếu danh sách "đang chạy trên server" | [research-261023-0003](research-261023-0003-ssh-session-persist-resume.md) | Cao với user SSH | Đã có research riêng — plan riêng |

### 5. Đề xuất ưu tiên

Thiết kế cho từng gap — chi tiết thực thi trong plan.

- **G1 Checkpoint**: tại `UserPromptSubmit` (Claude/Codex hook) hoặc khi người dùng gửi Enter (agent không hook), daemon chụp snapshot **không chạm working tree**: `GIT_INDEX_FILE=<tmp> git add -A && git write-tree && git commit-tree` → `refs/kelpie/cp/<session>/<n>`. UI: dòng thời gian lượt trong pane, diff lượt N, "Hoàn tác lượt này" (`git restore --source=<cp> --worktree -- <files>` chỉ các file lượt đó đổi), và "agent nào chạm file nào" khi nhiều phiên chung folder. Dọn ref khi đóng phiên + giữ tối đa N.
- **G2 Context & cost**: tail transcript JSONL **tăng dần theo offset** (đã có `open_lines(path, offset, …)` trong [agent_history.rs:334](../../app/src-tauri/src/agent_history.rs#L334)). Claude: `message.usage` (input + cache_read + cache_creation + output) → context dùng / cửa sổ của model; Codex: sự kiện `token_count`. Hiển thị vòng % trên tab + sidebar, ngưỡng 70/85 % đổi màu, gợi ý "Compact / Hand-off". Cost = bảng giá theo model (file JSON cập nhật kèm app). Trang Usage thêm biểu đồ ngày/tuần/theo project (giống ccusage) dựa trên cache scan sẵn có. *Không* chiếm statusLine của Claude (người dùng thường có statusline riêng).
- **G3 Finish worktree**: `.kelpie.yaml` thêm `check:` (vd `pnpm test`). Nút **Finish**: chạy check trong worktree → chọn Squash / Merge / Rebase vào base (trong main checkout, dùng `git merge-tree --write-tree` để biết trước xung đột) → chạy lại check trên kết quả (chống xung đột ngữ nghĩa) → xóa worktree + branch. **Overlap watch**: mỗi 30 s (chỉ khi có ≥2 worktree active) so `git diff --name-only base...branch` giữa các worktree → chip cảnh báo "2 agent cùng sửa `src/api.ts`".
- **G4 Push ra ngoài**: provider ntfy (topic + server tự host tùy chọn), Telegram bot, webhook JSON. Daemon gửi (để chạy cả khi app đóng), dựa trên cùng sự kiện inbox; lọc: needs-you / done / failed / automation; chỉ khi phiên chạy > X giây. ntfy action "Open" deep-link phone access; Allow/Deny từ xa để giai đoạn 2 (rủi ro bảo mật).
- **G5 Limit**: khi phiên fail vì limit: hành động 1-click "Tiếp tục lúc reset (15:00)" (daemon hẹn gửi `continue`), "Chuyển account B" (resume cùng hội thoại với home khác — chỉ khi account cùng vendor), "Hand-off sang Codex".
- **G6 Auto-update**: `tauri-plugin-updater` + chữ ký minisign, `latest.json` trên GitHub Releases; kiểm tra lúc mở + mỗi 6 h, cài khi người dùng đồng ý; **daemon được restart có kiểm soát** (phiên trở thành interrupted → resume flow sẵn có).
- **G7 Cross-review / best-of-N**: menu worktree "Review bằng…" gửi `git diff base...HEAD` + prompt review vào agent khác (tái dùng `chooseAgentAndSend`). Best-of-N = template Run: 1 prompt × N agent × N worktree, trang so sánh (số file, +/-, check pass, diff cạnh nhau) → "Chọn bản này" → Finish (G3), dọn phần còn lại.
- **G8 Agent config**: trang "Agent setup" theo project/global: MCP servers đọc từ `.mcp.json`, `~/.claude.json`, `~/.codex/config.toml`, `~/.gemini/settings.json`, `opencode.json`; bật/tắt, sao chép server sang agent khác (chuyển định dạng); file hướng dẫn: sửa CLAUDE.md, tùy chọn đồng bộ AGENTS.md/GEMINI.md (symlink hoặc copy). Ghi file có backup + diff xem trước.
- **G9 Processes & ports**: daemon đã biết PID gốc từng phiên → dùng `proc_tree` + bảng TCP listen (Windows `GetExtendedTcpTable`, macOS `lsof`/libproc) mỗi 5 s **chỉ cho phiên có tiến trình con**. Chip "`:5173`" trên phiên → mở browser pane. Panel "Processes": cây tiến trình theo phiên, RAM/CPU, Kill; cảnh báo tiến trình còn sống sau khi phiên đóng (orphan).
- **G10–G13**: log/checkout/stash list tối thiểu; Linux AppImage/deb trong CI; export/import JSON (loại secret); tách `store.ts` theo miền + vitest cho lib thuần (`layout.ts`, parser).

### 6. Ràng buộc hiệu năng & bảo mật
- Mọi tác vụ mới chạy **ngoài UI thread** (daemon hoặc Rust core, tokio blocking pool); UI chỉ nhận số đã tổng hợp qua store fine-grained.
- Checkpoint: debounce, bỏ qua repo > ngưỡng file (tùy chỉnh), tôn trọng `.gitignore`; đo `git add -A` trên repo lớn trước khi bật mặc định. Mục tiêu < 150 ms p95 trên repo 10k file.
- Transcript tail: đọc từ offset cuối, không parse lại; chỉ phiên đang hiện hoặc đang working.
- Port scan: 1 syscall bảng TCP/5 s, lọc theo PID trong cây phiên; tắt khi không có phiên nào có con.
- Push: token Telegram/ntfy trong keyring (như integrations); không gửi nội dung terminal trừ khi bật; mặc định chỉ "tên phiên + trạng thái".
- Updater: bắt buộc chữ ký; không tự cài khi agent đang working.
- MCP manager: chỉ ghi file khi người dùng xác nhận diff; backup trước.
- Merge-back: không bao giờ force; refuse khi main checkout dirty.

### 7. Không làm (non-goals)
- Tự xây agent/LLM chat bằng API key riêng; marketplace MCP/skills; cloud sync có server; telemetry; container sandbox per agent (để sau, khi có nhu cầu đo được); Allow/Deny từ xa qua Telegram trong vòng đầu.

## Implementation Recommendations

### Quick Start
1. Làm G6 (auto-update) sớm: mọi cải tiến sau đến được người dùng.
2. G2 → G1 → G3 (cùng chạm transcript/git; giá trị cao nhất mỗi ngày).
3. G4 + G5 (cùng hạ tầng sự kiện daemon).
4. G9, G7, G8, G11; cuối cùng G10, G12, G13.

### Code Examples
Snapshot không chạm working tree / index của người dùng:
```bash
tmp=$(mktemp); export GIT_INDEX_FILE="$tmp"
git read-tree HEAD && git add -A
tree=$(git write-tree)
cp=$(git commit-tree "$tree" -p HEAD -m "kelpie checkpoint s3 t7")
git update-ref "refs/kelpie/cp/s3/7" "$cp"; rm -f "$tmp"
git diff --name-only refs/kelpie/cp/s3/6 refs/kelpie/cp/s3/7   # file lượt 7 đổi
```
Biết trước xung đột merge mà không chạm working tree (git ≥ 2.38):
```bash
git merge-tree --write-tree --name-only base-branch agent-branch   # exit 1 = có xung đột
```

### Common Pitfalls
- Snapshot bằng `git stash create` bỏ sót file untracked → dùng index tạm.
- Đếm context Claude phải cộng cả `cache_read_input_tokens` + `cache_creation_input_tokens`.
- Đừng ghi đè statusLine/hook của người dùng; Kelpie đã ghi hook qua settings riêng mỗi phiên ([agents.rs:358](../../crates/kelpied/src/agents.rs#L358)) — giữ nguyên cơ chế đó.
- Updater thay binary daemon khi phiên đang chạy → phải đi qua resume flow, không kill thô.

## Resources & References
- [Best Tools for Managing Parallel AI Coding Agents in 2026 — DEV](https://dev.to/stravukarl/best-tools-for-managing-parallel-ai-coding-agents-in-2026-14l8)
- [9 Open-Source Agent Orchestrators (2026) — Augment](https://www.augmentcode.com/tools/open-source-agent-orchestrators)
- [Best AI Agent Orchestration Tools 2026 — crystl.dev](https://crystl.dev/blog/best-ai-agent-orchestration-tools/)
- [Best multi-agent coding tools — agentsroom.dev](https://agentsroom.dev/blog/best-multi-agent-coding-tools)
- [Abralo alternatives comparison](https://abralo.com/alternatives)
- [Parallel Code vs Vibe Kanban](https://parallelcode.app/compare/parallel-code-vs-vibe-kanban/)
- [Running Claude Code and Codex in parallel: worktree gotchas and cross-review — DEV](https://dev.to/offrun/running-claude-code-and-codex-in-parallel-on-one-repo-git-worktree-gotchas-and-cross-review-41eg)
- [What worktrees cannot see — DEV](https://dev.to/naw103/running-multiple-claude-code-agents-in-parallel-what-worktrees-cannot-see-4le7)
- [Git Worktrees + Claude Code 2026 — developersdigest](https://www.developersdigest.tech/blog/git-worktrees-claude-code-parallel-agents-guide)
- [Claude Code token usage — morphllm](https://www.morphllm.com/claude-code-token-usage)
- [Claude Code statusline for token burn — Andrew Connell](https://www.andrewconnell.com/articles/claude-code-cli-statusline/)
- [Oh My Posh Claude segment](https://ohmyposh.dev/docs/segments/cli/claude)
- [AgentX (MCP/skills across agents)](https://pkg.go.dev/github.com/agentsdance/agentx)
- [ln-004-agent-sync skill](https://playbooks.com/skills/levnikolaevich/claude-code-skills/ln-004-agent-sync)
- [claude-remote-approver (ntfy)](https://github.com/yuuichieguchi/claude-remote-approver) · [claude-push](https://github.com/coa00/claude-push) · [mcgram](https://pypi.org/project/mcgram/) · [Telegram + Claude Code hooks](https://medium.com/@dan.avila7/step-by-step-guide-connect-telegram-with-claude-code-hooks-1686fadcee65)
- Nội bộ: [SSH persist research](research-261023-0003-ssh-session-persist-resume.md), [Orca analysis](orca-analysis-261020-0132-features-architecture.md)

## Unresolved questions
1. Checkpoint bật mặc định hay opt-in? Phụ thuộc số đo `git add -A` trên repo lớn nhất người dùng có.
2. Bảng giá model cho cost: nhúng JSON tĩnh theo bản phát hành hay cho phép tải bản mới? (tải = thêm 1 network call).
3. Phát hành ở đâu cho updater: GitHub Releases public hay private? Cần khóa ký minisign.
4. Linux: ưu tiên AppImage hay .deb? Có người dùng Linux thực tế không?
5. G14 SSH: gộp vào plan này hay plan riêng (research đã xong)? Plan này giả định plan riêng.
6. Telegram Allow/Deny từ xa có cần trong vòng đầu không (rủi ro: ai có bot token là duyệt được lệnh)?
