# Research Report: Xem & điều khiển IDE / desktop từ xa qua app (Kelpie)

Thời điểm: 2026-10-24 18:20 (Asia/Saigon) · Branch `rebrand/kelpie`

## Mục lục
1. Tóm tắt điều hành
2. Phương pháp
3. Hiện trạng Kelpie (điểm gắn vào)
4. Phát hiện chính (capture · encode · transport · input · OS limits)
5. So sánh giải pháp
6. Khuyến nghị kiến trúc
7. Bảo mật
8. Hiệu năng & tuning độ trễ
9. Bẫy thường gặp
10. Bước tiếp theo
11. Câu hỏi chưa giải quyết
12. Nguồn

---

## 1. Tóm tắt điều hành

Use case: đang test phần mềm, agent cần người gõ input / bấm / đăng nhập → user mở phone, thấy màn hình (hoặc chỉ cửa sổ IDE), điều khiển, xong quay lại.

Kết luận thẳng:
- **Không cần tự build để có ngay hôm nay**: Tailscale + RustDesk (hoặc Sunshine/Apollo + Moonlight nếu có GPU) chạy được ngay, độ trễ LAN vài–vài chục ms. Đây là baseline để so.
- **Nếu tích hợp vào Kelpie** (đáng làm vì đã có phone server, pairing, token, quyền ghi, bind IP riêng/Tailscale): stack nên dùng là **WebRTC (str0m, sans-IO Rust) + H.264 phần cứng + Windows Graphics Capture / DXGI Desktop Duplication (Windows) và ScreenCaptureKit + VideoToolbox (macOS) + input qua DataChannel → `SendInput` / `CGEventPost`**, chạy trong **helper process riêng `kelpie-screen`** (giống cách `kelpie-stt` bị tách khỏi daemon).
- **WebRTC thắng WebCodecs+WebSocket** cho "ổn định nhất": UDP (không head-of-line blocking), có congestion control/BWE, jitter buffer, decode phần cứng native trên mọi browser mobile, và **không bắt buộc HTTPS** (WebCodecs chỉ chạy trong secure context, mà phone access của Kelpie hiện HTTPS là tùy chọn).
- **Giới hạn cứng phải nói rõ**: kelpied chạy per-user, cài `currentUser` → **không thấy/không điều khiển được UAC secure desktop, màn hình login/lock Windows, cửa sổ elevated** (UIPI). Secure desktop **chỉ** service SYSTEM vào được (đã xác minh: `uiAccess` không có tác dụng với secure desktop, chỉ giúp với cửa sổ elevated, và cũng cần ký số + Program Files) — trái mô hình cài đặt hiện tại. macOS tương tự: login window không capture được từ user agent.
- **Phone không chỉ để xem desktop** mà là công cụ điều khiển phiên agent hằng ngày → phiên agent đi **text qua WS** (không stream pixel); desktop/IDE đi WebRTC khi cần; đăng nhập trong browser pane Kelpie đi **CDP screencast** (nhẹ nhất). Client: một codebase SolidJS dùng chung component desktop, build ra PWA (kelpied phục vụ) + Tauri 2 mobile (store). Xem §6.1.
- Báo cáo chi tiết: [Windows host](researcher-261024-1820-windows-host-capture-input.md) · [Phone client/transport](researcher-261024-1820-mobile-client-transport.md) · [Takeover patterns + macOS](researcher-261024-1820-takeover-patterns-macos.md).

## 2. Phương pháp
- 5 web search (giới hạn skill), 2026-10. Kết hợp đọc repo: [README.md](../../README.md), [docs/architecture.md](../../docs/architecture.md), [crates/kelpied/src/mobile.rs](../../crates/kelpied/src/mobile.rs), [crates/kelpied/Cargo.toml](../../crates/kelpied/Cargo.toml).
- Từ khóa: low latency remote desktop 2026, RustDesk/Sunshine/Apollo/Selkies, Rust WebRTC str0m webrtc-rs, Windows Graphics Capture DXGI NVENC, UAC secure desktop SendInput uiAccess, ScreenCaptureKit CGEvent, WebCodecs WebTransport vs WebRTC.
- Chất lượng nguồn: nhiều bài roundup/affiliate (số latency không nhất quán) → chỉ dùng làm định hướng. Phần đánh dấu **[KT]** là kiến thức nền chưa xác minh lại trong phiên này — cần verify trước khi code.

## 3. Hiện trạng Kelpie — điểm gắn vào

| Có sẵn | Ý nghĩa cho tính năng mới |
|---|---|
| Phone server axum trong `kelpied` (port 4817), bind 1 IPv4 private/Tailscale, không bao giờ 0.0.0.0 | Có sẵn kênh signaling + auth; không cần STUN/TURN nếu chỉ LAN/Tailscale |
| Pairing QR 2 phút, token 256-bit hash SHA-256, so sánh constant-time, revoke đóng socket | Tái dùng nguyên; thêm quyền `can_control` riêng |
| `can_write` per device, Origin check, CSP, rate limit | Remote control là quyền cao hơn "gõ vào terminal" → tách flag |
| HTTPS tùy chọn (`tailscale cert`) | Ảnh hưởng chọn transport (WebCodecs cần secure context) |
| `kelpie-stt` là helper process riêng "để native engine không kéo sập app/daemon" | Áp dụng y hệt cho capture/encode (driver GPU crash không được giết PTY) |
| Keep-awake | Cần, nhưng không ngăn được **lock screen** — lock là điểm chết của capture |
| License MIT, installMode `currentUser` | Không nhúng code RustDesk (AGPL) / Sunshine (GPLv3). Không đạt điều kiện `uiAccess` |

Về "xem IDE": terminal/agent trong Kelpie **đã** stream dạng text qua phone (rẻ, nét hơn pixel). Pixel streaming chỉ cần cho: app GUI đang test, browser login/OAuth, IDE khác (VS Code/Cursor), dialog hệ thống. → Hỗ trợ 2 chế độ: **1 cửa sổ** (WGC capture theo HWND) và **cả màn hình**.

## 4. Phát hiện chính

### 4.1 Capture
**Windows**
- **Windows Graphics Capture (WGC)**: capture theo monitor *hoặc theo cửa sổ* — hợp "chỉ xem IDE". Crate `windows-capture` (dùng WGC, có encoder phần cứng, cũng hỗ trợ DXGI Desktop Duplication).
- **DXGI Desktop Duplication**: full monitor, có dirty rects/move rects, overhead thấp. Crate `dxgi-capture-rs` (1.2.2, 2026-04-07).
- [KT] Win10 19045 (máy dev): WGC có từ 1903; tắt viền vàng (`IsBorderRequired`) chỉ có trên Win11 → trên Win10 capture cửa sổ sẽ có viền vàng. `IsCursorCaptureEnabled` có từ 2004. Full-desktop nên ưu tiên DXGI trên Win10.
- Cả hai đều mất hình khi chuyển sang Winlogon desktop (lock/UAC/Ctrl+Alt+Del) nếu process không phải SYSTEM.

**macOS**
- **ScreenCaptureKit** (macOS 12.3+; Kelpie hỗ trợ 12+ → cần fallback `CGDisplayStream` cho 12.0–12.2 hoặc nâng min lên 12.3). Có capture theo window/app/display.
- Cần quyền TCC Screen Recording. Forum Apple: ở login window, buffer trả NULL/rỗng; Sequoia 15.0 có báo cáo `CGDisplayStream` treo đúng 30s rồi NULL.
- [KT] macOS 15 thêm nhắc lại quyền screen recording định kỳ (hàng tháng từ 15.1); app remote-desktop có thể xin entitlement `com.apple.developer.persistent-content-capture` (Apple duyệt). Cần verify.

### 4.2 Encode
- Codec: **H.264** là mẫu số chung (decode phần cứng trên mọi phone, mọi browser WebRTC). HEVC/AV1 hỗ trợ không đều trên WebRTC mobile → để sau.
- Windows: [KT] **Media Foundation H.264 encoder MFT** = phần cứng trên NVIDIA/AMD/Intel không cần SDK riêng từng hãng; bật `CODECAPI_AVLowLatencyMode`, CBR/VBR có trần, không B-frame. NVENC trực tiếp nhanh hơn chút nhưng chỉ NVIDIA. Fallback phần mềm: openh264 (BSD; binary Cisco có patent license).
- macOS: [KT] VideoToolbox `kVTCompressionPropertyKey_RealTime`, `EnableLowLatencyRateControl` (11.3+).
- Ví dụ gần nhất end-to-end: `JRF63/desktop-streaming` = DXGI → NVENC → webrtc-rs, tự phân mảnh NAL. Chưa xác minh còn maintain.

### 4.3 Transport

| | WebRTC (str0m) | WebCodecs + WebSocket | WebTransport + WebCodecs / MoQ |
|---|---|---|---|
| Lớp vận chuyển | UDP (SRTP), mất gói không chặn frame sau | TCP → head-of-line blocking khi 4G chập chờn | QUIC |
| Congestion control / BWE | Có (TWCC/GCC) | Tự làm (đo `bufferedAmount`, drop frame) | Tự làm |
| Decode | Native, phần cứng | `VideoDecoder` — **chỉ secure context (HTTPS)** | như WebCodecs |
| NAT | ICE có sẵn; LAN/Tailscale chỉ cần host candidate | không cần | server phải reachable |
| Mobile Safari | Ổn định lâu năm | [KT] VideoDecoder từ Safari 16.4 | WebTransport Baseline từ Safari 26.4 (03/2026) theo 1 nguồn vendor — chưa xác minh |
| Độ phức tạp | Trung bình (str0m lo DTLS/SRTP/RTP packetize) | Thấp | Cao, chưa chín cho remote desktop |
| Độ trễ đo được | Phân tích robotics: WebRTC chỉ thêm ~10 ms (jitter buffer), tổng 150–180 ms qua internet | Phụ thuộc mạng | webrtcHacks: 140–690 ms, "chưa production-ready" (bài cũ ~2024) |

**str0m vs webrtc-rs**: str0m sans-I/O, không thread/async nội bộ, API frame-level `Writer::write` → hợp tokio của kelpied, tự điều khiển socket UDP trên đúng IP phone server đã bind. webrtc-rs theo API RTCPeerConnection kiểu JS, nặng hơn. → **str0m**. [KT] Kiểm tra backend crypto của str0m (OpenSSL mặc định; có feature crypto Windows/Rust thuần?) — repo đã tránh aws-lc vì NASM, nên tránh luôn OpenSSL build trên Windows.

Đã xác minh thêm: `RTCPeerConnection` không có `[SecureContext]` trong spec W3C (chỉ `getUserMedia` bị chặn) → WebRTC nhận-only chạy trên `http://` LAN (chưa test thiết bị thật); `VideoDecoder` chỉ secure context, Safari từ 16.4; WebTransport cũng secure context + cần HTTP/3 → loại. str0m 0.24.1 (2026-10-03) có feature `wincrypto`/`rust-crypto` → tránh được aws-lc (NASM) và OpenSSL; có TWCC/BWE, SCTP DataChannel; chưa thấy ai dùng str0m cho desktop streaming (rủi ro: Kelpie là người đầu). Dự phòng: webrtc-rs 0.20 (bản sans-I/O mới; 0.17 chỉ còn sửa lỗi).

Rủi ro transport cần xử lý trong spike: host candidate của phone thường là mDNS `.local` → str0m phải nhận địa chỉ thật qua peer-reflexive từ STUN; MTU WireGuard của Tailscale 1280 → giới hạn RTP payload ~1100 B; Tailscale trên 4G có thể rơi về DERP relay (kiểm tra `tailscale ping`, mở UDP 41641). Safari: `jitterBufferTarget` chỉ có từ Safari 27, `playout-delay` được negotiate nhưng chưa chắc được tôn trọng.

Kết luận transport: dùng WebRTC. Lý do quyết định: (1) "ổn định nhất" trên mạng di động = UDP + BWE; (2) không ép HTTPS; (3) WebTransport chưa đủ chín, nguồn về Safari chỉ là vendor blog.

### 4.4 Input
- Kênh: WebRTC **DataChannel** (ordered+reliable cho phím/click; unordered/maxRetransmits=0 cho mouse-move) — không đi WS để không bị trễ chéo.
- Windows: `SendInput`; text/mật khẩu gửi dạng chuỗi → `KEYEVENTF_UNICODE` (không phụ thuộc layout bàn phím, Telex/VNI không can thiệp). Tọa độ `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` tính theo monitor đang stream, chú ý DPI awareness per-monitor v2.
- **UIPI**: process medium-integrity không gửi input vào cửa sổ elevated (Admin). Lối chính thống là `uiAccess=true` + ký số CA tin cậy + chạy từ thư mục bảo mật (Program Files) — Kelpie cài per-user → không đạt. `SendInput` trả số event đã inject → phát hiện foreground elevated (`TokenElevation`) và báo lên phone.
- Secure desktop (UAC, Ctrl+Alt+Del, lock/login): **chỉ** service SYSTEM chuyển desktop (`OpenInputDesktop`/`SetThreadDesktop`) — xác minh trong source RustDesk (`try_change_desktop`, `send_sas` qua `sas.dll`) và Sunshine (`syncThreadDesktop`, reinit khi `DXGI_ERROR_ACCESS_LOST`). `uiAccess` không giúp ở đây (MS docs).
- macOS: [KT] `CGEventPost` cần TCC Accessibility; text dùng `CGEventKeyboardSetUnicodeString`.
- Clipboard: gửi text từ phone → set clipboard + Ctrl+V là cách nhanh nhất cho chuỗi dài; đồng bộ ngược (desktop→phone) mặc định tắt.

## 5. So sánh giải pháp có sẵn

| Giải pháp | License | Ưu | Nhược với use case này |
|---|---|---|---|
| **RustDesk** (1.4.5, 01/2026) | AGPL-3.0 | Self-host relay, có service mode thấy UAC/login, app mobile đầy đủ | App riêng, không gắn được với "agent X đang cần bạn"; không nhúng code (AGPL vs MIT); latency báo cáo dao động 12–60 ms |
| **Sunshine / Apollo + Moonlight** | GPLv3 | Độ trễ thấp nhất nhóm OSS (~5 ms LAN, blog), encode GPU | Thiết kế cho game; điều khiển chuột/phím kiểu desktop trên phone kém tiện; cần cài host riêng |
| **Parsec** | Đóng | Sub-16 ms, 4K60, service SYSTEM bấm được UAC | Đóng, tài khoản cloud |
| **Windows RDP / Windows App** | Có sẵn | Không cài gì | [KT] Chiếm session console (khóa màn hình local), đổi độ phân giải, app GPU/WebGL có thể chạy khác → làm sai kết quả test GUI |
| **Chrome Remote Desktop** | Đóng | Dễ, qua browser | Qua Google, không tích hợp |
| **Selkies** | MPL/Apache [KT] | WebRTC + GStreamer trong browser | Thiên Linux/container |
| **Guacamole / noVNC (VNC)** | Apache/MPL | Chạy mọi browser | Encode kiểu VNC → trễ/băng thông kém hơn video codec |
| **Tự tích hợp vào Kelpie** | MIT | Mở thẳng từ "needs you" của session; tái dùng pairing/token; chọn 1 cửa sổ IDE | Công sức build; không vượt được UAC/login trừ khi thêm service SYSTEM |

## 6. Khuyến nghị kiến trúc

```
Phone browser (PWA của phone server)
  │  HTTPS/HTTP + WS  (đã có: pair, token, Origin, rate limit)
  │      └─ signaling: screen.offer/answer, screen.start{target: monitor|hwnd}, screen.stop
  │  UDP (WebRTC, cùng IP đã bind)
  │      ├─ video: H.264, SRTP
  │      └─ DataChannel "input": mouse/key/text/scroll
  ▼
kelpied ── mobile.rs: kiểm tra can_control, spawn/kill helper, chuyển SDP/ICE
  │  stdio JSON lines (như kelpie-stt)
  ▼
kelpie-screen (helper, chỉ chạy khi có viewer, tự thoát khi idle)
  ├─ capture: DXGI DD (monitor) | WGC (window)      / ScreenCaptureKit (mac)
  ├─ encode : MF H.264 HW MFT → openh264 fallback   / VideoToolbox
  ├─ str0m  : RTP packetize, TWCC BWE → điều bitrate encoder
  └─ input  : SendInput (UNICODE text)               / CGEventPost
```

Quyết định chính và lý do:
1. **Helper riêng** — crash driver GPU/encoder không kéo theo daemon giữ mọi PTY; nhất quán với quyết định đã ghi cho `kelpie-stt`.
2. **Chỉ host ICE candidate trên IP đã bind** — phone access vốn giới hạn LAN/Tailscale, nên không cần STUN/TURN; không mở thêm bề mặt mạng ngoài 1 port UDP cố định (dễ làm firewall rule như port 4817 hiện tại).
3. **Signaling qua WS hiện có** — không thêm endpoint auth mới.
4. **Target = monitor hoặc 1 cửa sổ** — "xem IDE" = chọn HWND từ danh sách cửa sổ; nút "Show desktop" từ thông báo needs-you của session.
5. **Encoder bám BWE** — str0m báo bandwidth estimate → đổi bitrate/fps/độ phân giải encoder; PLI/FIR → keyframe ngay.

UX phone (tối thiểu để dùng được cho login/test):
- Chế độ trackpad (con trỏ tương đối, ổn cho màn to) và direct-touch (chạm = click tại điểm).
- Pinch-zoom/pan phía client (không đổi độ phân giải host).
- Hàng phím đặc biệt: Esc, Tab, Ctrl, Alt, Win/Cmd, mũi tên, Enter; ô "gửi văn bản" (mật khẩu/OTP) gửi một lần dạng UNICODE, không lưu, không log.
- Banner rõ trên desktop khi đang bị điều khiển + phím tắt dừng ngay.

### 6.1 Phone client: một codebase, ba lớp điều khiển

Phone là công cụ điều khiển Kelpie hằng ngày (phiên Claude Code/Codex/shell), không chỉ xem desktop. Trang hiện tại ([crates/kelpied/mobile/app.js](../../crates/kelpied/mobile/app.js), ~258 dòng JS thuần) mới có: danh sách phiên, xem màn hình, reply + vài phím cố định.

| Lớp | Dùng cho | Transport | Tần suất |
|---|---|---|---|
| 1. Phiên agent | đọc/gõ/answer Claude Code, Codex, shell; chat view | **text qua WS hiện có** (VT state / chat relay) | hằng ngày |
| 2. Browser pane | login/OAuth/2FA trong pane Kelpie | **CDP `Page.startScreencast` + `Input.dispatch*`** qua WebView2 DevTools (đã dùng cho element screenshot) | thỉnh thoảng; chỉ Windows (WKWebView không có CDP) |
| 3. Desktop / cửa sổ IDE | app GUI đang test, dialog hệ thống | WebRTC + `kelpie-screen` | thỉnh thoảng |

Không stream pixel cho lớp 1: text nét hơn, ~0 băng thông, không trễ encode — lợi thế lớn nhất so với RustDesk.

**Hình thức app**: một client SolidJS dùng chung component desktop (session list, xterm.js, chat view), build 2 đường: (a) PWA do kelpied phục vụ như hiện nay, (b) Tauri 2 mobile (iOS/Android) đóng gói cùng bundle cho store. Native-only (Keychain cho token, notification, bàn phím cứng, share sheet) bật theo feature detection. Native thuần Swift/Kotlin không đáng: độ trễ video do host quyết định; WebRTC trong browser/webview đã decode phần cứng.

**Push "agent cần bạn" khi phone khoá** — cần quyết:
- PWA Web Push: daemon tự gửi tới push service (VAPID tự sinh), không cần server riêng; cần HTTPS + "thêm vào màn hình chính" (iOS 16.4+) + máy có internet [KT].
- Native APNs/FCM: cần relay server mình vận hành (không nhúng APNs key vào app) → thêm thành phần cloud, trái nguyên tắc "mọi thứ ở trên máy".

**Takeover UX** (theo Operator/Browserbase): trạng thái "needs you" → mở live view có input → nút "Xong" trả quyền cho agent. Trong lúc người điều khiển: tạm dừng agent đọc màn hình/DevTools, không log input (Operator là sản phẩm duy nhất công bố rõ điều này).

## 7. Bảo mật (threat model)
Thứ được bảo vệ: toàn quyền phiên người dùng (file, browser đã đăng nhập, mật khẩu đang gõ). Remote control = rủi ro cao nhất của app.
- Quyền mới `can_control` per device, mặc định tắt, tách khỏi `can_write`; bật phải xác nhận trên desktop.
- Mỗi phiên control: bắt đầu cần xác nhận trên desktop (tùy chọn "tin thiết bị này"), tự ngắt sau N phút không tương tác, ngắt ngay khi revoke device.
- Indicator luôn hiển thị (viền/toast/tray) + hotkey kill; ghi audit (device, giờ bắt đầu/kết thúc, target) — **không** ghi nội dung phím/text.
- DTLS-SRTP có sẵn của WebRTC; fingerprint DTLS trao qua WS đã auth → chống MITM trên LAN.
- Không bind 0.0.0.0 (giữ quy tắc hiện tại); UDP chỉ trên IP đã chọn.
- Clipboard desktop→phone mặc định tắt.
- Không cố bypass UAC/secure desktop bằng thủ thuật (tắt secure desktop, GPO UIAccess — chính forum Microsoft báo không ổn định).

## 8. Hiệu năng & tuning độ trễ
Mục tiêu glass-to-glass theo tầng mạng: **~60 ms LAN, ~100 ms Tailscale direct, ~200 ms 4G** (ước tính, cần đo; con số 30–80 ms chỉ khả thi trên LAN với NVENC). Encode: NVENC ULL 3–8 ms [KT], MF software 10–30+ ms [KT]. Media Foundation (đã xác minh): `CODECAPI_AVLowLatencyMode`, CBR, bitrate đổi trực tiếp được (`AVEncCommonMeanBitRate`), `AVEncVideoForceKeyFrame` cho PLI; intra refresh không có trong MF → dùng IDR theo yêu cầu + GOP dài.
- Không B-frame, GOP dài + keyframe theo yêu cầu (hoặc intra-refresh) để tránh spike keyframe.
- Capture theo dirty rect: desktop đứng yên → gần 0 bitrate, phone không tốn pin.
- Scale về độ phân giải phone (vd. 1280×720 hoặc 1600×900 cho text IDE); text IDE cần bitrate cao hơn video thường — ưu tiên giảm fps trước khi giảm độ nét khi BWE thấp.
- 30 fps mặc định, 60 fps khi băng thông dư.
- [KT] RTP header extension `playout-delay` (min=max=0) giảm jitter buffer trên Chrome; Safari chưa rõ.
- Ưu tiên process helper ở mức "above normal" giống *Prioritize visible terminals*.
- Đo bằng `RTCPeerConnection.getStats()` (jitterBufferDelay, framesDropped, RTT) hiển thị overlay debug.

## 9. Bẫy thường gặp
- Màn hình bị **lock** → capture đen; keep-awake không ngăn lock do policy/timeout → báo rõ trên phone "Máy đang khóa — cần mở bằng tay hoặc dùng RustDesk service".
- UAC prompt hiện ra giữa test → phone thấy đen/đứng, input không tác dụng. Báo trạng thái "secure desktop" thay vì treo im lặng.
- Cửa sổ elevated (chạy as Admin) → nhìn thấy nhưng click không ăn (UIPI). Báo rõ.
- DPI/multi-monitor lệch tọa độ → bắt buộc per-monitor DPI aware v2 trong helper.
- WGC trên Win10 có viền vàng quanh cửa sổ được capture — không phải bug.
- macOS: thiếu quyền Accessibility → xem được nhưng không điều khiển; prompt TCC chỉ hiện khi có user session.
- RDP làm khóa console → không dùng RDP để test app GUI chạy trên console.
- Nhúng code AGPL/GPL vào Kelpie MIT → không được.

## 10. Bước tiếp theo
1. **Ngay (0 code)**: thêm hướng dẫn trong user guide: Tailscale + RustDesk (bật service mode nếu cần UAC/login). Dùng làm baseline đo độ trễ.
1b. **Lớp 1 trước (giá trị cao nhất, rủi ro thấp)**: nâng phone client cho phiên agent — ô soạn thảo thật, dictation, hàng phím đặc biệt, chat view, dùng chung component SolidJS với desktop.
1c. **Lớp 2**: takeover browser pane qua CDP screencast (Windows/WebView2) — giải quyết phần lớn ca login/OAuth mà không cần capture OS.
2. **Spike 1–2 ngày**: binary thử `kelpie-screen` trên Windows: DXGI DD → MF H.264 HW → str0m → trang HTML nhận qua WebRTC trên iPhone + Android qua Tailscale. Đo glass-to-glass (đồng hồ ms trên màn hình, chụp phone), CPU/GPU, bitrate khi đứng yên. Kiểm tra backend crypto của str0m build được trên Windows không cần NASM/OpenSSL.
3. Nếu spike đạt (≤100 ms p95 qua Tailscale, ổn định trên 4G): lập plan phase qua `/ak:plan` — quyền `can_control`, signaling, input DataChannel, chọn window, UX phone, indicator, audit, macOS ScreenCaptureKit.
4. Chỉ cân nhắc service SYSTEM (UAC/login) nếu use case login Windows thật sự cần — đó là thay đổi mô hình cài đặt (per-machine, ký số), phải quyết riêng.

## 11. Câu hỏi chưa giải quyết
1. Cần điều khiển **UAC / lock / login Windows** không, hay chỉ trong phiên đã đăng nhập? (Quyết định có cần service SYSTEM + cài per-machine.)
2. Truy cập chỉ LAN/Tailscale hay cần qua internet không Tailscale? (Nếu có → cần TURN/relay, thay đổi lớn về bề mặt tấn công.)
3. Ưu tiên Windows trước, macOS sau?
4. Có cần audio không? (Hiện bỏ qua.)
5. Push khi phone khoá: PWA Web Push (cần HTTPS, không server) hay app native + relay server?
6. Phát hành Tauri 2 mobile lên store (tài khoản Apple 99 USD/năm, review) hay chỉ PWA?
7. Cần verify bằng spike: str0m `wincrypto`/`rust-crypto` build MSVC không NASM; WebRTC trên `http://` ở iPhone/Android thật; mDNS/peer-reflexive; MTU 1280; Safari tôn trọng `playout-delay`; `IsBorderRequired` cho app NSIS chưa đóng gói; MF HW encoder AMD/Intel tôn trọng low-latency; macOS: helper phải nằm trong .app bundle mới hiện trong danh sách Screen Recording (1 báo cáo forum), prompt screen recording hàng tháng trên Sequoia, entitlement persistent-content-capture xin qua form Apple.

## 12. Nguồn
- RustDesk / Moonlight / Parsec số liệu: [dupple.com](https://dupple.com/learn/best-remote-desktop-software), [remogrid.com](https://www.remogrid.com/blog/remote-work/best-free-remote-desktop-software), [remio.net](https://remio.net/blog/best-free-remote-desktop-2026), [tenvoai.com](https://tenvoai.com/blog/best-remote-desktop-software-2026-comprehensive), [softpicker.com](https://softpicker.com/best-remote-desktop-software/), [Wikipedia: Comparison of remote desktop software](https://en.wikipedia.org/wiki/Comparison_of_remote_desktop_software), [helpwire.app](https://www.helpwire.app/blog/remote-desktop-client-linux/)
- Capture/encode/WebRTC Rust: [windows-capture docs](https://docs.rs/windows-capture), [NiiightmareXD/windows-capture](https://github.com/NiiightmareXD/windows-capture), [dxgi-capture-rs](https://lib.rs/crates/dxgi-capture-rs), [dxgcap-rs](https://github.com/TommyGymer/dxgcap-rs), [JRF63/desktop-streaming](https://github.com/JRF63/desktop-streaming), [str0m](https://github.com/gab8i/str0m), [twicecat issue #22](https://github.com/ga-lep/twicecat/issues/22), [GitHub topic whep](https://github.com/topics/whep)
- Windows UAC/UIAccess: [MS Learn UIAccess secure locations](https://technet.microsoft.com/en-us/library/jj852244.aspx), [Broadcom KB UAC remote sessions](https://knowledge.broadcom.com/external/article/449607/unable-to-interact-with-notifier-prompt.html), [MS Q&A](https://learn.microsoft.com/en-us/answers/a/12737194), [Tech Community uiaccess](https://techcommunity.microsoft.com/tag/uiaccess), [Tenable CIS Win11](https://www.tenable.com/audits/items/CIS_Microsoft_Windows_11_Enterprise_v5.0.1_L1.audit:8e72790b29ee6a927c07a4a689035f85)
- macOS: [Apple forums ScreenCaptureKit](https://developer.apple.com/forums/tags/screencapturekit?page=4), [WWDC22 10156](https://developer.apple.com/videos/play/wwdc2022/10156/), [Recall.ai macOS capture](https://www.recall.ai/blog/macos-screencapture-api)
- Transport: [webrtcHacks WebCodecs/WebTransport](https://webrtchacks.com/webcodecs-webtransport-and-webrtc/), [Transitive Robotics WebRTC latency](https://transitiverobotics.com/blog/webrtc-latency-breakdown/), [Open Robotics discourse](https://discourse.openrobotics.org/t/where-does-latency-in-webrtc-video-streaming-come-from-an-analysis/54566), [Fora Soft low latency 2026](https://www.forasoft.com/blog/article/real-time-video-streaming-app-development-low-latency), [Fora Soft WebTransport](https://www.forasoft.com/learn/video-streaming/articles-streaming/webtransport-whip), [VideoSDK WebRTC vs WebTransport](https://www.videosdk.live/developer-hub/webtransport/webrtc-vs-webtransport)
