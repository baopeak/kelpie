# Benchmark: độ trễ gõ phím (keystroke → screen) và ưu tiên CPU

Ngày: 2026-10-23 04:05 (Asia/Saigon) · Nhánh: `rebrand/kelpie` · Plan: [261023-0305-typing-latency-chat-parity](../261023-0305-typing-latency-chat-parity/plan.md)

## Tóm tắt

- Chẩn đoán của user đúng: **gõ chậm khi CPU bận**. Máy i5-6440HQ 4 core: Claude Code rảnh echo p50 **5.6 ms**; khi 6 process chiếm CPU → p50 **295–380 ms**, p95 **510–640 ms**.
- Nguyên nhân gốc: Windows chỉ boost cửa sổ foreground (Kelpie), không boost agent sau nó. Mỗi phím đi qua daemon → console host (OpenConsole) → agent → console host → daemon, mỗi chặng xếp hàng sau mọi việc khác cùng mức Normal.
- Sửa (mặc định bật, *Prioritize visible terminals*): phiên đang hiển thị → agent + console host **Above normal**, daemon **Above normal**, phiên ẩn → cả cây **Below normal**. Con của agent (build/test) vẫn Normal.
- Kết quả Claude Code: tải từ các phiên Kelpie khác **360–380 → 5.5–5.9 ms** (bằng lúc rảnh); tải từ chương trình ngoài **295–308 → 8.5–9.3 ms** (p95 510–580 → 60–68 ms).
- "Tự cắt bớt hiển thị" phía host không giúp gì (lag nằm trong process CLI); fullscreen của Claude không đo được lợi ích ổn định trên máy này (xem Phụ lục).

## Phương pháp

- Công cụ: `kelpied bench-echo` (mới, `crates/kelpied/src/echo_bench.rs`). Tạo 1 session qua daemon, attach *interactive* (như terminal đang hiển thị), chờ chương trình sẵn sàng (`--ready ❯` cho Claude), gõ marker rồi 40 ký tự, mỗi phím đo từ lúc gửi Input frame tới frame output đầu tiên làm **màn hình VT của daemon** chứa chuỗi đã gõ. Khoảng nghỉ 120 ms giữa các phím.
- Tải: `--load 6` = 6 session nền mỗi cái quay 1 core (`kelpied bench-burn`), mô phỏng agent khác đang build/test; `--external 6` = 6 process quay core **ngoài** daemon (chương trình khác, agent ngoài Kelpie).
- Daemon bench cô lập (`KELPIE_ENDPOINT=kelpie-bench`, data dir riêng), bản release không LTO. 2 vòng xen kẽ.
- Máy: Windows 10 Pro 19045, i5-6440HQ (4 core), WebView2 155. Có nhiễu nền từ một phiên khác chạy song song (rustc, python).

## Kết quả (p50 / p95 / max, ms)

### Vòng 1

| Chương trình | Kịch bản | Policy tắt | Policy bật |
|---|---|---|---|
| cmd | rảnh | — | 0.2 / 1.2 / 3.3 |
| cmd | 6 phiên Kelpie bận | 37.2 / 84.6 / 269 | **0.2 / 12.4 / 15.0** |
| cmd | 6 process ngoài bận | 33.9 / 47.5 / 73.5 | 37.2 / 66.8 / 68.8 |
| Claude Code | rảnh | — | 5.6 / 9.5 / 16.1 |
| Claude Code | 6 phiên Kelpie bận | 359.7 / 522.7 / 691 | **5.9 / 18.0 / 29.1** |
| Claude Code | 6 process ngoài bận | 294.5 / 508.9 / 609 | **8.5 / 68.2 / 71.8** |

### Vòng 2

| Chương trình | Kịch bản | Policy tắt | Policy bật |
|---|---|---|---|
| cmd | rảnh | — | 0.3 / 0.7 / 1.1 |
| cmd | 6 phiên Kelpie bận | 33.6 / 77.5 / 91.7 | **0.2 / 0.8 / 2.9** |
| cmd | 6 process ngoài bận | 37.1 / 159.1 / 507 | **1.9 / 45.6 / 61.7** |
| Claude Code | rảnh | — | 5.6 / 8.8 / 34.1 |
| Claude Code | 6 phiên Kelpie bận | 380.5 / 643.4 / 694 | **5.5 / 7.9 / 35.8** |
| Claude Code | 6 process ngoài bận | 308.4 / 583.7 / 725 | **9.3 / 60.5 / 76.0** |

Ghi chú: với tải ngoài, chính `bench-echo` (đóng vai app, mức Normal) cũng bị tranh CPU → sàn đo ~30–40 ms; vì thế cmd (echo do console host làm, vốn ~0 ms) không cải thiện ổn định ở kịch bản này. Claude (300 ms) vượt xa sàn đó nên thấy rõ.

## Thí nghiệm chọn chặng cần boost (tải ngoài 6, Claude, p50/p95)

| Boost Above normal cho | p50 | p95 |
|---|---|---|
| không | 339–439 | 602–672 |
| daemon | 316 | 518 |
| claude.exe | 163 | 247 |
| daemon + claude.exe | 137 | 174 |
| claude.exe + OpenConsole | 50 | 193 |
| **daemon + claude.exe + OpenConsole** | **8–12** | **45–62** |

→ Phải boost cả ba; triển khai đúng như vậy (console host nhận diện bằng snapshot con của daemon trước/sau khi tạo pseudoconsole, có khoá tuần tự).

## Thiết kế đã triển khai

- `proc_tree::Priority { Background, Normal, Foreground }`, áp mỗi tick 250 ms theo `focused` (có view interactive) của session. Chỉ đổi khi khác trạng thái trước.
- Cây job (trừ root): Normal ↔ Below normal, chỉ đổi process đang ở lớp bị rời (process tự chọn Idle giữ nguyên). Root + console host: gán thẳng lớp đích. Process mới sinh từ cha Below normal thừa kế Below normal (quy tắc Windows), con của cha Above normal khởi đầu Normal.
- Chat session: Normal khi hiện, Below normal khi ẩn (ô nhập là của app, không cần boost).
- Daemon: Above normal khi policy bật (mặc định bật từ lúc khởi động; `daemon.set_priority`).
- Unix: no-op (unprivileged không nâng niceness lại được).

## Rủi ro / giới hạn

- Root agent Above normal: nếu chính TUI agent quay CPU (vd bug render loop) nó lấn nhẹ app; con của nó (build) không bị nâng.
- Agent ẩn chạy Below normal: khi máy rảnh không chậm đi; khi máy bận chúng nhường phiên đang xem — đúng ý đồ, có thể tắt trong Settings.
- Kelpie app/WebView2 vẫn Normal (Windows foreground boost theo quantum); e2e đo toàn tuyến bên dưới.

## Phụ lục: các phép đo phụ

- Lần đo đầu Claude rảnh p50 123 ms trùng lúc ổ C: đầy (0 GB) — Claude ghi file trên C:. Sau khi có chỗ trống: 5.6 ms. Không tái hiện có kiểm soát; ghi nhận như nghi vấn.
- Claude fullscreen (`CLAUDE_CODE_NO_FLICKER=1`) và Claude không status line: đo trong lúc có nhiễu nền (rustc của phiên khác) → 237–270 ms, không so sánh được với 5.6 ms. Chưa có bằng chứng fullscreen giảm latency gõ; không thêm setting riêng (đặt được qua biến môi trường trong profile agent).

## E2E toàn tuyến (app thật)

`scripts/e2e/typing_latency.py`: phím vào qua xterm (`term.input`), tính đến khi một frame **đã render** chứa ký tự — app → kelpied → PTY → chương trình → kelpied → webview. 35 phím/kịch bản, 6 phiên nền quay CPU, setting đổi qua `store.setSettings` (đường thật của app). App debug, daemon bench.

| Chương trình | Rảnh | 6 phiên bận, policy tắt | 6 phiên bận, policy bật |
|---|---|---|---|
| Shell | 18.2 / 18.6 ms | **1210.6 / 1701.6 ms** | **17.7 / 18.4 ms** |
| Claude Code | 18.3 / 19.2 ms | 267.7 / 364.7 ms | **18.3 / 32.0 ms** |

(p50 / p95.) Toàn tuyến cộng ~18 ms so với mức daemon (một frame + IPC + gom 4 ms); với policy bật, tải nền **không còn ảnh hưởng thấy được**. 0 phím bị mất ở mọi kịch bản; 8/8 check PASS.

## Chat view parity (e2e)

- `scripts/e2e/chat_parity.py` (claude, codex, omp): **31/31 PASS** — danh sách lệnh của agent (Claude 186, OMP 157, Codex 4 lệnh map), `/comp` → `/compact` đứng đầu + Tab, `@` gợi ý file, `$` skill Codex, picker model/effort/mode (Claude plan/haiku/low; Codex read-only; OMP plan), `/context` chạy trong chat (không gọi model), `/color` → *Run it in the terminal* → TUI chạy lệnh ("Session color set to: …"), Codex 1 lượt thật trên model chọn bằng picker.
- `scripts/e2e/chat.py` (sẵn có, hồi quy): Claude 4/4, OMP 2/2. Codex lỗi do môi trường: `~/.codex/config.toml` đặt `model = "gpt-6.1-sol"`, tài khoản ChatGPT không dùng được model này (model/list chỉ có gpt-6-luna, gpt-5.6-terra, gpt-5.6-luna); cùng luồng chạy được khi chọn `gpt-6-luna` bằng picker mới.

## Sau code review (sửa và đo lại)

- Bỏ `CLAUDE_CODE_ENTRYPOINT=sdk-ts`: đo lại trên máy rảnh, `initialize` vẫn trả 186 lệnh / 13 model / permission mode không cần biến này (lần đo đầu chạy khi CPU bận build, cửa sổ 30 s không đủ). Biến này làm Claude ghi nhận phiên là Agent SDK và đổi toolset.
- Agent chạy qua launcher (Codex npm: `node` → `codex.exe`): boost cả con mang tên binary của agent. Kiểm chứng trong app: Codex hiển thị → node/codex.exe/OpenConsole **AboveNormal**; ẩn → **BelowNormal**.
- Thứ tự áp: agent + console host trước rồi mới tới cây; quét lại mỗi 5 s (process sinh giữa lúc đổi trạng thái); danh sách pid tự nới khi > 512.
- Thread đọc output của phiên ẩn hạ xuống THREAD_PRIORITY_LOWEST (daemon above normal không parse output nền chen trước app khác).
- Log replay của chat: chỉ giữ dòng pinned mới nhất mỗi loại, tính vào ngân sách byte.
- Codex: `/word` lạ là prompt thường (chỉ lệnh TUI đã biết mới chuyển terminal); `/compact`, `/review`, `!cmd` trước khi thread sẵn sàng được xếp hàng; `/new` khi đang chạy bị từ chối; đổi model gửi kèm effort mặc định; model/effort/mode lưu trong id của `turn/start` nên reload vẫn khôi phục.
- Bench: luồng output đọc ở task riêng (timeout trên channel, không cắt frame).

E2E sau sửa: chat parity **31/31**; typing (app thật) shell 1402 → **17.4 ms**, Claude 300 → **17.7 ms** p50 (rảnh: 18.0 / 17.9 ms); unit test kelpied 47/47.
Codex TUI 0.163 mở vào màn hình danh sách phiên, không phải ô nhập → `bench-echo` không đo được latency gõ của Codex (phím thành hotkey); đã kiểm chứng bằng mức ưu tiên thay vì latency.

## Câu hỏi chưa giải quyết

1. Có nên boost cả WebView2 renderer / Kelpie.exe khi tải ngoài rất nặng? Cần số đo e2e.
2. macOS: không có cơ chế tương đương không cần quyền; có cần `taskpolicy`/QoS class?
