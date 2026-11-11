# Kịch bản demo Kelpie

Tài liệu này hướng dẫn người trình bày giới thiệu Kelpie bằng bộ demo đi kèm repository; trong các bước bên dưới, người trình bày được gọi là "bạn". Mỗi cảnh ghi rõ mục tiêu, phần chuẩn bị, các bước thao tác và lời dẫn gợi ý, kèm theo kết quả người xem cần thấy và cách đưa màn hình về trạng thái ban đầu.

Bộ demo chỉ dùng dữ liệu mẫu. Mỗi project là một repository Git nhỏ, chạy bằng Python chuẩn và không kết nối dịch vụ bên ngoài. Tuy nhiên, câu trả lời mà agent tạo ra trong buổi demo là kết quả thật, vì vậy bạn không nên hứa trước nội dung câu trả lời của model.

Tài liệu tham chiếu gồm [README](../README.vi.md), [hướng dẫn sử dụng tiếng Việt](user-guide.vi.md) và [hướng dẫn đầy đủ bằng tiếng Anh](user-guide.md).

## 1. Thông điệp chính

Sau buổi demo, người xem nên nhớ ba ý sau:

1. **Một nơi cho mọi agent.** Kelpie chạy Claude Code, Codex, OMP và 20 CLI agent khác cạnh shell, trong nhiều project cùng lúc. Người dùng vẫn giữ nguyên công cụ, tài khoản và quyền hạn của mình.
2. **Luôn biết việc nào cần mình.** Bảng điều khiển Dashboard, chuông Activity và điện thoại cho người dùng biết phiên nào đang chờ trả lời, phiên nào đã xong và phiên nào bị lỗi.
3. **Cập nhật không làm gián đoạn công việc.** Các phiên chạy trong daemon `kelpied`, một tiến trình nền tách khỏi cửa sổ, nên khi Kelpie tự cập nhật thì chỉ giao diện khởi động lại. Agent vẫn tiếp tục làm việc trong lúc đó, và người dùng mở lại đúng màn hình cũ.

Ý thứ ba là lợi ích thực tế nhất của kiến trúc daemon. Bạn không nên nhấn mạnh việc "đóng cửa sổ rồi bỏ đi", vì người viết code thường muốn mở Kelpie để theo dõi tiến độ. Tuỳ chọn chạy nền khi đóng cửa sổ vẫn tồn tại, nhưng bạn chỉ nên nhắc ngắn khi nói về automation hoặc điện thoại.

## 2. Chuẩn bị

### 2.1. Phần mềm

- Bạn cài Kelpie từ [trang Releases](https://github.com/baopeak/kelpie/releases) hoặc build từ mã nguồn theo README.
- Bạn cũng cài và đăng nhập sẵn ít nhất hai agent CLI, ví dụ Claude Code và Codex, rồi kiểm tra rằng Settings → Agents hiển thị chúng là *installed*.
- Máy cần có Python 3.10 trở lên và Git.
- Cảnh điện thoại cần một điện thoại có camera, còn cảnh SSH cần một máy chủ thật mà bạn có quyền đăng nhập.

### 2.2. Tạo bộ demo

Từ thư mục gốc của repository, bạn chạy hai lệnh sau. Thư mục đích phải chưa tồn tại hoặc đang trống.

```bash
python scripts/e2e/readme_demo.py ../kelpie-demo-projects
python ../kelpie-demo-projects/atlas-commerce/smoke.py
```

Lệnh thứ nhất tạo tám project Git, hai worktree và một plugin mẫu, còn lệnh thứ hai kiểm tra rằng project chính chạy được và phải in ra `PASS atlas-commerce`.

| Thành phần | Nội dung | Dùng trong cảnh |
|---|---|---|
| `atlas-commerce` | Tính giá giỏ hàng, mã giảm giá và ngưỡng miễn phí vận chuyển; có một thay đổi chưa commit để duyệt | 3, 4, 5, 6, 7, 8 |
| `atlas-commerce/.kelpie.yaml` | `setup`, `tabs`, `copy` (`.env.local`) và `check` (`python smoke.py`) | 4, 6 |
| `atlas-commerce/plans/checkout-hardening/` | Kế hoạch hai giai đoạn (phase) với các ô đánh dấu | 8 |
| `atlas-commerce/runs/checkout-hardening.json` | Một run gồm ba nhiệm vụ (task), mỗi nhiệm vụ chờ phê duyệt hoặc chờ nhiệm vụ trước | 8 |
| `atlas-commerce/docs/` | Markdown có sơ đồ Mermaid và tệp `checkout-flow.mmd` | 7 |
| `worktrees/coupon-validation` | Nhánh `feature/coupon-validation` đã có một commit hoàn chỉnh | 6 |
| `worktrees/shipping-threshold` | Nhánh `fix/shipping-threshold` chưa có thay đổi | 4 |
| `beacon-api` | API catalog nhỏ; `python src/app.py --serve` mở cổng 8765 | 10 |
| `pulse-analytics` | Dữ liệu CSV và notebook `notebooks/revenue-summary.ipynb` | 7 |
| `relay-workers` | Hàng đợi công việc đơn giản, dùng để tạo một phiên "cần bạn" | 3 |
| Bốn project còn lại | `orbit-mobile`, `prism-design`, `harbor-infra` và `field-notes` làm cho thanh bên và board trông như một ngày làm việc thật | 2, 4 |
| `plugins/kelpie-demo-tools/` | Plugin khai báo một lệnh, một quick command, một theme terminal và một mục trên thanh trạng thái | 12 |

### 2.3. Đưa dữ liệu vào Kelpie

1. Bạn mở Kelpie, nhấn `+` ở thanh bên rồi thêm từng thư mục project. Kelpie tự thêm hai worktree của `atlas-commerce` vào dưới project cha.
2. Tiếp theo, bạn ghim `atlas-commerce` và đặt trạng thái board cho các project theo trường `status` trong tệp `projects.json` của bộ demo, hoặc giữ trạng thái mặc định.
3. Trong Settings → Agents, bạn tắt *Hibernate idle agents* để agent không bị ngủ giữa buổi demo.
4. Nếu trình chiếu trên màn hình lớn, bạn nên tăng cỡ chữ giao diện trong Settings → Appearance.

### 2.4. Danh sách kiểm tra năm phút trước giờ demo

- [ ] Tất cả agent cần dùng đã đăng nhập và chạy được trong terminal.
- [ ] `python smoke.py` chạy đạt trong `atlas-commerce`.
- [ ] Không có phiên cũ nào hiển thị dữ liệu cá nhân hoặc dự án thật.
- [ ] Hệ điều hành đã bật chế độ không làm phiền, trừ thông báo của Kelpie nếu bạn cần trình diễn chúng.
- [ ] Mạng phải ổn định vì các agent gọi model qua internet.

## 3. Kịch bản ngắn: 3 phút

Kịch bản này phù hợp với phần mở đầu của một buổi họp hoặc một video ngắn.

| Thời gian | Thao tác | Lời dẫn gợi ý |
|---|---|---|
| 0:00–0:30 | Kelpie đang mở `atlas-commerce` với một agent, một shell và cây thư mục. | "Khi làm việc với nhiều coding agent, phần khó nhất không phải là khởi động chúng mà là nhớ việc nào đang ở đâu. Kelpie gom tất cả vào một bàn làm việc." |
| 0:30–1:00 | Bạn mở tab lưới 2×2 đã chuẩn bị sẵn, nơi Claude Code, Codex, OMP và Claude Code ở chat view đang cùng trả lời câu hỏi về checkout (cảnh 2). | "Bốn agent của ba hãng đang cùng làm việc trên một repository. Tôi vẫn dùng chính các CLI mình đã quen, vì Kelpie không thay model hay tài khoản mà chỉ đặt chúng cạnh nhau." |
| 1:00–1:40 | Bạn mở board bằng `Ctrl+Shift+J` rồi chỉ vào hai worktree của `atlas-commerce`. | "Mỗi việc có một worktree riêng, một nhánh riêng và một ghi chú cho biết nó đang làm gì, nhờ đó các agent không ghi đè lên nhau." |
| 1:40–2:20 | Sau khi mở Dashboard bằng `Ctrl+Shift+M`, bạn nhấn **Answer** trên phiên đang chờ. | "Khi agent cần tôi, tôi không phải lật từng tab. Bảng điều khiển này xếp phiên chờ lâu nhất lên đầu, và tôi trả lời ngay tại đây." |
| 2:20–3:00 | Trong diff của `src/checkout.py`, bạn thêm một ghi chú duyệt (review note) và gửi cho agent. | "Khi agent xong việc, tôi duyệt kết quả ngay trong Kelpie và gửi nhận xét lại cho agent. Ngoài ra, khi Kelpie cập nhật lên phiên bản mới, các agent này vẫn tiếp tục chạy." |

## 4. Kịch bản đầy đủ: 12–15 phút

Bạn có thể trình bày riêng từng cảnh dưới đây. Với buổi giới thiệu chung, bạn nên giữ thứ tự từ cảnh 1 đến cảnh 9, sau đó chọn thêm các cảnh tuỳ chọn theo khán giả.

### Cảnh 1. Màn hình đầu tiên

- **Mục tiêu:** Cảnh này giới thiệu Kelpie và Kip, linh vật của sản phẩm.
- **Chuẩn bị:** Bạn mở Kelpie khi chưa có project nào, hoặc dùng ảnh `docs/screenshots/welcome.png`.
- **Thao tác:** Trên màn hình chào, bạn chỉ vào các nút *Add project folder*, *Add remote project (SSH)*, *SSH hosts* và *Command palette*.
- **Lời dẫn:** "Kelpie là bàn làm việc trên máy tính dành cho các coding agent. Từ màn hình này, tôi có thể mở một thư mục trên máy hoặc trên máy chủ qua SSH, quản lý các máy chủ, hoặc tìm mọi thứ bằng một ô tìm kiếm."
- **Kết quả:** Người xem thấy màn hình chào và hiểu các cách bắt đầu.

### Cảnh 2. Bốn agent, ba hãng, một màn hình

- **Mục tiêu:** Đây là cảnh tạo ấn tượng chính của buổi demo, vì người xem thấy Claude Code, Codex, OMP và Claude Code ở chat view cùng làm việc trên một repository, trong một tab và vào cùng một lúc.
- **Chuẩn bị:** Claude Code, Codex và OMP đều đã đăng nhập. Trước buổi demo, bạn nên mở mỗi CLI một lần trong `atlas-commerce` để trả lời trước câu hỏi "trust this folder".
- **Thao tác:**
  1. Bạn chọn `atlas-commerce`, nhấn `Ctrl+Shift+L` và chọn lưới 2×2. Kelpie tạo bốn ô trống, và mỗi ô hỏi bạn muốn chạy gì.
  2. Ở ô 1 bạn nhấn **Claude Code**, ở ô 2 nhấn **Codex**, còn ở ô 3 nhấn **OMP**. Với ô 4, bạn nhấn *More…*, chọn **Claude Code** và đánh dấu *Chat view*.
  3. Tiếp theo, bạn giao cho mỗi agent một câu hỏi chỉ đọc khác nhau:
     - Claude Code: "Đọc src/checkout.py và docs/checkout-rules.md. Nêu hai ranh giới rủi ro nhất trong tối đa 6 dòng. Không sửa tệp, không chạy lệnh."
     - Codex: "Đọc smoke.py và src/checkout.py. Gợi ý một assertion còn thiếu cho ngưỡng miễn phí vận chuyển, tối đa 6 dòng. Không sửa tệp."
     - OMP: "Đọc README.md và src/checkout.py. Giải thích mục đích demo và hai trường hợp biên, tối đa 8 dòng. Không sửa tệp."
     - Claude Code chat: "Giải thích mục đích của demo trong ba gạch đầu dòng và nêu một trường hợp kiểm tra đầu vào."
  4. Trong lúc bốn agent làm việc, bạn chỉ vào thanh bên, nơi cả bốn phiên cùng có chấm xanh dương *working* kèm model và thời gian hoạt động. Thanh trạng thái lúc này đếm "4 working".
  5. Khi các agent trả lời xong, bạn nhấn `Ctrl+Shift+Enter` trên từng khung để phóng to và đọc câu trả lời, rồi nhấn lại để trở về lưới.
  6. Cuối cùng, bạn chỉ vào khung chat. Đây vẫn là Claude Code nhưng hiển thị dạng hội thoại, có dòng công cụ *Read*, vòng tròn context và nút đổi model.
- **Lời dẫn:** "Tôi không phải chọn một agent duy nhất. Claude Code, Codex và OMP chạy cạnh nhau bằng tài khoản và quyền của chính tôi. Cùng một agent có thể chạy trong terminal hoặc trong chat view, tuỳ cách tôi muốn đọc. Trong khi đó, thanh bên cho tôi biết agent nào đang làm và agent nào đã xong."
- **Kết quả:** Người xem thấy lưới bốn agent, bốn câu trả lời thật và trạng thái của từng phiên trên thanh bên.
- **Ảnh tham chiếu:** `workspace.png`, `claude.png`, `codex.png`, `omp.png`, `chat.png`, `layouts.png`, `new-session.png`.
- **Lưu ý chi phí:** Cảnh này gọi model thật của cả ba hãng, nhưng chi phí thấp vì mỗi câu hỏi chỉ đọc vài tệp nhỏ.

### Cảnh 3. Biết ngay phiên nào cần mình

- **Mục tiêu:** Cảnh này cho thấy Dashboard và Activity giúp người dùng không bỏ sót câu hỏi của agent.
- **Chuẩn bị:** Bạn mở một terminal trong `relay-workers` và gõ `Read-Host "Run the next demo check (y/n)"` (PowerShell) để tạo một lời nhắc chờ trả lời. Nếu dùng agent thật, bạn có thể nhờ agent làm một việc cần xin quyền.
- **Thao tác:**
  1. Bạn chuyển sang project khác để phiên đó chạy ở nền.
  2. Sau đó, bạn chỉ vào chấm màu hổ phách ở thanh bên và dấu chấm trên chuông ở thanh trạng thái.
  3. Bạn nhấn `Ctrl+Shift+M` để mở Dashboard, rồi nhấn **Answer** và chọn `y`.
  4. Cuối cùng, phím `Ctrl+Shift+.` đưa bạn tới phiên chờ lâu nhất tiếp theo.
- **Lời dẫn:** "Kelpie theo dõi màn hình của từng phiên, nên khi một agent đặt câu hỏi thì phiên đó chuyển sang nhóm *Needs you*. Nhờ vậy, tôi trả lời được mà không phải mở đúng tab của phiên đó."
- **Kết quả:** Phiên chuyển từ *Needs you* sang *Working* hoặc *Done*.
- **Ảnh tham chiếu:** `dashboard.png`, `inbox.png`.

### Cảnh 4. Làm song song bằng worktree

- **Mục tiêu:** Cảnh này cho thấy hai việc trong cùng một repository có thể chạy song song mà không giẫm chân nhau.
- **Thao tác:**
  1. Bạn nhấp chuột phải vào `atlas-commerce` và chọn **New worktree…**.
  2. Trong hộp thoại, bạn chỉ vào phần `.kelpie.yaml`. Kelpie liệt kê lệnh `setup`, các tab sẽ mở và tệp `.env.local` sẽ được sao chép, nhưng chỉ chạy chúng khi người dùng đánh dấu đồng ý.
  3. Sau đó bạn nhấn `Esc` để không tạo worktree thật, hoặc tạo một worktree nếu còn thời gian.
  4. Khi mở board bằng `Ctrl+Shift+J`, bạn kéo một thẻ từ *Todo* sang *In progress*.
- **Lời dẫn:** "Tên nhánh cho biết code nằm ở đâu, còn ghi chú worktree và cột trên board cho biết vì sao việc đó quan trọng lúc này. Agent cũng có thể tự cập nhật ghi chú bằng lệnh `kelpied worktree set`."
- **Kết quả:** Người xem thấy board với bốn cột *Todo → In progress → Review → Done*.
- **Ảnh tham chiếu:** `worktree-dialog.png`, `board.png`.

### Cảnh 5. Duyệt kết quả ngay trong Kelpie

- **Mục tiêu:** Cảnh này cho thấy kết quả của agent được duyệt ngay tại chỗ, kèm đầy đủ ngữ cảnh.
- **Thao tác:**
  1. Bạn mở tab **Changes** (`Ctrl+Shift+G`) của `atlas-commerce` và nhấp vào `src/checkout.py`.
  2. Ở lề dòng có `coupon.strip().upper()`, bạn nhấp dấu `+` và viết: "Hãy thêm assertion cho mã có khoảng trắng và chữ thường."
  3. Sau đó bạn nhấn **Send to agent ▾** và chọn agent đang chạy.
  4. Bạn có thể nói thêm về **Review with…**, tính năng cho phép Kelpie nhờ một agent của hãng khác duyệt cùng worktree.
- **Lời dẫn:** "Khi agent báo xong, đó là lúc tôi bắt đầu duyệt chứ không phải lúc bỏ qua bước duyệt. Ghi chú nằm đúng dòng code và được gửi thẳng về agent."
- **Kết quả:** Agent nhận một yêu cầu chứa vị trí dòng và nội dung ghi chú.
- **Ảnh tham chiếu:** `review-notes.png`, `changes.png`, `composer.png`.

### Cảnh 6. Hoàn tất một worktree bằng Finish

- **Mục tiêu:** Cảnh này cho thấy Kelpie gộp việc đã xong một cách an toàn.
- **Chuẩn bị:** Worktree `coupon-validation` đã có commit `fix: accept pasted, lowercase coupon codes`.
- **Thao tác:**
  1. Bạn chọn worktree `coupon-validation` trong thanh bên, mở palette và chọn *Finish Worktree…*.
  2. Trong hộp thoại Finish, bạn chỉ vào cách gộp (squash, merge commit hoặc rebase), thông điệp commit và danh sách check `python smoke.py`.
  3. Tiếp theo, bạn giải thích rằng Kelpie đọc check từ nhánh đích và chỉ chạy chúng sau khi người dùng phê duyệt đúng danh sách đó.
  4. Nếu muốn giữ bộ demo cho lần sau, bạn nhấn **Cancel**; còn nếu muốn trình diễn trọn vẹn, bạn chạy Finish.
- **Lời dẫn:** "Finish dừng các phiên của worktree, commit phần còn dở, gộp nhánh và chạy check, sau đó mới dọn worktree. Trong suốt quá trình này, Kelpie không bao giờ dùng tuỳ chọn force ở bất kỳ bước nào."
- **Kết quả:** Người xem thấy hộp thoại Finish với danh sách check đã được liệt kê.
- **Ảnh tham chiếu:** `finish.png`.
- **Khôi phục:** Nếu đã chạy Finish, bạn tạo lại bộ demo vào một thư mục trống mới.

### Cảnh 7. Đọc kết quả ở dạng phù hợp

- **Mục tiêu:** Cảnh này cho thấy chat view và các trình xem tệp giúp đọc kết quả dễ hơn so với terminal.
- **Thao tác:**
  1. Trong khung Claude Code hoặc OMP, bạn nhấp biểu tượng chat trên thanh tiêu đề để chuyển cùng cuộc hội thoại sang chat view.
  2. Bạn gửi yêu cầu: "Đọc README.md và src/checkout.py, giải thích mục đích trong ba gạch đầu dòng. Không sửa tệp."
  3. Khi agent trả lời, bạn chỉ vào các dòng công cụ, vòng tròn context và nút chọn model.
  4. Sau đó bạn mở `docs/architecture.md` để thấy sơ đồ Mermaid, mở `data/latency.csv` để thấy bảng có thể sắp xếp, rồi mở `pulse-analytics/notebooks/revenue-summary.ipynb`.
- **Lời dẫn:** "Có lúc tôi muốn xem giao diện terminal của agent, có lúc tôi lại muốn đọc như một cuộc trò chuyện. Kelpie chuyển qua lại giữa hai cách xem, và tại mỗi thời điểm chỉ có một nơi được ghi vào cuộc hội thoại."
- **Kết quả:** Người xem thấy câu trả lời thật của agent, còn các tệp được hiển thị đúng định dạng.
- **Ảnh tham chiếu:** `chat.png`, `markdown.png`, `csv.png`, `notebook.png`, `editor.png`.

### Cảnh 8. Từ kế hoạch tới điều phối nhiều agent

- **Mục tiêu:** Cảnh này cho thấy Kelpie đọc kế hoạch có sẵn trong repository và điều phối nhiều agent theo thứ tự.
- **Thao tác:**
  1. Bạn mở tab **Plan** của `atlas-commerce`, nơi Kelpie hiển thị `plans/checkout-hardening` với hai giai đoạn và tiến độ.
  2. Khi bạn tích một ô, tệp Markdown trên đĩa thay đổi đúng một ký tự.
  3. Tiếp theo, bạn nhấn biểu tượng tia sét ở cuối một nhiệm vụ để xem hộp thoại **Run with…**, rồi nhấn `Esc`.
  4. Để tạo run, bạn mở terminal ngoài Kelpie và chạy `kelpied run create runs/checkout-hardening.json` trong thư mục `atlas-commerce`. Ngoài ra, bạn có thể mở trang **Runs** (`Ctrl+Shift+R`) và tạo run tương tự bằng **New run**.
  5. Cuối cùng, bạn chỉ ra rằng hai nhiệm vụ đầu đang chờ phê duyệt và mỗi nhiệm vụ có worktree riêng, còn nhiệm vụ thứ ba chờ hai nhiệm vụ đầu.
- **Lời dẫn:** "Kế hoạch là các tệp Markdown nằm trong repository chứ không phải một cơ sở dữ liệu riêng. Khi việc lớn hơn, tôi chia nó thành nhiều nhiệm vụ, chọn agent cho từng nhiệm vụ và để daemon chạy chúng theo thứ tự phụ thuộc."
- **Kết quả:** Trang Runs hiển thị một run đang chờ phê duyệt và chưa có agent nào khởi động.
- **Ảnh tham chiếu:** `plan.png`, `runs.png`.
- **Lưu ý chi phí:** Nếu bạn nhấn **Approve & start**, agent thật sẽ chạy và tốn token.

### Cảnh 9. Cập nhật không làm gián đoạn agent

- **Mục tiêu:** Cảnh này chứng minh lợi ích chính của việc tách daemon khỏi cửa sổ.
- **Cách A, khuyến nghị:** Bạn cài một bản Kelpie cũ hơn bản mới nhất trên GitHub Releases, nhưng phải từ 0.1.10 trở lên. Vì các bản cũ hơn 0.1.10 chạy daemon từ thư mục cài đặt, lần cập nhật đầu tiên từ những bản đó sẽ dừng các phiên một lần.
  1. Bạn giao cho một agent một việc dài, ví dụ "Viết thêm assertion cho mọi ranh giới trong checkout và chạy smoke test sau mỗi bước."
  2. Trên thanh trạng thái, bạn nhấn *Update x.y.z*, đọc ghi chú phát hành và chọn **Install now**.
  3. Sau khi Kelpie khởi động lại, bạn chỉ vào khung agent để người xem thấy màn hình và cuộc hội thoại vẫn nguyên, còn agent vẫn đang làm việc.
- **Cách B, khi không có bản cập nhật:** Bạn bật Settings → Session Daemon → *When Kelpie closes* → **Keep it running in the background**, đóng Kelpie trong lúc agent làm việc rồi mở lại. Cơ chế này giống với lúc cập nhật, vì chỉ giao diện khởi động lại còn daemon vẫn giữ các phiên.
- **Lời dẫn:** "Tôi không phải chờ agent làm xong mới dám cập nhật. Khi Kelpie tự cập nhật, cửa sổ mở lại và agent vẫn đang chạy đúng chỗ cũ. Nếu muốn, tôi có thể chọn **Install when idle** để Kelpie tự cập nhật khi không còn phiên nào đang làm việc."
- **Kết quả:** Người xem thấy cùng một phiên trước và sau khi ứng dụng khởi động lại.

## 5. Cảnh tuỳ chọn

### Cảnh 10. Processes, cổng và trình duyệt

- **Mục tiêu:** Cảnh này cho thấy Kelpie theo dõi các chương trình mà phiên khởi chạy và mở trang web ngay cạnh agent.
- **Thao tác:**
  1. Trong `beacon-api`, bạn mở shell và chạy `python src/app.py --serve`.
  2. Khi bạn mở tab **Processes** của panel bên phải, Kelpie hiển thị cây tiến trình kèm PID, bộ nhớ, CPU và cổng `8765`.
  3. Bạn nhấp cổng `8765` để mở trang trong một tab trình duyệt của Kelpie, rồi thêm `/v1/products?category=paper` vào địa chỉ.
  4. Sau đó bạn bật **Design mode**, nhấp một phần tử trên trang và gửi phần tử đó cho agent kèm ghi chú.
  5. Cuối cùng, bạn dùng **Kill children** để dừng server; Kelpie sẽ hỏi trước và nói rõ cổng nào được giải phóng.
- **Ảnh tham chiếu:** `processes.png`, `chatgpt.png`.

### Cảnh 11. Thử nhiều agent và chọn kết quả tốt nhất

- **Mục tiêu:** Cảnh này cho thấy cùng một yêu cầu có thể được giao cho hai đến năm agent, mỗi agent làm trong một worktree riêng.
- **Thao tác:** Bạn mở palette, chọn *Try with Several Agents…*, nhập yêu cầu "Thêm assertion cho ranh giới 9.999 và 10.000 cent sau giảm giá; không đổi quy tắc giá", rồi chọn Claude Code và Codex. Khi hai nhiệm vụ xong, bạn mở tab **Compare** của run để so sánh thời gian, chi phí, số dòng thay đổi và kết quả check, sau đó chọn **Use this one**.
- **Ảnh tham chiếu:** `try.png`.
- **Lưu ý chi phí:** Cảnh này gọi model thật của mọi agent được chọn.

### Cảnh 12. Tuỳ biến bằng plugin, quick command và automation

- **Thao tác:**
  1. Bạn mở Settings → Plugins và chọn cài từ thư mục `kelpie-demo-projects/plugins/kelpie-demo-tools`.
  2. Khi bạn nhấn **Review & enable**, Kelpie liệt kê mọi lệnh mà plugin có thể chạy trước khi bật, và bạn chỉ cho người xem thấy danh sách đó.
  3. Sau khi bật plugin, bạn chọn theme *Kelpie Tide* trong Settings → Terminal và chạy lệnh *Run the smoke checks* từ palette.
  4. Cuối cùng, bạn mở Settings → Automations và tạo một automation ở trạng thái tắt, ví dụ "Mỗi sáng thứ Hai, agent tóm tắt commit tuần trước".
- **Ảnh tham chiếu:** `plugins.png`, `automations.png`.

### Cảnh 13. Làm việc trên máy chủ qua SSH

- **Chuẩn bị:** Bạn cần một máy chủ thật, tốt nhất là máy có `tmux`.
- **Thao tác:** Bạn mở trang **SSH hosts** (`Ctrl+Alt+H`), thêm máy chủ, kiểm tra kết nối và nhấn **Claude Code here**. Sau đó, bạn mở một project trên máy, chia khung và chọn máy chủ ở dòng **Run on**. Khi đó người xem thấy khung chạy trên máy chủ có sọc màu và theme riêng.
- **Lời dẫn:** "Phiên trên máy chủ chạy trong tmux, nên nó vẫn tiếp tục khi mạng rớt hoặc khi máy tính của tôi ngủ."
- **Ảnh tham chiếu:** `hosts.png`. Ảnh trong bộ demo dùng địa chỉ dành cho tài liệu (RFC 5737) và không kết nối tới máy chủ nào.

### Cảnh 14. Điện thoại

- **Chuẩn bị:** Bạn bật Settings → Mobile → *Mobile access* và làm theo hai bước *Install* và *Pair*. Vì Kelpie yêu cầu xác thực bằng Windows Hello, mật khẩu hoặc Touch ID, bạn nên ghép đôi điện thoại trước buổi demo.
- **Thao tác:** Bạn tạo một phiên chờ trả lời như ở cảnh 3, rồi để người xem thấy điện thoại rung. Sau đó bạn mở phiên trên điện thoại và trả lời bằng nút `y`.
- **Lời dẫn:** "Nội dung truyền giữa điện thoại và máy tính được mã hoá đầu cuối. Mỗi điện thoại phải được duyệt trên máy tính, và tôi không cần tài khoản Kelpie nào."

### Cảnh 15. Sub-session: agent giao việc cho agent

- **Thao tác:** Trong một phiên Claude Code, bạn gõ "Mở hai sub-session bằng Codex: một cái kiểm tra beacon-api, một cái kiểm tra orbit-mobile. Báo cáo kết quả." Kelpie hiển thị thẻ phê duyệt cho từng phiên con, và sau khi bạn duyệt, các phiên con xuất hiện dưới phiên cha trong thanh bên.
- **Lưu ý chi phí:** Mỗi sub-session là một agent thật nên đều tốn token.

### Cảnh 16. Cá nhân hoá

- **Thao tác:** Bạn đổi giao diện sang theme sáng và đổi ngôn ngữ sang tiếng Việt trong Settings → Appearance. Sau đó bạn mở Settings → Shortcuts để ghi lại một phím tắt, và nếu đã tải mô hình giọng nói thì bấm `Ctrl+Shift+Space` để đọc chính tả.
- **Ảnh tham chiếu:** `light.png`, `vietnamese.png`, `settings.png`, `shortcuts.png`.

## 6. Kịch bản theo đối tượng

| Đối tượng | Thời lượng | Các cảnh nên dùng |
|---|---|---|
| Khách hàng lần đầu thấy Kelpie | 10 phút | 1, 2, 3, 5, 9 |
| Lập trình viên làm một mình với nhiều agent | 8 phút | 2, 3, 7, 9, 16 |
| Trưởng nhóm quan tâm đến việc duyệt code và chất lượng | 10 phút | 4, 5, 6, 11, 8 |
| Kỹ sư làm việc trên máy chủ | 8 phút | 2, 13, 10, 9 |
| Người hay rời bàn làm việc | 6 phút | 3, 14, 12 |

## 7. Sau buổi demo

1. Bạn đóng các phiên demo bằng menu chuột phải hoặc trong Settings → Session Daemon.
2. Nếu không dùng nữa, bạn xoá các project demo khỏi thanh bên.
3. Nếu đã chạy Finish, Try hoặc Runs, bạn tạo lại bộ demo vào một thư mục trống mới để lần sau bắt đầu từ trạng thái sạch.
4. Trường hợp đã dùng điện thoại mượn, bạn tắt *Mobile access* và gỡ ghép đôi điện thoại đó.

## 8. Câu hỏi thường gặp

**Kelpie có kèm model AI không?** Kelpie không kèm model AI nào. Ứng dụng khởi chạy các CLI agent mà người dùng đã cài và đăng nhập, vì vậy chi phí model do tài khoản của từng agent chi trả.

**Kelpie có thu thập dữ liệu sử dụng không?** Kelpie không thu thập dữ liệu sử dụng và không có telemetry (cơ chế gửi số liệu sử dụng về nhà phát triển). Tuy nhiên, các agent vẫn gửi yêu cầu tới nhà cung cấp model của chúng theo chính sách riêng.

**Kelpie hỗ trợ những agent nào?** Kelpie nhận diện Claude Code, Codex, OMP và 20 CLI khác như Gemini, Cursor, OpenCode và GitHub Copilot. Ngoài ra, người dùng có thể thêm agent tuỳ chỉnh từ registry Agent Client Protocol hoặc bằng plugin.

**Kelpie chạy trên hệ điều hành nào?** Kelpie chạy trên Windows 10 1809 trở lên, Windows 11, macOS 12 trở lên và Linux x86-64 (AppImage hoặc .deb).

**Điều gì xảy ra khi đóng Kelpie?** Theo mặc định, Kelpie hỏi trước rồi dừng daemon cùng các phiên, và lần mở sau agent tiếp tục đúng cuộc hội thoại. Người dùng cũng có thể chọn giữ daemon chạy nền. Riêng khi cập nhật, daemon không bao giờ dừng.

**Ảnh trong README có phải kết quả thật không?** Dữ liệu project là dữ liệu mẫu, còn kết quả in ra của shell, cổng của server catalog và câu trả lời của Claude Code, Codex, OMP và Claude Code chat đều là kết quả chạy thật. Tệp [`docs/screenshots/capture.json`](screenshots/capture.json) ghi rõ phần nào là dữ liệu mẫu.
