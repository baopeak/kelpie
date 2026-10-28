# Chat view kiểu VS Code + lỗi chập chờn: kết quả

Ngày 2026-10-23 · nhánh `rebrand/kelpie` (chưa commit) · Biên bản tranh luận 5 agent: [synthesis-261023-1452](synthesis-261023-1452-chat-view-ui-debate.md) · Review: [code-reviewer-261023-1540](code-reviewer-261023-1540-chat-ui-review.md)

## 1. Lỗi chập chờn — nguyên nhân thật (đã kiểm chứng)

| Lỗi | Nguyên nhân | Bằng chứng | Sửa |
|---|---|---|---|
| Claude resume "không nhớ hội thoại" | Test gõ câu hỏi khi Claude **chưa vẽ màn hình** (resume mất 8 s lúc máy rảnh, lâu hơn khi bận); màn hình trống cũng "đứng yên" nên test tưởng đã sẵn sàng → chữ in trên banner rồi bị nuốt | Dump màn hình: câu hỏi nằm *trên* banner Claude; trước: lỗi 6/6 (rảnh và tải) | Test chờ màn hình đã vẽ + xác nhận đã nhận câu hỏi, gõ/Enter lại nếu chưa. Sau: Claude 6/6 đạt; 1 lần có tải cần Enter lần 2 (đúng race CPU) |
| Codex resume "No saved session found" | **Lỗi sản phẩm**: Codex chạy thread nền riêng (gpt-5.6-luna, skill selection) cũng gọi `notify`; Kelpie lấy id thread nền (không được lưu) làm id hội thoại → restart resume thất bại | Rollout `01a167be…` tồn tại, id hook `01a167bf…` (+50 s) chỉ có trong log Codex với model nền | `kelpied hook` chỉ nhận thread-id có rollout đã lưu (ngày lấy từ UUIDv7). Sau: Codex resume đúng hội thoại; `codex exec resume` trả lời đúng |
| Codex trả lời chậm sau resume | Hook UserPromptSubmit/Stop của user chạy ~7 s, màn hình đứng yên | Probe: trả lời sau 7 s | Test chờ câu trả lời tới 180 s |
| Codex "I can't provide that." | Model GPT-6-Luna từ chối nhắc lại "secret number" | Màn hình lỗi; probe dùng "the number" trả lời đúng | Đổi câu chữ test. Sau: Codex 7/7 |
| parity: xoá worktree để lại thư mục | **Lỗi sản phẩm**: `unwatch` tới khi watcher còn đang tạo (chậm khi máy bận) → watcher vẫn được thêm, giữ thư mục mãi | Đọc code: `watch()` thêm vào map sau khi tạo xong, không khoá | Ticket huỷ watcher đang tạo; xoá worktree chờ unwatch; chờ tới 3 s cho thư mục "đang chờ xoá". parity 3/3 × 74/74 |
| "restored as interrupted" khi máy bận (2/6) | Giả thuyết: `taskkill /T` giết từng tiến trình, daemon kịp ghi "đã thoát" | Chưa kiểm chứng | Chưa sửa |

## 2. Chat view (đồng thuận 5/5 agent)

Dòng tool có động từ + file + chi tiết, IN/OUT, diff dòng thật (+/−); gộp đọc/tìm; "Thought for Ns"; hàng đợi tin nhắn (chip, Send now = steer Codex / ghi giữa lượt Claude, dừng = giữ hàng đợi); kế hoạch ghim (Codex/ACP; Claude `-p` 2.1.309 không có tool todo); vòng context + chi phí, bấm để compact; thẻ duyệt có lệnh/diff/plan, phím 1–9, từ chối kèm lời nhắn, AskUserQuestion & ExitPlanMode trả lời trong thẻ; thẻ "N files changed" mở diff; pill "Waiting for your approval"/"Jump to latest"; thời gian chạy; Shift+Tab đổi mode (không vào full-access/bypass); **ngày giờ góc phải mỗi prompt và phản hồi**, giữ giờ gốc khi phát lại (daemon chèn `kelpie_time`).

## 3. Kiểm thử

| | Kết quả |
|---|---|
| vitest app | 86/86 |
| kelpied unit | 54/54 |
| chat_ui (mới, Claude + Codex thật) | 16/18 (Codex: sandbox lỗi với thư mục 8.3 + MCP planner của user treo; test đã đổi sang đường dẫn dài) |
| chat_parity / chat / parity ×3 | 31/31 · 8/8 · 74/74 ×3 |
| Claude TUI trong app (gõ prompt, thu hẹp/giãn pane, ẩn/hiện tab khi đang trả lời) | 5/5 ×2, không có khung nhập kẹt |

## Câu hỏi còn mở

- "Khung nhập bị đẩy lên" user thấy: chưa tái hiện được (pane hẹp, đổi kích thước, ẩn/hiện). Ảnh của user đang cuộn lên ("Back to bottom"); cần biết thao tác ngay trước đó.
- M2 review (steer bị từ chối chạy sau tin kế tiếp) và các mục Low còn lại.
- Ảnh đính kèm trong composer; checkpoint/rewind (theo phase-03).
