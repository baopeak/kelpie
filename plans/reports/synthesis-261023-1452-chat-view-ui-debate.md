# Tổng hợp: 5 nghiên cứu độc lập + tranh luận về chat view

Ngày 2026-10-23 · 5 agent nghiên cứu độc lập, đọc chéo báo cáo của nhau, phản biện, bầu top 8.

| # | Mảng | Báo cáo |
|---|---|---|
| 1 | Claude Code VS Code extension (bundle 2.1.304 trên máy + docs) | [researcher-…-vscode-chat-ui-ideas](researcher-261023-1438-vscode-chat-ui-ideas.md) |
| 2 | Codex desktop app / IDE ext (schema app-server sinh tại máy, codex 0.163) | [researcher-…-codex-app-ui-ideas](researcher-261023-1438-codex-app-ui-ideas.md) |
| 3 | Cursor, Windsurf, Copilot agent mode | [researcher-…-cursor-windsurf-copilot-ui-ideas](researcher-261023-1438-cursor-windsurf-copilot-ui-ideas.md) |
| 4 | Zed (ACP), Cline, Roo, Kilo | [researcher-…-zed-cline-roo-ui-ideas](researcher-261023-1438-zed-cline-roo-ui-ideas.md) |
| 5 | App quản lý nhiều phiên agent (Conductor, opcode, Crystal, Claude Desktop…) | [researcher-…-agent-desktop-managers-ui-ideas](researcher-261023-1438-agent-desktop-managers-ui-ideas.md) |

## Đồng thuận (5/5 phiếu)

1. **Dòng công cụ có cấu trúc + gộp nhóm**: tiêu đề động từ + tên file (`Read app.ts (lines 75-80)`, `Edit x.rs +3 −1`, `Bash <mô tả>`), thân IN/OUT có "Show more", gộp chuỗi đọc/tìm liên tiếp ("Explored 5 files, 2 searches"), suy nghĩ gập "Thought for 4s".
2. **Hàng đợi tin nhắn hiển thị được** (chip, huỷ, gửi ngay) + Codex `turn/steer`. *Là sửa lỗi đúng đắn, không chỉ đẹp*: hiện Codex gọi `turn/start` giữa lượt, ACP bắn `session/prompt` chồng nhau (`codex-adapter.ts:388-398`, `acp-adapter.ts:173-181`).
3. **Thẻ tổng kết lượt**: "3 files changed +42 −7", bấm mở DiffView sẵn có (ở đó đã có ghi chú review → `notesPrompt`).
4. **Kế hoạch / todo ghim**: Claude `TodoWrite`, Codex `turn/plan/updated`, ACP `plan`. Hiện ACP biến mỗi lần cập nhật plan thành một mục "thinking" mới (lỗi), TodoWrite hiện JSON thô.
5. **Thẻ phê duyệt tốt hơn**: phím tắt số, từ chối kèm lời nhắn, xem diff/lệnh ngay trong thẻ, trả lời `AskUserQuestion` và duyệt `ExitPlanMode` ngay trong chat (hiện gần như hỏng).
6. **Vòng context/usage** ở thanh composer, ẩn khi agent không báo.

## Đa số (4/5)

7. Pill "↓ tin mới" / "đang chờ bạn duyệt", đồng hồ chạy khi agent làm việc, đồng bộ mode (ACP `current_mode_update` đang bị bỏ qua), Shift+Tab đổi mode.

## Bị gạt / hoãn (có lý do)

| Ý tưởng | Lý do |
|---|---|
| Accept/Reject từng file trước khi ghi (Cursor/Copilot) | Kelpie không chặn được việc ghi của agent → chỉ là cổng giả |
| Undo từng file, rewind riêng từng agent (`--rewind-files`, Codex `thread/revert`) | Flag Claude ẩn, không theo dõi Bash; Codex revert chỉ sửa lịch sử, không sửa file. Checkpoint đã có kế hoạch chung `plans/261023-0036-vibe-coder-gaps/phase-03` |
| Side chat, bản đồ subagent, worktree handoff, PR/CI chip, dashboard usage | Lớn, giao thức không hỗ trợ, hoặc đã có ở sidebar/inbox/git panel |
| Virtualization thật | 300 mục + `content-visibility` đủ; chỉ làm khi đo thấy chậm |
| Đính kèm ảnh | Thiếu thật (Codex bỏ ảnh âm thầm) nhưng L; để đợt sau |

## Tranh chấp dữ kiện đã giải quyết

| Câu hỏi | Kết luận | Bằng chứng |
|---|---|---|
| Codex có plan/diff/token/steer? | Có: `turn/plan/updated{plan[{step,status}]}`, `turn/diff/updated{diff}`, `thread/tokenUsage/updated{tokenUsage{last,total,modelContextWindow}}`, `turn/steer{expectedTurnId}` | schema sinh bằng `codex app-server generate-ts`, kiểm lại trong phiên này |
| Claude có context window? | Có: `result.modelUsage[model].contextWindow`; số đang dùng = `input + cache_creation + cache_read` của `assistant.message.usage` | chạy thử `claude -p` stream-json 2.1.309 |
| "60m" ở footer extension | Nhiều khả năng là đồng hồ prompt-cache 1h (usage báo `ephemeral_1h_input_tokens`) | suy luận, chưa thấy tận mắt |
| Định dạng trả lời AskUserQuestion | `updatedInput: {...input, answers: {[question]: "label1, label2"}}` | bundle extension 2.1.304 |
| Kelpie đã có hàng đợi? | Không (chỉ trước khi thread/session sẵn sàng) | đọc code adapter |

## Kế hoạch triển khai (đợt này)

1–7 ở trên. Ảnh, checkpoint/rewind: đợt sau (checkpoint theo phase-03).

## Câu hỏi còn mở

- ACP `usage_update`: có trong danh sách spec nhưng chưa thấy OMP phát → vòng context tự ẩn.
- Codex `/plan` (collaboration mode) chưa xác minh trong schema → vẫn chuyển terminal.
