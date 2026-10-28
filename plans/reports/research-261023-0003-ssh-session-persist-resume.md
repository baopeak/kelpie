# Giữ phiên SSH chạy trên server & mở lại — nghiên cứu, đánh giá nhu cầu, thiết kế cài đặt

Ngày: 2026-10-23 · Nhánh: `rebrand/kelpie` · Loại: research/design (chưa code)

## 1. Tóm tắt

- **Phần lớn đã có.** Phiên SSH chạy trong tmux `kelpie-<id>` (`tmux new-session -A`). Đóng cửa sổ Kelpie hay khởi động lại máy đều không làm mất phiên: daemon giữ kết nối, hoặc lúc mở lại thì gắn lại vào đúng tmux đó (cài đặt `resumeSessions`).
- **Lỗ hổng thật nằm ở thao tác _đóng pane_.** Kelpie chỉ tắt tiến trình `ssh` ở máy mình, còn phiên tmux trên server vẫn chạy mãi. Phiên đó không còn đường nào để mở lại từ Kelpie, cũng không có cách nào dọn nó. Hộp thoại lại hỏi *"Stop claude and close this pane?"*, tức nói với người dùng một điều sai sự thật.
- **Đề xuất:**
  1. Đóng pane phải chọn rõ một trong hai: **Kết thúc trên server** hoặc **Để chạy trên server**.
  2. Thêm danh sách **"Đang chạy trên server"** để mở lại hoặc kết thúc phiên.
  3. **Tự kết nối lại** khi rớt mạng.
  4. (Tùy chọn) Tự dọn các phiên đã tách mà bỏ quên lâu ngày.
- Không cần một "job manager" riêng như Wave. tmux đã đủ, và Kelpie đã dùng tmux sẵn.

## 2. Hiện trạng (đã đọc code)

| Tình huống | Điều xảy ra hôm nay | Bằng chứng | Đánh giá |
|---|---|---|---|
| Đóng cửa sổ app | Daemon `kelpied` vẫn chạy, ssh vẫn nối. Mở lại thấy phiên sống | [user-guide.md:4](../../docs/user-guide.md#L4) | ✅ Đủ |
| Khởi động lại máy, cập nhật, crash | Phiên được đánh dấu `interrupted`. App resume theo `resumeSessions` (khi hiện / tất cả / tắt). Lệnh restart chạy lại ssh → `tmux new-session -A` gắn lại, agent trên server vẫn chạy suốt thời gian đó | [daemon.rs:496-531](../../crates/kelpied/src/daemon.rs#L496-L531), [store.ts:1210](../../app/src/lib/store.ts#L1210), [agents.rs:248-261](../../crates/kelpied/src/agents.rs#L248-L261) | ✅ Đủ |
| Server mất tmux session (server reboot) | Restart dùng `Resume::Latest`, mở lại hội thoại mới nhất của agent trong thư mục | [daemon.rs:173-183](../../crates/kelpied/src/daemon.rs#L173-L183) | ✅ Fallback hợp lý |
| **Rớt mạng / laptop ngủ** | `ssh` thoát (255), phiên hiện "exited/failed". Người dùng phải tự bấm Resume | Không có logic reconnect | ⚠️ Thiếu tự kết nối lại |
| **Đóng pane / "Stop and close"** | `session.close` chỉ giết `ssh` local. tmux trên server **sống mãi**, Kelpie xóa luôn bản ghi phiên | [store.ts:986-996](../../app/src/lib/store.ts#L986-L996), [daemon.rs:588-595](../../crates/kelpied/src/daemon.rs#L588-L595); toàn repo không có `kill-session` | ❌ Rò phiên + nói sai trên UI |
| Hộp thoại đóng | "Stop {kind} and close this pane?", nhưng phiên remote thực ra không bị dừng | [pane-view.tsx:29-33](../../app/src/components/pane-view.tsx#L29-L33), [sidebar.tsx:127-133](../../app/src/components/sidebar.tsx#L127-L133), [commands.ts:92-99](../../app/src/lib/commands.ts#L92-L99) | ❌ |
| "Stop daemon & all sessions" | Giống trường hợp đóng pane: tmux trên server vẫn còn | [main.rs:62](../../crates/kelpied/src/main.rs#L62) | ⚠️ |
| Host không có tmux | Không có cách giữ phiên. Rớt mạng là mất | `remote.tmux=false` | Chỉ cần nói rõ trên UI |

## 3. Góc nhìn người dùng — có cần không?

**Có, nhưng không phải phần "tự resume khi mở lại app"** (phần đó đã chạy). Cái người dùng thật sự cần:

1. **"Tôi đóng tab Claude đang chạy task dài, mai muốn xem kết quả."** Hôm nay việc đóng tab vô tình giữ được task chạy tiếp, nhưng người dùng không biết điều đó và không có đường quay lại. Cần chủ động **tách** và có chỗ để **mở lại**.
2. **"Tôi đóng hẳn vì xong việc."** Hôm nay việc đóng tab để lại tmux + Claude chiếm RAM trên server, tích tụ theo thời gian (mỗi pane đã đóng thành một phiên rác). Cần **kết thúc thật**.
3. **"Wi-Fi chập chờn hoặc gập laptop."** Người dùng mong terminal tự nối lại như Termius ("auto-reconnect", 2023), thay vì thấy màn hình đỏ "failed".
4. **"Đổi máy."** Phiên chạy trên server thì máy Kelpie khác (hoặc điện thoại) cũng phải thấy và vào tiếp được. tmux đã hỗ trợ việc này, chỉ thiếu danh sách để xem.

Không cần thêm: một cài đặt "giữ phiên khi tắt app" kiểu Wave (opt-in), vì Kelpie đã giữ mặc định nhờ daemon + tmux. Thêm nữa chỉ gây rối.

## 4. Sản phẩm khác

| Sản phẩm | Cách làm | Bài học |
|---|---|---|
| **Wave Terminal v0.14** (durable sessions) | Chạy một job manager nhỏ trên host, đệm output khi mất kết nối, tự gắn lại. Tính năng **tắt mặc định**. Header có biểu tượng khiên cho 3 trạng thái: thường / bền·đang gắn / bền·đã tách | **Chỉ báo trạng thái gắn/tách trên pane** rất đáng học. Không cần agent riêng vì đã có tmux |
| **VS Code Remote-SSH** | `remote.SSH.reconnectionGraceTime` (mặc định khoảng 3 giờ). Maintainer từ chối giữ qua ngày vì tốn tài nguyên và gợi ý dùng tmux. Issue xin detach/reattach vẫn mở | Phiên giữ lâu **phải có hạn hoặc phải dọn được** |
| **Termius** | Có auto-reconnect. Khuyên chạy agent trong tmux và đặt startup `tmux attach -t …` | Tự nối lại là kỳ vọng mặc định. tmux là chuẩn chung |
| tmux | `kill-session` là không đảo ngược được. `list-sessions -F` cho tên, thời điểm tạo, hoạt động cuối, số client đang gắn | Đủ dữ liệu cho danh sách và cho việc dọn |

## 5. Thiết kế đề xuất

### 5.1 Đóng pane SSH (tmux) — hỏi, nhưng chỉ khi có lý do (đã chốt với user 2026-10-23)

Chỉ hỏi khi lựa chọn thật sự có ý nghĩa. Nút được chọn sẵn (Enter) đổi theo trạng thái phiên, nên phần lớn trường hợp chỉ cần bấm Enter.

| Trạng thái phiên | Hành vi | Nút được chọn sẵn |
|---|---|---|
| Đã thoát | Đóng, không hỏi | — |
| Shell rảnh (đang ở dấu nhắc) | Kết thúc trên server, không hỏi | — |
| Đang chạy (shell chạy lệnh, agent working) hoặc đang cần bạn | Hỏi | **Để chạy trên server** |
| Agent rảnh (đã xong lượt) | Hỏi | **Kết thúc** (hội thoại vẫn resume được bằng `--continue`) |
| Không có tmux | Hộp thoại "Dừng …?" 2 nút như cũ, chữ sửa cho đúng | — |

- Đóng nhiều phiên cùng lúc (tab, grid, project): hỏi **một lần** cho tất cả, kiểu "3 phiên đang chạy trên prod-api".
- Sau khi chọn "Để chạy": hiện toast "Đang chạy trên prod-api · Mở lại". Đây cũng là cách người dùng tìm ra danh sách 5.2.
- Ô "Không hỏi lại" ghi vào setting và có thể đổi trong Settings → Remote.

Hộp thoại:

```
┌ Đóng "claude · prod-api"? ───────────────────────────────┐
│ Phiên đang chạy trên server prod-api.                    │
│                                                          │
│ [Để chạy trên server]   [Kết thúc]   [Hủy]               │
│ ☐ Nhớ lựa chọn này                                       │
└──────────────────────────────────────────────────────────┘
```

- Shell đang rảnh: đóng luôn, kết thúc trên server, không hỏi. Giống hành vi hiện tại với shell local.
- Phiên không có tmux: giữ hộp thoại cũ nhưng sửa chữ thành "Dừng … ?" cho đúng sự thật.
- Menu chuột phải pane / phiên trong sidebar có thêm **"Tách — để chạy trên server"**. Shortcut tùy chọn.

### 5.2 Danh sách "Đang chạy trên server"

Danh sách xuất hiện ở hai chỗ, dùng chung một component:

- **Trang SSH host → host**: mục "Phiên đang chạy (3)".
- **Sidebar project** (project remote, hoặc project local có pane SSH): dòng mờ "3 phiên trên server", bấm vào để mở.

```
Đang chạy trên prod-api                          [Làm mới]
● claude   /srv/app     tách 2 giờ trước · hoạt động 5 phút   [Mở lại] [Kết thúc]
● shell    /srv/app     tách 3 ngày · rảnh 3 ngày             [Mở lại] [Kết thúc]
○ claude   /srv/api     đang mở ở máy khác                    [Mở lại]
                                         [Kết thúc các phiên rảnh > 7 ngày]
```

- **Mở lại**: tạo phiên Kelpie mới với **cùng id** (để `kelpie-<id>` khớp), rồi `new-session -A` gắn vào. Nó cũng đi qua `startSession` sẵn có.
- **Dữ liệu**: một lệnh ssh duy nhất:
  ```sh
  tmux list-sessions -F '#{session_name}	#{session_created}	#{session_activity}	#{session_attached}	#{@kelpie_kind}	#{@kelpie_title}	#{pane_current_path}' 2>/dev/null | grep -E '^(kelpie|erka)-'
  ```
- **Metadata nằm trên server.** Khi tạo phiên, đặt tmux user option `@kelpie_kind` và `@kelpie_title`. Máy khác, hay chính máy này sau khi đã xóa bản ghi, vẫn biết đó là Claude ở thư mục nào.
- **Khi mở app hoặc kết nối host**: nếu có phiên đã tách, hiện toast một lần, kiểu "prod-api còn 2 phiên đang chạy · Xem". Không tự mở lại, vì người dùng đã chủ động tách.

### 5.3 Tự kết nối lại

- Áp dụng khi phiên remote **có tmux** thoát với mã 255 (lỗi ssh), mà người dùng không yêu cầu dừng.
- Thử lại theo backoff 2s → 5s → 15s → 30s… rồi dừng sau khoảng 10 phút.
- Pane hiện dải "Mất kết nối — đang nối lại (lần 3)… [Nối ngay] [Thôi]".
- Daemon làm việc này, để nó vẫn chạy khi cửa sổ app đóng. Phiên không có tmux thì không tự nối lại (vì sẽ mất trạng thái), chỉ hiện nút.

### 5.4 Cài đặt (tối thiểu)

**Settings → Remote**

| Cài đặt | Giá trị | Mặc định | Ghi chú |
|---|---|---|---|
| Khi đóng phiên đang chạy trên server | Hỏi · Để chạy trên server · Kết thúc | **Hỏi** | Ô "Nhớ lựa chọn" trong hộp thoại ghi vào đây |
| Tự kết nối lại khi mất mạng | Bật/Tắt | **Bật** | Chỉ áp dụng cho phiên có tmux |
| Tự kết thúc phiên đã tách nếu rảnh quá | Không bao giờ · 1 ngày · 7 ngày · 30 ngày | **Không bao giờ** | Chỉ chạy khi Kelpie kết nối host (không có tiến trình nền trên server). Chỉ chạm vào `kelpie-*`/`erka-*` có `session_attached=0` |

**Trang host** (đã có): "Giữ phiên trong tmux". Đổi chữ thành **"Giữ phiên chạy trên server (tmux)"**. Khi host không có tmux thì kèm câu "Không có tmux: phiên mất khi rớt mạng hoặc đóng".

**Agent settings** (đã có): `resumeSessions` (khi hiện / tất cả / tắt) giữ nguyên, phạm vi vẫn là phiên bị gián đoạn do restart.

Không thêm: ngưỡng thời gian reconnect, chọn kiểu backoff, cài đặt cho từng host về việc đóng/giữ. Những thứ đó không ai chỉnh.

### 5.5 Chỉ báo trên pane và sidebar

- Pane remote có tmux: một chấm nhỏ cạnh nhãn host. Đặc = đang gắn, rỗng = đang nối lại.
- Tooltip: "Chạy trong tmux trên prod-api — đóng app không làm mất phiên".
- Phiên đã tách không nằm trong sidebar như phiên thường. Chúng chỉ hiện ở danh sách 5.2, tránh lẫn với phiên đang mở.

## 6. Kỹ thuật

- **Kết thúc trên server**: daemon chạy một ssh ngắn `tmux kill-session -t '=kelpie-<id>'`. Cần quote, và prefix `=` để khớp tên chính xác. Lệnh chạy trước khi giết ssh local. Nếu host không vào được, ghi vào hàng đợi "cần dọn" và thử lại ở lần kết nối sau.
- **Tách**: chỉ cần giết ssh local (đúng như hôm nay) và **giữ bản ghi** kèm cờ `detached`, hoặc dựa hoàn toàn vào `list-sessions`. Khuyến nghị dựa vào `list-sessions` + user option, để không sinh hai nguồn sự thật.
- **API daemon**: `session.close` thêm `{ remote: "end" | "detach" }`. Thêm `remote.list_sessions { dest, port, config }` và `remote.kill_sessions { ..., names[] }`.
- **Reconnect**: trong session exit handler, nếu `spec.remote.tmux && code == 255 && !user_stopped` thì lên lịch `restart`. Chặn chạy song song bằng cơ chế sẵn có ("never run two processes in one session", [session.rs:281](../../crates/kelpied/src/session.rs#L281)).
- **Hibernate** cho phiên remote hiện chỉ là tách (agent vẫn chạy trên server). Giữ nguyên, nhưng tooltip "ngủ" của pane remote nên nói rõ điều đó.
- **Bảo mật**: chỉ kill phiên có prefix của Kelpie. Tên phiên lấy từ chính lệnh `list-sessions`, lọc `[A-Za-z0-9-]`. User option chỉ chủ tài khoản đó đọc được.
- **Test**: unit test cho chuỗi lệnh kill/list (quote, prefix `=`) và cho quyết định reconnect (mã 255 / không 255 / user stop). E2E với `ssh_test_server.py` gặp hạn chế: server test chạy Git Bash, không có tmux. Phần tmux cần thử tay trên server thật.

## 7. Thứ tự làm

| Bước | Nội dung | Giá trị | Công |
|---|---|---|---|
| 1 | Đóng = kết thúc trên server, có hộp thoại 3 nút + setting "Khi đóng…", sửa chữ hộp thoại | Hết rò phiên, UI nói đúng | Nhỏ–vừa |
| 2 | Danh sách "Đang chạy trên server" (host page + sidebar) + toast khi kết nối | Mở lại được, cả từ máy khác | Vừa |
| 3 | Tự kết nối lại khi mã 255 | Hết màn hình đỏ khi Wi-Fi chập | Nhỏ |
| 4 | Tự dọn phiên rảnh > N ngày | Dọn rác lâu dài | Nhỏ |

Bước 1 nên làm trước: đây là lỗi hiện tại (rò tài nguyên + thông tin sai), không chỉ là tính năng mới.

## 8. Nguồn

- [Wave Terminal — Durable Sessions](https://docs.waveterm.dev/durable-sessions)
- [vscode-remote-release #1701 — detach/reattach, giới hạn vài giờ](https://github.com/microsoft/vscode-remote-release/issues/1701)
- [DEV — remote.SSH.reconnectionGraceTime và Claude Code](https://dev.to/jeromefromhk/the-question-that-disappeared-remote-claude-code-and-the-3-hour-trap-1d8c)
- [Termius — Auto-reconnect](https://termius.com/blog/stay-connected-with-auto-reconnect)
- [Termius — 8 tips for AI agents on mobile (tmux attach khi startup)](https://termius.com/blog/8-tips-for-using-ai-agents-on-mobile-in-termius)
- [Atera — kill vs detach tmux session](https://www.atera.com/blog/how-to-close-a-tmux-session/)

## 9. Câu hỏi còn mở

1. ~~Mặc định khi đóng phiên~~ → đã chốt: **Hỏi** theo trạng thái (xem 5.1).
2. Phiên đã tách có hiện trong sidebar (mờ, trong nhóm riêng) không, hay chỉ hiện ở danh sách host/project?
3. Có làm bước 4 (tự dọn) ngay không, hay để người dùng tự dọn qua danh sách?
4. Format `#{@user_option}` trong `list-sessions -F` cần kiểm lại trên tmux cũ (≤ 2.x, ví dụ CentOS 7). Nếu không hỗ trợ thì lấy metadata từ bản ghi local.
