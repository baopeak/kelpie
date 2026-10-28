# Code review: chat UI parity, queue, watcher race, Codex hook

Ngày 2026-10-23 · reviewer độc lập (code-reviewer agent) · phạm vi: `app/src/chat/*`, `chat-rows/pane/composer.tsx`, `styles.css` (chat), `watcher.rs`, `worktrees.ts`, `agents.rs` `codex_thread_saved`, `main.rs` `hook()`, `scripts/e2e/agent_resume.py`.
Typecheck sạch; vitest chat 43/43 lúc review (sau sửa: 86/86 toàn app).

## Phát hiện → xử lý

| # | Mức | Phát hiện | Xử lý |
|---|---|---|---|
| H1 | Cao | Phím số / pill "đang chờ duyệt" tác động vào thẻ duyệt cũ (log replay không có câu trả lời; Claude interrupt; Codex interrupt) | Thẻ mở bị đánh dấu "Không còn cần" khi lượt kết thúc (`expireApprovals` khi busy→false, cả lúc replay); Claude `control_cancel_request` → "Cancelled"; phím số chỉ cho thẻ live (`at`) mới nhất của lượt đang chạy. Test. |
| H2 | Cao | Codex `/new` gõ trước khi thread mở → `busy` kẹt, hàng đợi đứng | Kiểm tra lượt chạy bằng `turnId` thay vì `busy`; xả hàng đợi trước-thread thì tắt `busy` trước. |
| M1 | TB | "Send now" một `/lệnh` / `!cmd` đang chờ → bị steer thành văn bản | Lệnh không bao giờ steer: dừng lượt rồi gửi kế tiếp. Test. |
| M2 | TB | Steer bị từ chối có thể chạy sau tin kế tiếp | Chưa sửa (hiếm: cần `turn/completed` tới trước lỗi steer). Ghi nhận. |
| M3 | TB | Shift+Tab nhảy sang full-access / bypass; regex `/mode/` khớp cả `model` | Bỏ full-access/bypass khỏi vòng; chỉ setting `mode`. |
| M4 | TB | ACP Stop không trả lời permission đang mở → agent có thể không kết thúc lượt | `interrupt()` trả `{outcome: cancelled}` cho mọi request mở, đánh dấu thẻ. |
| M5 | TB | Hàng đợi mất/treo: sendNow sau exit; chip chết; gửi khi chưa attach bị replay xoá | Chặn sendNow khi exited/!ready; exit → báo "Chưa gửi" + xoá chip; gửi khi !ready → vào hàng đợi. Test. |
| L | Thấp | `projectRelative` nhận `../` | Từ chối `..`, bỏ `./`. |
| L | Thấp | `lineDiff` O(n·m) mỗi Edit | Cắt phần đầu/cuối trùng trước LCS (50k dòng, đổi 1 dòng: < 500 ms, test). |
| L | Thấp | `unwatch` drop watcher trong lock | Drop sau khi nhả lock. |
| L | Thấp | Codex v2 file-change approval hiện tên method | Lấy diff từ item `itemId`. |
| L | Thấp | id hàng đợi có thể trùng | Bộ đếm. |
| L | Thấp | `diffStats` bỏ `+++`/`---` thật; `unifiedFiles` không khớp path có ngoặc kép; stableEnd `~~~` trong ```; ExitPlanMode bỏ lời nhắn khi chọn allow; hook vẫn chuyển status của thread nền | Chưa sửa (ảnh hưởng nhỏ). |

## Câu hỏi còn mở

1. Claude stream-json nhận tin giữa lượt: chèn vào lượt hay chạy lượt riêng? (nếu riêng: hàng đợi xả sớm — vô hại)
2. Codex `thread/shellCommand` có phát `turn/started/completed`?
3. Codex `turn/steer` được nhận nhưng chưa dùng khi lượt kết thúc thì sao?
4. Codex `FileUpdateChange.diff` cho file mới có phải nội dung thô?
5. Claude 2.1.x `-p` có TodoWrite? → **Đã trả lời trong phiên: không** (chỉ `Task`, `TaskStop`; TodoWrite/TaskCreate không có) → bảng kế hoạch Claude chỉ hiện với bản còn TodoWrite.
6. Đường dẫn turn-diff Codex tương đối với git root, project là thư mục con → mở nhầm file.
