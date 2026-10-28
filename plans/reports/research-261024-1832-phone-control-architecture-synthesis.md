# Tổng hợp: điều khiển phiên Kelpie + màn hình desktop từ điện thoại

Thời điểm: 2026-10-24 18:32 (Asia/Saigon) · Branch `rebrand/kelpie` · Tiêu chí chọn: **hiện đại · hiệu suất tối ưu · thân thiện user nhất** (kèm privacy, công sức).

Tổng hợp từ 8 báo cáo (cùng thư mục):
[remote-desktop chính](research-261024-1820-remote-desktop-view-control.md) ·
[Windows host](researcher-261024-1820-windows-host-capture-input.md) ·
[phone transport](researcher-261024-1820-mobile-client-transport.md) ·
[takeover + macOS](researcher-261024-1820-takeover-patterns-macos.md) ·
[kết nối + ghép đôi](researcher-261024-1820-phone-connectivity-pairing.md) ·
[đối thủ](researcher-261024-1820-mobile-agent-control-landscape.md) ·
[giao thức phiên + UX](researcher-261024-1820-mobile-session-protocol-ux.md) ·
[app shell + push](researcher-261024-1820-mobile-app-shell-push.md) ·
[màn hình + spike](researcher-261024-1820-screen-control-gaps-spike.md)

---

## 1. Kết luận một đoạn

Phone là **bộ điều khiển Kelpie hằng ngày**, màn hình chỉ là lớp phụ khi agent kẹt ở GUI/login. Kiến trúc tốt nhất theo 3 tiêu chí: **PWA cài lên màn hình chính do `kelpied` tự phục vụ**, phiên agent đi **state-sync có số thứ tự qua WebSocket hiện có** (chat view = thẻ native, prompt/permission = nút bấm, nhập liệu = textarea native để Telex/VNI chạy đúng), **Web Push tự host** (VAPID do daemon sinh, không server Kelpie), kết nối theo bậc **LAN/Tailscale → WebRTC P2P → relay tự host mã hoá đầu cuối (tắt mặc định)**, và màn hình theo 2 tầng: **CDP screencast cho browser pane** (Windows, làm trước) và **`kelpie-screen` = capture theo OS + H.264 phần cứng + WebRTC (str0m)** cho desktop/cửa sổ IDE. Không làm app native thuần; app store chỉ là bản nâng cấp sau nếu cần Live Activities.

Khác biệt so với thị trường: chưa đối thủ nào có **một hộp "cần bạn" chung cho mọi agent** + **giành quyền màn hình khi agent kẹt**. Claude Remote Control mạnh nhất (14/15) nhưng chỉ Claude, không có màn hình.

## 2. Kiến trúc chốt

```
┌──────────── Phone: PWA (installed, HTTPS qua tailscale cert) ────────────┐
│ Inbox "cần bạn" mọi agent │ Chat cards │ Text/terminal view │ Screen view │
│ textarea native (IME, dictation) · nút prompt/permission · thanh phím     │
│ Service Worker: Web Push · resume theo seq                                │
└───────┬────────────────────────────────────────────────┬─────────────────┘
        │ WSS: state frames (seq, ≤1/100 ms), input,     │ WebRTC (UDP)
        │ signaling WebRTC, push subscribe               │ H.264 + DataChannel input
        ▼                                                ▼
┌──────────────────────── kelpied (per-user) ─────────────────────────────┐
│ mobile.rs: pair QR + duyệt desktop, token/device, can_write, can_control │
│ attach delta/snapshot → state-sync · choices::detect → prompt cards       │
│ Web Push sender (VAPID tự sinh) · ACL · audit                             │
│     │ stdio JSON                         │ WebView2 DevTools (app)        │
│     ▼                                    ▼                                │
│ kelpie-screen (helper, on-demand)     browser pane: Page.startScreencast  │
│  WGC (window) / DXGI DD (monitor)      + Input.dispatch* / insertText     │
│  MF H.264 HW → str0m (wincrypto)                                          │
│  SendInput (UNICODE) / CGEventPost                                        │
└──────────────────────────────────────────────────────────────────────────┘
Kết nối: Bậc 1 LAN/Tailscale trực tiếp → Bậc 2 WebRTC P2P (STUN) → Bậc 3 relay tự host (Noise, chỉ thấy ciphertext, opt-in)
```

## 3. Lựa chọn theo từng lớp (Pick)

| Lớp | Chọn | Hiện đại | Hiệu suất | Thân thiện | Privacy | Công sức (5=rẻ) | Á quân & lý do thua |
|---|---|---|---|---|---|---|---|
| Kết nối | LAN/Tailscale → WebRTC P2P → relay Noise tự host | 4 | 4 | 5 | 4 | 3 | iroh + app native: browser chỉ chạy qua relay, bắt buộc store app, relay công cộng của n0 trái no-telemetry |
| Giao thức phiên | State-sync seq, coalesce 100 ms, resume `since_seq`, `client_id` idempotent | 4 | 5 | 5 | – | 3 | Mirror xterm.js byte thô (kiểu ttyd/VibeTunnel): lỗi IME Telex, tốn băng thông/pin 4G |
| UI phiên | Chat cards + prompt buttons + text view; xterm.js chỉ đọc làm toggle | 4 | 5 | 5 | – | 3 | Full xterm.js: mobile không được maintainer hỗ trợ |
| App shell + push | PWA cài + Web Push tự host | 3 | 4 | 3 | 5 | 5 | Native (Capacitor/Tauri) + relay đánh thức: 5/4/5/3/1–2 — cần relay Kelpie vận hành, Mac + 99 USD/năm, review 4.2.7, plugin push Tauri v0.1 chưa chạy iOS |
| Browser login | CDP screencast WebView2 | 3 | 3 | 5 | 5 | 4 | — (chỉ Windows; macOS fallback sang kelpie-screen) |
| Desktop/IDE | WGC/DXGI + MF H.264 HW + str0m + SendInput | 5 | 4 | 4 | 5 | 2 | JPEG qua WS: tốn băng thông, TCP đứng hình trên 4G. Dự phòng: webrtc-rs 0.20 → RustDesk qua Tailscale |

Crate đã kiểm tra license/maintain: `windows-capture` (MIT, 2.0.1), `screencapturekit` (Apache-2.0, 11.1.0), `str0m` 0.24.1 (`wincrypto`/`rust-crypto` tránh NASM). Loại: `rustdesk-org/hwcodec` (không có license), `scap` (beta, đứng), `crabgrab` (archived), mọi code RustDesk/Sunshine/moonlight-web (AGPL/GPL vs Kelpie MIT).

## 4. 5 pattern bắt buộc copy (từ đối thủ)
1. Ghép đôi 1 lần bằng QR, mọi phiên hiện ra — không wrapper, không restart phiên (Happy phải restart → điểm yếu).
2. Push chạy khi app đóng / phone khoá.
3. Thẻ duyệt 1 chạm: lệnh, diff, gợi ý rủi ro; phím thô chỉ là fallback.
4. Live view tự reconnect và phát lại phần đã lỡ.
5. Composer có đính kèm ảnh/file + đọc chính tả của bàn phím; terminal là tab phụ.

## 5. Lộ trình đề xuất (theo giá trị/rủi ro)

| Phase | Nội dung | Vì sao trước |
|---|---|---|
| 0 | Sửa ngay: guard `e.isComposing` ở [app.js:224](../../crates/kelpied/mobile/app.js#L224) (Enter đang gửi giữa lúc IME ghép chữ Telex) | 5 dòng, lỗi thật ảnh hưởng user Việt |
| 1 | Phiên agent: prompt cards từ `choices::detect` + permission options, state-sync seq + resume, thanh phím mở rộng | Giá trị hằng ngày lớn nhất, dùng lại logic daemon sẵn có |
| 2 | Chat view parity trên phone (hiện báo "trả lời trên máy tính") | Khoảng trống lớn nhất so với đối thủ |
| 3 | Web Push tự host + hướng dẫn HTTPS Tailscale + ghép đôi trong PWA đã cài (iOS PWA không chung storage với Safari) | Biến phone thành "inbox cần bạn" thật |
| 4 | Spike A — CDP screencast takeover (1 ngày): pass ≤150 ms p95 LAN / ≤250 ms Tailscale, ≥15 fps cuộn, OAuth popup qua `NewWindowRequested`, `isTrusted`; dừng nếu popup bất khả hoặc pane ẩn mất frame | Giải phần lớn ca login/OAuth, không cần quyền OS |
| 5 | Spike B — `kelpie-screen` Windows (2–3 ngày): gate build str0m `wincrypto` giờ đầu; đo glass-to-glass LAN/Tailscale/4G trên iPhone + Android, ≥2 hãng GPU | Rủi ro kỹ thuật cao nhất → đo trước khi plan |
| 6 | Kết nối bậc 2–3: WebRTC P2P cho kênh phiên, relay Noise tự host opt-in | Chỉ khi user cần ngoài Tailscale |
| 7 (tuỳ) | Shell native + relay đánh thức (Live Activities, actionable notification); service SYSTEM cho UAC/login | Chỉ khi có nhu cầu rõ — đổi mô hình cài đặt/vận hành |

Mục tiêu độ trễ màn hình: ~60 ms LAN, ~100 ms Tailscale direct, ~200 ms 4G (ước tính, spike sẽ đo).

## 6. Bảo mật — quy tắc không thương lượng
- Quyền `can_control` riêng, mặc định tắt; mỗi phiên control có indicator trên desktop + hotkey dừng + timeout; audit không ghi nội dung phím.
- Lúc người điều khiển: tạm dừng agent đọc màn hình/DevTools, không log input (theo Operator).
- Trang phone luôn do `kelpied` phục vụ (relay phục vụ JS = phá E2E).
- Notification trên lock screen: không chứa nội dung lệnh; Approve mở app sau sinh trắc/khóa; Deny được phép không xác thực; nonce dùng 1 lần, hết hạn ngắn.
- Không bind 0.0.0.0; relay opt-in, chỉ thấy ciphertext.
- Per-user không vào được UAC/lock/login/cửa sổ elevated → phone phải **báo rõ trạng thái** thay vì đứng hình.

## 7. Cần anh/chị quyết
1. Có chấp nhận **bắt buộc HTTPS (tailscale cert)** để có Web Push + PWA cài đặt không? (Không HTTPS → không push khi phone khoá.)
2. Phone page có được thêm **build step** (dùng lại component SolidJS `app/src/chat`) không, hay giữ JS thuần?
3. Kết nối ngoài Tailscale (bậc 2–3) có trong phạm vi không? Nếu có: Kelpie có chạy **relay công cộng mặc định** hay chỉ cho tự host?
4. Ưu tiên Windows trước cho phần màn hình (macOS sau)?
5. UAC/màn hình login có phải yêu cầu cứng không (→ service SYSTEM, cài per-machine, ký số)?

## 8. Còn phải xác minh (spike/thiết bị thật)
- WebRTC trên `http://` ở iPhone/Android thật; mDNS → peer-reflexive; RTP ≤1100 B qua MTU 1280 Tailscale; Safari tôn trọng `playout-delay`.
- Telex/VNI trên xterm.js mobile (Gboard + Unikey Android, Telex iOS) — hiện suy luận từ issue mở.
- Web Push iOS từ PWA phục vụ qua Tailscale HTTPS; độ tin cậy iOS 18/26.
- WebView2: screencast khi pane ẩn, `isTrusted` của `Input.*`, fps thực tế.
- MF HW encoder AMD/Intel tôn trọng low-latency + đổi bitrate trực tiếp; `windows-capture` xuất D3D11 texture zero-copy.
- macOS: helper trong .app bundle mới hiện ở Screen Recording; prompt hàng tháng Sequoia; entitlement persistent-content-capture.
- Claude iOS app có push cho permission prompt chưa (issue 02/2026 nói chưa) — ảnh hưởng mức khác biệt.
