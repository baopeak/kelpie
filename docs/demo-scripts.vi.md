# Kịch bản demo Kelpie

Tài liệu này hướng dẫn người trình bày giới thiệu Kelpie bằng bộ demo đi kèm repository. Mỗi cảnh ghi rõ mục tiêu, phần chuẩn bị, các bước thao tác, lời dẫn gợi ý, kết quả người xem cần thấy và cách đưa màn hình về trạng thái ban đầu.

Bộ demo chỉ dùng dữ liệu mẫu. Các project là repository Git nhỏ, chạy bằng Python chuẩn và không kết nối dịch vụ bên ngoài. Kết quả do agent tạo ra trong buổi demo là kết quả thật, vì vậy người trình bày không nên hứa trước nội dung câu trả lời của model.

Tài liệu tham chiếu: [README](../README.vi.md), [hướng dẫn sử dụng tiếng Việt](user-guide.vi.md) và [user guide đầy đủ (tiếng Anh)](user-guide.md).

## 1. Thông điệp chính

Người trình bày nên để người xem nhớ ba ý sau:

1. **Một nơi cho mọi agent.** Kelpie chạy Claude Code, Codex, OMP và 20 CLI agent khác cạnh shell, trong nhiều project cùng lúc. Người dùng giữ nguyên công cụ, tài khoản và quyền hạn của mình.
2. **Luôn biết việc nào cần mình.** Dashboard, chuông Activity và điện thoại cho biết phiên nào đang chờ trả lời, phiên nào đã xong và phiên nào bị lỗi.
3. **Cập nhật không làm gián đoạn công việc.** Các phiên chạy trong daemon `kelpied`, tách khỏi cửa sổ. Khi Kelpie tự cập nhật, chỉ giao diện khởi động lại; agent vẫn tiếp tục làm việc và người dùng mở lại đúng màn hình cũ.

Ý thứ ba là lợi ích thực tế nhất của kiến trúc daemon. Người trình bày không nên nhấn mạnh việc "đóng cửa sổ rồi bỏ đi", vì người viết code thường muốn mở Kelpie để theo dõi tiến độ. Tuỳ chọn chạy nền khi đóng cửa sổ vẫn tồn tại, nhưng chỉ nên nhắc ngắn khi nói về automation hoặc điện thoại.

## 2. Chuẩn bị

### 2.1. Phần mềm

- Người trình bày cài Kelpie từ [trang Releases](https://github.com/baopeak/kelpie/releases) hoặc build từ mã nguồn theo README.
- Người trình bày cài và đăng nhập sẵn ít nhất hai agent CLI, ví dụ Claude Code và Codex. Settings → Agents phải hiển thị chúng là *installed*.
- Máy cần có Python 3.10 trở lên và Git.
- Cảnh điện thoại cần một điện thoại có camera. Cảnh SSH cần một máy chủ thật mà người trình bày có quyền đăng nhập.

### 2.2. Tạo bộ demo

Người trình bày chạy lệnh sau từ thư mục gốc của repository. Thư mục đích phải chưa tồn tại hoặc đang trống:

```bash
python scripts/e2e/readme_demo.py ../kelpie-demo-projects
python ../kelpie-demo-projects/atlas-commerce/smoke.py
```

Lệnh thứ nhất tạo tám project Git, hai worktree và một plugin mẫu. Lệnh thứ hai kiểm tra rằng project chính chạy được và phải in ra `PASS atlas-commerce`.

| Thành phần | Nội dung | Dùng trong cảnh |
|---|---|---|
| `atlas-commerce` | Tính giá giỏ hàng, mã giảm giá và ngưỡng miễn phí vận chuyển; có một thay đổi chưa commit để review | 3, 4, 5, 6, 7, 8 |
| `atlas-commerce/.kelpie.yaml` | `setup`, `tabs`, `copy` (`.env.local`) và `check` (`python smoke.py`) | 4, 6 |
| `atlas-commerce/plans/checkout-hardening/` | Kế hoạch có hai phase với checklist | 8 |
| `atlas-commerce/runs/checkout-hardening.json` | Một run ba task, mỗi task chờ phê duyệt hoặc chờ task trước | 8 |
| `atlas-commerce/docs/` | Markdown có sơ đồ Mermaid và tệp `checkout-flow.mmd` | 7 |
| `worktrees/coupon-validation` | Nhánh `feature/coupon-validation` đã có một commit hoàn chỉnh | 6 |
| `worktrees/shipping-threshold` | Nhánh `fix/shipping-threshold` chưa có thay đổi | 4 |
| `beacon-api` | API catalog nhỏ; `python src/app.py --serve` mở cổng 8765 | 10 |
| `pulse-analytics` | Dữ liệu CSV và notebook `notebooks/revenue-summary.ipynb` | 7 |
| `relay-workers` | Hàng đợi job đơn giản, dùng để tạo một phiên "cần bạn" | 3 |
| Bốn project còn lại | `orbit-mobile`, `prism-design`, `harbor-infra` và `field-notes` làm cho sidebar và board trông như một ngày làm việc thật | 2, 4 |
| `plugins/kelpie-demo-tools/` | Plugin khai báo một lệnh, một quick command, một theme terminal và một mục status bar | 12 |

### 2.3. Đưa dữ liệu vào Kelpie

1. Người trình bày mở Kelpie và nhấn `+` ở sidebar, sau đó thêm từng thư mục project. Kelpie tự thêm hai worktree của `atlas-commerce` vào dưới project cha.
2. Người trình bày ghim `atlas-commerce` và đặt trạng thái board cho các project theo trường `status` trong tệp `projects.json` của bộ demo, hoặc để trạng thái mặc định.
3. Người trình bày tắt *Hibernate idle agents* trong Settings → Agents, để agent không bị ngủ giữa buổi demo.
4. Người trình bày tăng cỡ chữ giao diện trong Settings → Appearance nếu trình chiếu trên màn hình lớn.

### 2.4. Danh sách kiểm tra năm phút trước giờ demo

- [ ] Tất cả agent cần dùng đã đăng nhập và chạy được trong terminal.
- [ ] `python smoke.py` chạy đạt trong `atlas-commerce`.
- [ ] Không có phiên cũ nào hiển thị dữ liệu cá nhân hoặc dự án thật.
- [ ] Thông báo hệ điều hành đã bật chế độ không làm phiền, trừ thông báo của Kelpie nếu cần trình diễn.
- [ ] Mạng ổn định, vì các agent gọi model qua internet.

## 3. Kịch bản ngắn: 3 phút

Kịch bản này phù hợp với phần mở đầu của một buổi họp hoặc một video ngắn.

| Thời gian | Thao tác | Lời dẫn gợi ý |
|---|---|---|
| 0:00–0:30 | Mở Kelpie với `atlas-commerce` đang hiển thị: một agent, một shell và cây thư mục. | "Khi làm việc với nhiều coding agent, phần khó nhất không phải là khởi động chúng. Phần khó nhất là nhớ việc nào đang ở đâu. Kelpie gom tất cả vào một bàn làm việc." |
| 0:30–1:00 | Mở tab lưới 2×2 đã chuẩn bị sẵn: Claude Code, Codex, OMP và Claude Code ở chat view đang cùng trả lời câu hỏi về checkout (cảnh 2). | "Bốn agent của ba hãng đang làm việc trên cùng một repository. Tôi dùng chính các CLI mình đã quen; Kelpie không thay model hay tài khoản, nó đặt chúng cạnh nhau." |
| 1:00–1:40 | Mở board bằng `Ctrl+Shift+J`. Chỉ vào hai worktree của `atlas-commerce`. | "Mỗi việc có một worktree riêng, một nhánh riêng và một ghi chú cho biết nó đang làm gì. Các agent không ghi đè lên nhau." |
| 1:40–2:20 | Mở Dashboard bằng `Ctrl+Shift+M`. Nhấn **Answer** trên phiên đang chờ. | "Khi agent cần tôi, tôi không phải lật từng tab. Dashboard xếp phiên chờ lâu nhất lên đầu, và tôi trả lời ngay tại đây." |
| 2:20–3:00 | Mở diff của `src/checkout.py`, thêm một review note và gửi cho agent. | "Khi agent xong việc, tôi review ngay trong Kelpie và gửi nhận xét lại cho agent. Khi Kelpie cập nhật phiên bản mới, các agent này vẫn tiếp tục chạy." |

## 4. Kịch bản đầy đủ: 12–15 phút

Mỗi cảnh dưới đây có thể trình bày độc lập. Người trình bày nên giữ thứ tự từ cảnh 1 đến cảnh 9 cho buổi giới thiệu chung, rồi chọn thêm các cảnh tuỳ chọn theo khán giả.

### Cảnh 1. Màn hình đầu tiên

- **Mục tiêu:** giới thiệu Kelpie và Kip, linh vật của sản phẩm.
- **Chuẩn bị:** mở Kelpie khi chưa có project nào, hoặc dùng ảnh `docs/screenshots/welcome.png`.
- **Thao tác:** người trình bày chỉ vào các nút *Add project folder*, *Add remote project (SSH)*, *SSH hosts* và *Command palette*.
- **Lời dẫn:** "Kelpie là bàn làm việc trên máy tính cho các coding agent. Tôi có thể mở một thư mục trên máy, mở một thư mục trên máy chủ qua SSH, quản lý máy chủ, hoặc tìm mọi thứ bằng một ô tìm kiếm."
- **Kết quả:** người xem thấy màn hình chào và hiểu các cách bắt đầu.

### Cảnh 2. Bốn agent, ba hãng, một màn hình

- **Mục tiêu:** đây là cảnh "wow" của buổi demo. Người xem thấy Claude Code, Codex, OMP và Claude Code ở chat view cùng làm việc trên một repository, trong một tab, ở cùng một thời điểm.
- **Chuẩn bị:** Claude Code, Codex và OMP đã đăng nhập. Người trình bày nên mở mỗi CLI một lần trong `atlas-commerce` trước buổi demo để trả lời câu hỏi "trust this folder".
- **Thao tác:**
  1. Người trình bày chọn `atlas-commerce` và nhấn `Ctrl+Shift+L`, chọn lưới 2×2. Kelpie tạo bốn ô trống, mỗi ô hỏi muốn chạy gì.
  2. Ô 1: người trình bày nhấn **Claude Code**. Ô 2: nhấn **Codex**. Ô 3: nhấn **OMP**. Ô 4: nhấn *More…*, chọn **Claude Code** và đánh dấu *Chat view*.
  3. Người trình bày giao cho mỗi agent một câu hỏi chỉ đọc khác nhau:
     - Claude Code: "Đọc src/checkout.py và docs/checkout-rules.md. Nêu hai ranh giới rủi ro nhất trong tối đa 6 dòng. Không sửa tệp, không chạy lệnh."
     - Codex: "Đọc smoke.py và src/checkout.py. Gợi ý một assertion còn thiếu cho ngưỡng miễn phí vận chuyển, tối đa 6 dòng. Không sửa tệp."
     - OMP: "Đọc README.md và src/checkout.py. Giải thích mục đích demo và hai trường hợp biên, tối đa 8 dòng. Không sửa tệp."
     - Claude Code chat: "Giải thích mục đích của demo trong ba gạch đầu dòng và nêu một trường hợp kiểm tra đầu vào."
  4. Trong lúc bốn agent làm việc, người trình bày chỉ vào sidebar: bốn phiên cùng có chấm xanh dương *working*, kèm model và thời gian hoạt động. Thanh trạng thái đếm "4 working".
  5. Khi các agent trả lời xong, người trình bày nhấn `Ctrl+Shift+Enter` trên từng pane để phóng to và đọc câu trả lời, rồi nhấn lại để trở về lưới.
  6. Người trình bày chỉ vào pane chat: cùng một Claude Code nhưng hiển thị dạng hội thoại, có dòng công cụ *Read*, vòng tròn context và nút đổi model.
- **Lời dẫn:** "Tôi không phải chọn một agent duy nhất. Claude Code, Codex và OMP chạy cạnh nhau với tài khoản và quyền của chính tôi. Cùng một agent có thể chạy trong terminal hoặc trong chat view, tuỳ cách tôi muốn đọc. Sidebar cho tôi biết agent nào đang làm, agent nào đã xong."
- **Kết quả:** người xem thấy lưới bốn agent, bốn câu trả lời thật và sidebar có trạng thái của từng phiên.
- **Ảnh tham chiếu:** `workspace.png`, `claude.png`, `codex.png`, `omp.png`, `chat.png`, `layouts.png`, `new-session.png`.
- **Lưu ý chi phí:** cảnh này gọi model thật của cả ba hãng; mỗi câu hỏi chỉ đọc vài tệp nhỏ nên chi phí thấp.

### Cảnh 3. Biết ngay phiên nào cần mình

- **Mục tiêu:** cho thấy Dashboard và Activity giúp người dùng không bỏ sót câu hỏi của agent.
- **Chuẩn bị:** mở một terminal trong `relay-workers` và gõ `Read-Host "Run the next demo check (y/n)"` (PowerShell) để tạo một lời nhắc chờ trả lời. Với agent thật, người trình bày có thể nhờ agent làm một việc cần xin quyền.
- **Thao tác:**
  1. Người trình bày chuyển sang project khác để phiên đó chạy ở nền.
  2. Người trình bày chỉ vào chấm màu hổ phách ở sidebar và dấu chấm trên chuông status bar.
  3. Người trình bày nhấn `Ctrl+Shift+M` để mở Dashboard, rồi nhấn **Answer** và chọn `y`.
  4. Người trình bày nhấn `Ctrl+Shift+.` để nhảy tới phiên chờ lâu nhất tiếp theo.
- **Lời dẫn:** "Kelpie theo dõi màn hình của từng phiên. Khi một agent hỏi, phiên đó chuyển sang nhóm *Needs you*. Tôi trả lời mà không phải mở đúng tab."
- **Kết quả:** phiên chuyển từ *Needs you* sang *Working* hoặc *Done*.
- **Ảnh tham chiếu:** `dashboard.png`, `inbox.png`.

### Cảnh 4. Làm song song bằng worktree

- **Mục tiêu:** cho thấy hai việc trong cùng repository có thể chạy song song mà không giẫm chân nhau.
- **Thao tác:**
  1. Người trình bày nhấp chuột phải vào `atlas-commerce` và chọn **New worktree…**.
  2. Người trình bày chỉ vào phần `.kelpie.yaml`: Kelpie hiển thị lệnh `setup`, các tab sẽ mở và tệp `.env.local` sẽ được sao chép, rồi chỉ chạy khi người dùng đánh dấu đồng ý.
  3. Người trình bày nhấn `Esc` để không tạo worktree thật, hoặc tạo một worktree nếu còn thời gian.
  4. Người trình bày mở board bằng `Ctrl+Shift+J`, kéo một thẻ từ *Todo* sang *In progress*.
- **Lời dẫn:** "Tên nhánh cho biết code nằm ở đâu. Ghi chú worktree và cột trên board cho biết vì sao việc đó quan trọng lúc này. Agent cũng có thể tự cập nhật ghi chú bằng `kelpied worktree set`."
- **Kết quả:** người xem thấy board với bốn cột *Todo → In progress → Review → Done*.
- **Ảnh tham chiếu:** `worktree-dialog.png`, `board.png`.

### Cảnh 5. Review ngay trong Kelpie

- **Mục tiêu:** cho thấy kết quả của agent được review tại chỗ, kèm ngữ cảnh.
- **Thao tác:**
  1. Người trình bày mở tab **Changes** (`Ctrl+Shift+G`) của `atlas-commerce` và nhấp `src/checkout.py`.
  2. Người trình bày nhấp dấu `+` ở lề dòng có `coupon.strip().upper()` và viết: "Hãy thêm assertion cho mã có khoảng trắng và chữ thường."
  3. Người trình bày nhấn **Send to agent ▾** và chọn agent đang chạy.
  4. Người trình bày nói thêm về **Review with…**: Kelpie có thể nhờ một agent của hãng khác review cùng worktree.
- **Lời dẫn:** "Khi agent báo xong, đó là lúc tôi bắt đầu review chứ không phải lúc bỏ qua review. Ghi chú nằm đúng dòng code và đi thẳng về agent."
- **Kết quả:** agent nhận một prompt chứa vị trí dòng và nội dung ghi chú.
- **Ảnh tham chiếu:** `review-notes.png`, `changes.png`, `composer.png`.

### Cảnh 6. Hoàn tất một worktree bằng Finish

- **Mục tiêu:** cho thấy Kelpie gộp việc đã xong một cách an toàn.
- **Chuẩn bị:** worktree `coupon-validation` đã có commit `fix: accept pasted, lowercase coupon codes`.
- **Thao tác:**
  1. Người trình bày chọn worktree `coupon-validation` trong sidebar và mở palette, chọn *Finish Worktree…*.
  2. Người trình bày chỉ vào cách gộp (squash, merge commit hoặc rebase), thông điệp commit và danh sách check `python smoke.py`.
  3. Người trình bày giải thích rằng check được đọc từ nhánh đích và chỉ chạy sau khi người dùng phê duyệt đúng danh sách đó.
  4. Người trình bày nhấn **Cancel** nếu muốn giữ bộ demo cho lần sau, hoặc chạy Finish nếu muốn trình diễn trọn vẹn.
- **Lời dẫn:** "Finish dừng các phiên của worktree, commit phần còn dở, gộp, chạy check, rồi mới dọn worktree. Kelpie không bao giờ force bất cứ thứ gì."
- **Kết quả:** người xem thấy hộp thoại Finish với check đã được liệt kê.
- **Ảnh tham chiếu:** `finish.png`.
- **Khôi phục:** nếu đã chạy Finish, người trình bày tạo lại bộ demo vào một thư mục trống mới.

### Cảnh 7. Đọc kết quả ở dạng phù hợp

- **Mục tiêu:** cho thấy chat view và các trình xem tệp giúp đọc kết quả dễ hơn terminal.
- **Thao tác:**
  1. Trong pane Claude Code hoặc OMP, người trình bày nhấp biểu tượng chat ở header để chuyển cùng cuộc hội thoại sang chat view.
  2. Người trình bày gửi prompt: "Đọc README.md và src/checkout.py, giải thích mục đích trong ba gạch đầu dòng. Không sửa tệp."
  3. Người trình bày chỉ vào các dòng công cụ, vòng tròn context và nút chọn model.
  4. Người trình bày mở `docs/architecture.md` để thấy sơ đồ Mermaid, mở `data/latency.csv` để thấy bảng có thể sắp xếp, và mở `pulse-analytics/notebooks/revenue-summary.ipynb`.
- **Lời dẫn:** "Đôi khi tôi muốn giao diện terminal của agent, đôi khi tôi muốn đọc như một cuộc trò chuyện. Kelpie chuyển qua lại mà vẫn giữ một người ghi duy nhất cho cuộc hội thoại."
- **Kết quả:** người xem thấy câu trả lời thật của agent và các tệp được hiển thị đúng định dạng.
- **Ảnh tham chiếu:** `chat.png`, `markdown.png`, `csv.png`, `notebook.png`, `editor.png`.

### Cảnh 8. Từ kế hoạch tới orchestration

- **Mục tiêu:** cho thấy Kelpie đọc kế hoạch có sẵn trong repository và điều phối nhiều agent theo thứ tự.
- **Thao tác:**
  1. Người trình bày mở tab **Plan** của `atlas-commerce`. Kelpie hiển thị `plans/checkout-hardening` với hai phase và tiến độ.
  2. Người trình bày tích một ô; tệp Markdown trên đĩa thay đổi đúng một ký tự.
  3. Người trình bày nhấn biểu tượng tia sét ở cuối một task để xem hộp thoại **Run with…**, rồi nhấn `Esc`.
  4. Người trình bày mở terminal ngoài Kelpie và chạy `kelpied run create runs/checkout-hardening.json` trong thư mục `atlas-commerce`, hoặc mở trang **Runs** (`Ctrl+Shift+R`) và tạo run tương tự bằng **New run**.
  5. Người trình bày chỉ vào ba task: hai task chờ phê duyệt, mỗi task có worktree riêng, và task thứ ba chờ hai task đầu.
- **Lời dẫn:** "Kế hoạch là Markdown trong repository, không phải một cơ sở dữ liệu riêng. Khi việc lớn hơn, tôi chia thành task, chọn agent cho từng task và để daemon chạy theo thứ tự phụ thuộc."
- **Kết quả:** trang Runs hiển thị một run đang chờ phê duyệt; chưa agent nào khởi động.
- **Ảnh tham chiếu:** `plan.png`, `runs.png`.
- **Lưu ý chi phí:** nếu người trình bày nhấn **Approve**, agent thật sẽ chạy và tốn token.

### Cảnh 9. Cập nhật không làm gián đoạn agent

- **Mục tiêu:** chứng minh lợi ích chính của daemon tách rời.
- **Cách A, khuyến nghị:** người trình bày cài một bản Kelpie cũ hơn bản mới nhất trên GitHub Releases, từ 0.1.10 trở lên. Bản cũ hơn 0.1.10 chạy daemon từ thư mục cài đặt, nên lần cập nhật đầu tiên từ bản đó sẽ dừng các phiên một lần.
  1. Người trình bày giao cho một agent một việc dài, ví dụ "Viết thêm assertion cho mọi ranh giới trong checkout và chạy smoke test sau mỗi bước."
  2. Người trình bày nhấn *Update x.y.z* trên status bar, đọc release notes và chọn **Install now**.
  3. Kelpie khởi động lại. Người trình bày chỉ vào pane agent: màn hình và cuộc hội thoại vẫn nguyên, agent vẫn đang làm việc.
- **Cách B, khi không có bản cập nhật:** người trình bày bật Settings → Session Daemon → *When Kelpie closes* → **Keep it running in the background**, đóng Kelpie trong lúc agent làm việc, rồi mở lại. Cơ chế này giống với lúc cập nhật: chỉ giao diện khởi động lại, còn daemon giữ các phiên.
- **Lời dẫn:** "Tôi không phải chờ agent làm xong mới dám cập nhật. Kelpie tự cập nhật, cửa sổ mở lại, và agent vẫn đang chạy đúng chỗ cũ. Tôi có thể chọn **Install when idle** để Kelpie tự cập nhật khi không còn phiên nào đang làm việc."
- **Kết quả:** người xem thấy cùng một phiên trước và sau khi app khởi động lại.

## 5. Cảnh tuỳ chọn

### Cảnh 10. Processes, cổng và trình duyệt

- **Mục tiêu:** cho thấy Kelpie theo dõi chương trình mà phiên khởi chạy và mở trang web ngay cạnh agent.
- **Thao tác:**
  1. Trong `beacon-api`, người trình bày mở shell và chạy `python src/app.py --serve`.
  2. Người trình bày mở tab **Processes** của panel bên phải. Kelpie hiển thị cây tiến trình kèm PID, bộ nhớ, CPU và cổng `8765`.
  3. Người trình bày nhấp cổng `8765` để mở trang trong một tab trình duyệt của Kelpie, rồi thêm `/v1/products?category=paper` vào địa chỉ.
  4. Người trình bày bật **Design mode**, nhấp một phần tử trên trang và gửi cho agent kèm ghi chú.
  5. Người trình bày dùng **Kill children** để dừng server; Kelpie hỏi trước và nói rõ cổng nào sẽ được giải phóng.
- **Ảnh tham chiếu:** `processes.png`, `chatgpt.png`.

### Cảnh 11. Thử nhiều agent và chọn kết quả tốt nhất

- **Mục tiêu:** cho thấy cùng một prompt có thể giao cho hai đến năm agent, mỗi agent một worktree.
- **Thao tác:** người trình bày mở palette và chọn *Try with Several Agents…*, nhập prompt "Thêm assertion cho ranh giới 9.999 và 10.000 cent sau giảm giá; không đổi quy tắc giá", rồi chọn Claude Code và Codex. Khi hai task xong, người trình bày mở tab **Compare** của run để so sánh thời gian, chi phí, số dòng thay đổi và check, sau đó chọn **Use this one**.
- **Ảnh tham chiếu:** `try.png`.
- **Lưu ý chi phí:** cảnh này gọi model thật của mọi agent được chọn.

### Cảnh 12. Tuỳ biến bằng plugin, quick command và automation

- **Thao tác:**
  1. Người trình bày mở Settings → Plugins, chọn cài từ thư mục `kelpie-demo-projects/plugins/kelpie-demo-tools`.
  2. Người trình bày nhấn **Review & enable** và chỉ ra rằng Kelpie liệt kê mọi lệnh plugin có thể chạy trước khi bật.
  3. Sau khi bật, người trình bày chọn theme *Kelpie Tide* trong Settings → Terminal và chạy lệnh *Run the smoke checks* từ palette.
  4. Người trình bày mở Settings → Automations và tạo một automation đã tắt, ví dụ "Mỗi sáng thứ Hai, agent tóm tắt commit tuần trước".
- **Ảnh tham chiếu:** `plugins.png`, `automations.png`.

### Cảnh 13. Làm việc trên máy chủ qua SSH

- **Chuẩn bị:** một máy chủ thật, tốt nhất có `tmux`.
- **Thao tác:** người trình bày mở trang **SSH hosts** (`Ctrl+Alt+H`), thêm máy chủ, kiểm tra kết nối và nhấn **Claude Code here**. Sau đó người trình bày mở một project trên máy, chia pane và chọn máy chủ ở dòng **Run on**, để thấy pane trên máy chủ có sọc màu và theme riêng.
- **Lời dẫn:** "Phiên trên máy chủ chạy trong tmux, nên vẫn sống khi mạng rớt hoặc máy tính của tôi ngủ."
- **Ảnh tham chiếu:** `hosts.png`. Ảnh trong bộ demo dùng địa chỉ tài liệu (RFC 5737) và không kết nối tới máy chủ nào.

### Cảnh 14. Điện thoại

- **Chuẩn bị:** bật Settings → Mobile → *Mobile access* và làm theo hai bước *Install* và *Pair*. Người trình bày nên ghép đôi trước buổi demo vì Kelpie yêu cầu xác thực bằng Windows Hello, mật khẩu hoặc Touch ID.
- **Thao tác:** người trình bày tạo một phiên chờ trả lời như ở cảnh 3, rồi để người xem thấy điện thoại rung, mở phiên và trả lời bằng nút `y`.
- **Lời dẫn:** "Nội dung giữa điện thoại và máy tính được mã hoá đầu cuối. Mỗi điện thoại phải được duyệt trên máy tính, và không cần tài khoản Kelpie."

### Cảnh 15. Sub-session: agent giao việc cho agent

- **Thao tác:** trong một phiên Claude Code, người trình bày gõ "Mở hai sub-session bằng Codex: một cái kiểm tra beacon-api, một cái kiểm tra orbit-mobile. Báo cáo kết quả." Kelpie hiển thị thẻ phê duyệt cho mỗi phiên con. Sau khi duyệt, các phiên con xuất hiện dưới phiên cha trong sidebar.
- **Lưu ý chi phí:** mỗi sub-session là một agent thật và tốn token.

### Cảnh 16. Cá nhân hoá

- **Thao tác:** người trình bày đổi giao diện sang theme sáng, đổi ngôn ngữ sang tiếng Việt trong Settings → Appearance, mở Settings → Shortcuts để ghi lại một phím tắt, và bấm `Ctrl+Shift+Space` để đọc chính tả bằng giọng nói nếu đã tải mô hình giọng nói.
- **Ảnh tham chiếu:** `light.png`, `vietnamese.png`, `settings.png`, `shortcuts.png`.

## 6. Kịch bản theo đối tượng

| Đối tượng | Thời lượng | Các cảnh nên dùng |
|---|---|---|
| Khách hàng lần đầu thấy Kelpie | 10 phút | 1, 2, 3, 5, 9 |
| Lập trình viên làm một mình với nhiều agent | 8 phút | 2, 3, 7, 9, 16 |
| Trưởng nhóm quan tâm đến review và chất lượng | 10 phút | 4, 5, 6, 11, 8 |
| Kỹ sư làm việc trên máy chủ | 8 phút | 2, 13, 10, 9 |
| Người hay rời bàn làm việc | 6 phút | 3, 14, 12 |

## 7. Sau buổi demo

1. Người trình bày đóng các phiên demo bằng menu chuột phải hoặc Settings → Session Daemon.
2. Người trình bày xoá các project demo khỏi sidebar nếu không dùng nữa.
3. Nếu đã chạy Finish, Try hoặc Runs, người trình bày tạo lại bộ demo vào một thư mục trống mới để lần sau bắt đầu từ trạng thái sạch.
4. Người trình bày tắt *Mobile access* và gỡ ghép đôi điện thoại nếu đã dùng điện thoại mượn.

## 8. Câu hỏi thường gặp

**Kelpie có kèm model AI không?** Không. Kelpie khởi chạy các CLI agent mà người dùng đã cài và đăng nhập. Chi phí model do tài khoản của agent đó chi trả.

**Kelpie có thu thập dữ liệu sử dụng không?** Không. Kelpie không có telemetry. Các agent vẫn gửi yêu cầu tới nhà cung cấp model của chúng theo chính sách riêng.

**Kelpie hỗ trợ những agent nào?** Kelpie nhận diện Claude Code, Codex, OMP và 20 CLI khác như Gemini, Cursor, OpenCode và GitHub Copilot. Người dùng có thể thêm agent tuỳ chỉnh từ registry Agent Client Protocol hoặc bằng plugin.

**Kelpie chạy trên hệ điều hành nào?** Kelpie chạy trên Windows 10 1809 trở lên, Windows 11, macOS 12 trở lên và Linux x86-64 (AppImage hoặc .deb).

**Điều gì xảy ra khi đóng Kelpie?** Theo mặc định, Kelpie hỏi trước rồi dừng daemon cùng các phiên. Lần mở sau, agent tiếp tục đúng cuộc hội thoại. Người dùng có thể chọn giữ daemon chạy nền. Khi cập nhật, daemon không bao giờ dừng.

**Ảnh trong README có phải kết quả thật không?** Dữ liệu project là dữ liệu mẫu. Output của shell, cổng của server catalog và câu trả lời của Claude Code, Codex, OMP và Claude Code chat là kết quả chạy thật. Tệp [`docs/screenshots/capture.json`](screenshots/capture.json) ghi rõ phần nào là dữ liệu mẫu.
