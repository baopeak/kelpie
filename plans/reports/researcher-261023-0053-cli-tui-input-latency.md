# Research Report: Input/typing lag của coding-agent CLI trong session dài

Timestamp: 2026-10-23 00:58 (Asia/Saigon) · Nhánh: `rebrand/kelpie` · Loại: research (chưa code) · 14 lượt search/fetch

## Executive Summary

- **Lag nằm trong process CLI, không ở terminal.** Bằng chứng mạnh nhất: Claude Code #53166 — main thread 99% idle, terminal emulator rảnh, session 58 giờ gõ chậm 200–500 ms; `/exit` rồi `claude` mới → hết lag ngay trong cùng terminal.
- **Giả thuyết phổ biến (chưa maintainer xác nhận):** Ink/React re-render toàn bộ cây component mỗi frame/keystroke, chi phí tăng theo history/tool calls/context. Không issue nào có số đo profile thật.
- **Trim display phía host gần như vô dụng** — xoá scrollback (`ESC[3J`), giảm scrollback xterm/alacritty không giảm lag vì lag nằm ở render tree của CLI. Chỉ lợi nhẹ về RAM/bytes.
- **"Auto-trim display" chính thức duy nhất nằm trong CLI:** Claude Code fullscreen rendering (alt-screen, chỉ giữ message đang nhìn thấy trong render tree — "memory stays constant regardless of conversation length"). Gemini CLI / Qwen Code có virtual viewport tương tự.
- **Đòn bẩy thật cho Kelpie:** đo keystroke-to-echo trước; bật fullscreen cho Claude Code (A/B trên Win10); nhắc `/compact`/`/clear` theo ngưỡng; relaunch process khi lag.

## Phương pháp

- Nguồn: official docs `code.claude.com` (fullscreen, sessions, env-vars); GitHub issues Claude Code (+ mirror claudeissues.com) #18943, #29366, #31194, #53166, #14466, #58498, #22265, #14461; Qwen Code design doc (virtual viewport); blog Codex v0.145 (bên thứ ba); Chad Austin về terminal latency Windows; bài pi-tui (bên thứ ba).
- Search terms: "Claude Code input lag long session", "CLAUDE_CODE_NO_FLICKER fullscreen", "Gemini CLI input lag Ink Static", "Codex CLI TUI input lag", "opencode TUI input lag", "oh-my-pi pi-tui differential rendering", "ConPTY xterm.js Windows 10 vs 11".
- Hạn chế: không fetch trực tiếp được repo gemini-cli, openai/codex, opencode, oh-my-pi → thiếu số issue cho 4 CLI này. Nhiều issue Claude Code bị bot đóng "not planned"/duplicate, không phản hồi maintainer.

## Nguyên nhân theo từng CLI

### Claude Code (React + Ink) — evidence mạnh nhất

| Triệu chứng | Issue | Nguyên nhân (độ tin cậy) |
|---|---|---|
| Gõ chậm khi context ~80%+, regression từ 2.0.x (báo ở 2.1.6) | #18943 | Chưa xác định. Giả thuyết: token counting mỗi keystroke, status line/context% chặn input loop, hooks/MCP/subagents, memory pressure — **chưa kiểm chứng**. Có báo 600 MB+ RSS, CPU 20–40%. |
| Session 58 h, 809 tool calls, 82% context, gõ trễ 200–500 ms (v2.1.119) | #53166 | Re-render full tree mỗi keystroke (**giả thuyết**). Bằng chứng mạnh: process mới hết lag, main thread 99.5% idle, terminal không phải thủ phạm. Bot đóng là duplicate #18943. |
| Lag từ lúc mở ~30 s (v2.1.62, WSL2 + Windows native) | #29366 | Debug log ~536 re-render/32 s (~30 fps), mỗi frame `blit=0, write=1012` (full rewrite). Giả thuyết reporter: "fast mode" trigger render loop. |
| Lag tăng theo tải/độ dài session, Backspace tệ nhất (v2.1.63) | #31194 | Reporter: kiến trúc rebuild cả screen buffer mỗi frame, không framerate control. Máy test yếu. |
| Lag tích luỹ qua SSH, cuối cùng crash React error #185 | #14466 | `/clear` giảm tạm thời. |
| Input freeze tới 20 s sau `/compact` rồi đổ ra một lúc | #58498 | Chỉ đọc được tiêu đề. |
| Lag khi API chậm | #14461 | Event loop bị chặn khi chờ API (theo reporter). |

Anthropic xác nhận gián tiếp qua docs fullscreen: classic renderer "sends a lot of data to the terminal each update", memory không phẳng trong conversation dài → fullscreen sinh ra để sửa. Docs **không** hứa giảm latency gõ, chỉ flicker/memory/throughput.

**Lưu ý Windows (liên quan Kelpie):** docs ghi "Windows Terminal and other ConPTY-backed hosts coalesce these positioned writes incorrectly" → stale text; workaround `CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT=1` (repaint toàn bộ cell mỗi frame, nhiều bytes hơn). Chế độ này tự bật cho background session/agent view trên Windows. "SSH tới Claude Code chạy trên Windows" mặc định Classic → gợi ý Anthropic chưa tin fullscreen qua ConPTY (suy luận, chưa kiểm chứng).

### Codex CLI (Rust + ratatui)

- Không xác nhận được issue TUI cụ thể. Blog bên thứ ba (codex.danielvaughan.com, 2026-07-26) nhắc #18693, #9290 và v0.145.0 chuyển sang incremental Markdown rendering (cache segment theo hash) + giới hạn command output hiển thị — **nhiều claim không nguồn, coi là chưa đáng tin**.
- Chắc chắn: không có Node event loop → lag kiểu "re-render cây React" ít khả thi; "re-render toàn bộ Markdown mỗi frame" vẫn có thể (chưa độc lập xác nhận).

### Gemini CLI (Ink fork)

- Không tìm được issue cụ thể. Qwen Code design doc: Gemini CLI dùng `@jrichman/ink@6.6.9` (fork có ResizeObserver, StaticRender) + virtualized list; Qwen port lại.
- Cơ chế gốc theo Qwen doc: `<Static>` của Ink append-only nhưng nếu feed cả history mỗi render → 1000 turn = 1000 React render + layout pass mỗi thay đổi state.

### OpenCode, OMP (oh-my-pi)

- Không có nguồn xác nhận lag, không số issue.
- OMP/pi-mono: pi-tui là differential renderer line-level + CSI 2026 synchronized output (nguồn bên thứ ba; "sub-millisecond" là marketing).
- OpenCode: OpenTUI (Zig + SolidJS) — **chưa verify**.

## Việc user tự làm được (trong CLI)

**Claude Code (verified từ docs):**
- `/clear` (conversation mới, cũ vẫn `/resume` được); `/compact [hướng dẫn]` thay history bằng summary; `/context` xem cái gì chiếm context.
- `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` (1–100): chỉ hạ được ngưỡng auto-compact.
- `/tui fullscreen` hoặc `CLAUDE_CODE_NO_FLICKER=1` (v2.1.89+; env bắt buộc trước v2.1.110) — "keeps memory usage flat in long conversations"; relaunch tại chỗ, giữ conversation. Từ 5/2026 user mới có thể mặc định fullscreen (tuỳ feature flag/điều kiện máy).
- `CLAUDE_CODE_DISABLE_VIRTUAL_SCROLL=1` ngược lại — render mọi message (chỉ dùng khi scroll trống).
- `CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1`, `CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false`, `CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY=1`, `CLAUDE_CODE_DISABLE_MOUSE=1`.
- `/focus`: view tối giản (prompt cuối + tóm tắt tool + câu trả lời cuối) — chưa rõ giảm render cost.
- Thử bỏ custom `statusLine` (#18943 nghi ngờ, chưa chứng minh).
- Workaround cộng đồng: `/exit` → `claude --continue`.
- **Không có** option chính thức "giới hạn số message hiển thị" ngoài fullscreen/virtual scroll. Phần cuối trang env-vars chưa đọc hết.

**Codex, Gemini, OpenCode, OMP:** chưa đủ nguồn về option riêng. Hướng chung: `/compact`, `/new`/`/clear`, nâng cấp bản mới.

## Việc host terminal làm được (đánh giá thật)

| Biện pháp | Giảm lag gõ? | Ghi chú |
|---|---|---|
| Xoá scrollback (`ESC[3J`) / giảm scrollback xterm.js, alacritty 5000 | **Gần như không** | Lag ở process CLI. Chỉ tiết kiệm RAM host; rủi ro mất history. CLI ở alt-screen (Claude fullscreen) vốn không dùng scrollback host. |
| Cắt/ẩn output cũ ở webview | **Không** | CLI vẫn giữ và re-render cây cũ. Trim ở host còn gây lệch. |
| xterm.js WebGL vs DOM | Ít | Giữ WebGL; chỉ ảnh hưởng tốc độ vẽ. |
| Synchronized output (DEC 2026) | Chống flicker, không giảm lag | Claude probe 2026 lúc khởi động. Kelpie cần kiểm xterm.js + kelpied trả lời query này đúng (**chưa kiểm**). |
| Kitty keyboard protocol | Không liên quan | Dùng bất cẩn có thể gây nhiễu shell. |
| ConPTY Win10 vs Win11 | **Không có benchmark** | Windows Terminal 1.19 cắt latency một nửa (Chad Austin); ConPTY pass-through mới nhanh hơn. Nghi ngờ Win10 cũ chậm với output nhiều bytes (full repaint) — chưa xác nhận. |
| Windows Defender quét `node.exe` | **Không có nguồn** | Ảnh hưởng startup/IO, ít khả năng ảnh hưởng từng phím. |
| Coalesce 4 ms trong kelpied | Thêm ≤ ~4 ms | Không đáng kể so với 200–500 ms; cần đo. |
| `alacritty_terminal` parse mọi byte | Có thể tốn CPU khi CLI full repaint (~30 fps × 1000 write như #29366) | Rust nên rẻ; **chưa đo**. |

## Đề xuất cho Kelpie

Đo trước → biện pháp rẻ → biện pháp can thiệp.

| # | Kỹ thuật | Impact | Effort | Risk | Ghi chú |
|---|---|---|---|---|---|
| 1 | **Đo keystroke-to-echo** trong kelpied (t ghi byte vào PTY → t byte output đầu tiên chứa echo) | Quyết định mọi thứ | Thấp–TB | Thấp | Cô lập lag CLI vs pipeline Kelpie. Bắt buộc trước khi tối ưu. |
| 2 | Gợi ý / nút "Compact / Clear" khi session dài (heuristic thời gian, token, số tool call) | TB–Cao | Thấp | Thấp | Chỉ gửi khi user bấm; tự gửi có thể chèn vào prompt đang gõ. Hiệu quả tạm thời (#18943). |
| 3 | Launch Claude Code với `CLAUDE_CODE_NO_FLICKER=1` (tuỳ chọn theo CLI) | Có thể Cao (Claude) | Thấp | TB | Giảm bytes/memory; chưa có bằng chứng giảm latency gõ. Rủi ro ConPTY stale text (cần `ALT_SCREEN_FULL_REPAINT=1`), mất native scrollback/selection. A/B trên Win10 19045. |
| 4 | Nút "Restart CLI giữ conversation" (kill → `claude --continue`/`--resume <id>`) | TB–Cao với lag kiểu #53166 | TB | TB | Sau compaction resume chỉ nạp summary + phần sau (verified docs). Chưa compact thì cây render có thể lớn lại (**chưa kiểm**). Mất background tasks; phải truyền lại `--mcp-config`/`--settings`/`--add-dir`. Chỉ khi idle. |
| 5 | Native input box của Kelpie, gửi cả prompt bằng bracketed paste | Cao về cảm giác gõ | Cao | Cao | Gõ không qua CLI. Mất autocomplete slash/`@file`, vim mode, history của CLI; paste dài có thể bị collapse "[Pasted text]". Chỉ làm khi 1–4 không đủ. |
| 6 | Trim scrollback xterm/alacritty | Thấp | Thấp | TB | Không khuyến nghị trừ khi mục tiêu là RAM. |
| 7 | Tạm tắt parser `alacritty_terminal` khi tab ẩn | Thấp–TB (CPU) | TB | TB | Chỉ nếu đo thấy CPU kelpied cao. |
| 8 | Cảnh báo cập nhật CLI khi bản cũ có bug render đã biết | Thấp | Thấp | Thấp | Cần changelog xác nhận. |

**Trả lời thẳng:** "auto-trim display" từ phía Kelpie không hiệu quả — display không phải thứ chậm, process CLI mới chậm. Trim thật phải xảy ra *bên trong* CLI: fullscreen/virtual scroll, `/compact`, `/clear`, hoặc process mới.

## Nguồn tham khảo

- Claude Code fullscreen rendering: https://code.claude.com/docs/en/fullscreen
- Claude Code env vars: https://code.claude.com/docs/en/env-vars
- Claude Code sessions/resume: https://code.claude.com/docs/en/sessions
- #18943: https://github.com/anthropics/claude-code/issues/18943
- #29366: https://github.com/anthropics/claude-code/issues/29366
- #31194: https://github.com/anthropics/claude-code/issues/31194
- #53166: https://github.com/anthropics/claude-code/issues/53166
- #14466: https://claudeissues.com/issue/14466-input-lag-builds-up-over-time-in-ssh-sessions-eventually-crashes-with-react-erro
- #58498 (chỉ title): https://claudeissues.com/issue/58498-bug-input-invisible-cursor-frozen-for-up-to-20s-after-compact-then-dumps-at-once
- #22265: https://claudeissues.com/issue/22265-bug-command-line-tool-becomes-slower-and-slower-as-session-progresses-slowness-s
- #14461: https://claudeissues.com/issue/14461-bug-input-latency-severely-affected-by-api-response-times-typing-becomes-unusabl
- Qwen Code virtual viewport: https://qwenlm.github.io/qwen-code-docs/en/design/virtual-viewport/README/
- Blog Codex v0.145 (bên thứ ba, độ tin thấp): https://codex.danielvaughan.com/2026/07/26/codex-cli-v0145-terminal-rendering-pipeline-incremental-markdown-inline-visualization-performance/
- Windows terminal latency (Chad Austin): https://chadaustin.me/2024/02/windows-terminal-latency/
- pi-tui differential rendering (bên thứ ba): https://instagit.com/badlogic/pi-mono/pi-tui-differential-rendering-architecture/
- Flicker Claude Code 2.0.72: https://steipete.me/posts/2025/signature-flicker

## Câu hỏi chưa giải quyết

1. Classic renderer có render lại toàn bộ history sau `--resume`/`--continue`? Cần đo.
2. Fullscreen có giảm latency gõ, hay chỉ giảm bytes/memory? Docs không hứa.
3. Fullscreen trên ConPTY Win10 19045 có stale text không; `ALT_SCREEN_FULL_REPAINT=1` làm latency tốt hay xấu?
4. Issue/PR cụ thể cho Gemini CLI, Codex TUI, OpenCode, OMP — cần `gh issue list` trực tiếp.
5. Env vars còn lại của Claude Code (spinner, tips, reduced motion) — phần cuối trang env-vars chưa đọc.
6. Pipeline kelpied (coalesce 4 ms, parser alacritty) có làm chậm echo không — cần số đo.
7. Không có benchmark ConPTY Win10 vs Win11; không có nguồn về Defender + node.exe.
