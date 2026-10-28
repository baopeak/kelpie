# Audit + kiểm thử: độ trễ gõ phím và chat view parity

Ngày: 2026-10-23 08:30–14:20 (Asia/Saigon) · Nhánh: `rebrand/kelpie` (chưa commit) · Plan: [261023-0305](../261023-0305-typing-latency-chat-parity/plan.md) · Benchmark trước: [benchmark-261023-0405](benchmark-261023-0405-typing-latency.md)

## Kết luận

- **Có cải thiện, đo A/B trước/sau:** Claude Code khi 6 tiến trình chiếm CPU: p50 **430–734 ms → 6–53 ms**; trong app thật (tới frame đã vẽ) p95 **706 ms → 19 ms**, shell **2801 → 18 ms**.
- **Audit tìm ra và đã sửa 1 lỗi nghiêm trọng của chính thiết kế ban đầu:** hạ phiên ẩn xuống *below normal* làm chúng **ngừng in output 30 s+** khi máy bận (đo được). Đổi thiết kế: chỉ nâng phiên đang xem, không hạ gì.
- **Không có hồi quy chức năng CLI:** toàn bộ e2e liên quan đều PASS; 2 lỗi còn lại tái hiện y hệt trên bản HEAD (lỗi có sẵn / flaky).

## Phương pháp

- Audit: 2 reviewer độc lập (lần 2 tập trung hồi quy CLI) + tự audit diff.
- A/B: daemon **HEAD bca2899** (API 4, không có policy) vs **working tree** (API 6), cùng endpoint cô lập, cùng `kelpied bench-echo` (mỗi daemon chạy client build cùng nó; code đo giống hệt), 2 vòng xen kẽ, Claude chạy đúng kiểu app (`--kind claude`).
- E2E trong app thật (WebDriver, app debug build mới, daemon cô lập, `KELPIE_DAEMON_PATH` ghim đúng bản).

## A/B trước/sau (p50 / p95 ms, bench qua daemon)

| | Trước (HEAD) | Sau (thiết kế cuối) |
|---|---|---|
| Claude, 6 phiên Kelpie bận | 514–734 / 989–1133 | 7.5–41 / 75–122 |
| Claude, 6 tiến trình ngoài bận | 430 / 672 (vòng 2: không echo trong 10 s) | 30–49 / 69–81 |
| cmd, có tải | 34–74 | 37–47 (sàn đo ~35 ms: chính client đo bị tranh CPU) |

Trong app (typing_latency.py, bản cuối, p95): shell 2801 → **18 ms**, Claude 706 → **19 ms**, 0 phím mất.

## Phát hiện audit và cách xử lý

| # | Mức | Phát hiện | Xử lý / kiểm chứng |
|---|---|---|---|
| A1 | **Cao** | Phiên ẩn bị hạ *below normal* → shell/agent không in gì 30 s+ khi máy bận (tái hiện: hạ riêng pwsh → echo mất 30 s; trả Normal → echo ngay). `agent_resume` 5/11 vì thế. | Bỏ hạ ưu tiên hoàn toàn; chỉ nâng agent + console host của phiên đang xem + daemon; reader output của phiên ẩn hạ ở mức thread. Sau sửa: agent_resume claude/omp 11/11. |
| A2 | Cao | Codex chat tự ép sandbox/approval preset từ lượt 2 (mất network access, writable roots). | Chỉ gửi mode khi user chọn; giữ chi tiết sandbox của thread; khôi phục pick chỉ khi replay. Test vitest mới. |
| A3 | TB | `!cmd` (Codex) chạy shell ngoài sandbox cả với text gửi hộ (send-to-agent, quick command). | Chỉ khi user tự gõ trong composer. Test. |
| A4 | TB | `/new …` nuốt phần chữ sau; `/model <typo>` làm hỏng mọi lượt sau; `/word` lạ bị chặn. | `/new` có chữ → prompt thường; `/model` chỉ nhận model có trong danh sách; chỉ lệnh TUI đã biết mới chuyển terminal. Test. |
| A5 | TB | Win32 calls chạy trên async ticker và trong session lock; quét lại cả khi policy tắt. | Chuyển sang `spawn_blocking`, ngoài lock; policy tắt → áp một lần. |
| A6 | TB | `image_of` trước khi gán job → nới khe tiến trình con thoát kill-on-close. | Tra sau khi gán job. |
| A7 | TB | Hand-off "Run it in the terminal": lệnh nằm lại trong ô nhập (Claude 2.1.309 chưa xử lý Enter khi hook khởi động còn chạy). | Gốc rễ: prompt cho phiên resume được *gõ* → race. Kiểm chứng `claude --resume <id> "/cmd"` và `codex resume <id> "…"` chạy prompt trong đúng hội thoại → dùng tham số dòng lệnh. 3/3 hội thoại mới PASS; e2e PASS. |
| A8 | Thấp | Composer: Enter với `@`/`$` gõ đủ phải nhấn 2 lần; kết quả `@` cũ đè mới. | Enter gửi khi token đã đủ/trống; generation counter. |
| A9 | Thấp | Gõ prompt khi chương trình chưa vẽ gì ("quiet" tính từ lúc spawn). | Quiet chỉ tính sau output đầu tiên (tối đa 30 s như cũ). |
| A10 | Thấp | Phiên `custom` (dev server…) bị nâng. | Không nâng phiên custom. |
| A11 | — | `CLAUDE_CODE_ENTRYPOINT=sdk-ts` không cần và làm usage bị gắn nhãn Agent SDK. | Đã bỏ (đo lại trên máy rảnh). |

## E2E cuối cùng (bản cuối)

| Script | Kết quả |
|---|---|
| chat_parity (claude, codex, omp) | 31/31 (lượt 2) · claude 13/13 sau sửa A7 |
| typing_latency (shell, claude) | 8/8 |
| chat (claude, codex, omp; TUI ↔ chat) | 8/8 |
| smoke | 16/16 |
| parity | 74/74 (lượt 2) · 73/74 lượt 3: "worktree with a linked folder removes cleanly" — không liên quan, PASS 2 lượt trước |
| restart / scrollback / tabs_panes | 3/3 · 8/8 · 28/28 |
| layouts --ai (resume Claude thật, terminal + chat) | 52/52 |
| agent_resume claude + omp | 11/11 (lượt 2); claude chạy lại: 6/6, 5/6 |
| agent_resume codex | 5/6 |
| Unit: kelpied / kelpie-proto / vitest app | 52/52 · 14/14 · 64/64 (×3 lần, ổn định) |

Lỗi còn lại đều **có sẵn trên HEAD**: `agent_resume` "remembers the conversation" — Codex 5/6 trên HEAD y hệt; Claude HEAD 1/2 lần lỗi giống bản sau (flaky: câu hỏi gõ vào TUI vừa resume).

## Môi trường / lưu ý

- Ổ C: đầy 2 lần (0 GB) — đã xoá ~23 GB thư mục tạm của 3 phiên Claude cũ (user đồng ý). Ổ D: còn < 1 GB, đang giảm (phiên khác build).
- Một phiên khác làm song song (auth/session token, API 6) trên cùng working tree; đã tính trong mọi build/test.
- Codex: `~/.codex/config.toml` đặt `gpt-6.1-sol`, tài khoản không dùng được → e2e đặt profile `-m gpt-6-luna` trong data dir thử nghiệm.
- Claude CLI tự cập nhật 2.1.308 → 2.1.309 trong lúc test.

## Câu hỏi chưa giải quyết

1. `agent_resume` "remembers the conversation" flaky (có sẵn) — nên làm script chờ TUI sẵn sàng tốt hơn?
2. `parity` "worktree with a linked folder removes cleanly" lỗi 1/3 lượt — có thể khoá file trên Windows; cần theo dõi.
3. macOS: policy không tác dụng (không nâng được niceness khi không có quyền).
