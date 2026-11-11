<p align="center"><img src="app/src/assets/brand/kip-peek.svg" alt="Kip, linh vật của Kelpie, ló ra từ một cửa sổ terminal" width="120"></p>

<h1 align="center">Kelpie</h1>

<p align="center"><strong>Mọi coding agent bạn dùng, đặt cạnh nhau, trong một bàn làm việc không bao giờ để sót việc.</strong></p>

<p align="center">
  <a href="https://github.com/baopeak/kelpie/releases"><img alt="Bản phát hành mới nhất" src="https://img.shields.io/github/v/release/baopeak/kelpie?label=t%E1%BA%A3i%20v%E1%BB%81"></a>
  <img alt="Windows, macOS và Linux" src="https://img.shields.io/badge/n%E1%BB%81n%20t%E1%BA%A3ng-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-4c7a8a">
  <a href="LICENSE"><img alt="Giấy phép MIT" src="https://img.shields.io/badge/gi%E1%BA%A5y%20ph%C3%A9p-MIT-4c7a8a"></a>
</p>

<p align="center">
  <a href="https://github.com/baopeak/kelpie/releases"><strong>Tải Kelpie</strong></a> ·
  <a href="#xem-kelpie-hoạt-động">Xem Kelpie hoạt động</a> ·
  <a href="#dùng-thử-bộ-demo">Dùng thử bộ demo</a> ·
  <a href="docs/user-guide.vi.md">Hướng dẫn sử dụng</a> ·
  <a href="README.md">English</a>
</p>

![Bốn agent cùng làm việc trong một tab Kelpie: Claude Code, Codex và OMP trong terminal riêng, Claude Code trong chat view, cùng review phần checkout, cạnh tám project demo](docs/screenshots/workspace.png)

Bạn mở một agent trong terminal. Sau đó, agent thứ hai nhận một việc khác, bài test cần một shell riêng, một lỗi cần một nhánh khác, và trình duyệt bị che khuất sau tất cả. Lúc này, việc khó nhất không còn là khởi động công việc. Việc khó nhất là nhớ công việc đang nằm ở đâu: agent nào đang chờ câu trả lời, bản checkout nào thuộc về việc nào, và phần nào thật sự đã sẵn sàng để review.

**Kelpie là bàn làm việc trên máy tính dành cho đúng khoảnh khắc đó.** Kelpie chạy Claude Code, Codex, OMP và hai mươi CLI agent khác bên cạnh shell của bạn, trên mọi project và worktree. Kelpie cho bạn biết phiên nào cần bạn, giúp bạn review những gì agent đã thay đổi và gộp phần việc đã xong một cách an toàn. Các phiên chạy trong một tiến trình nền riêng, vì vậy **việc cập nhật Kelpie không bao giờ làm gián đoạn agent đang làm việc**.

## Vì sao nên dùng Kelpie

- **Bạn giữ nguyên công cụ của mình.** Kelpie khởi chạy các CLI agent bạn đã cài, với tài khoản, gói thuê bao và quy tắc cấp quyền của chính bạn. Kelpie không kèm model và không yêu cầu tài khoản.
- **Bạn thấy ngay việc nào cần mình.** Dashboard xếp mọi phiên vào các nhóm *Needs you*, *Working*, *Done* và *Failed*. Bạn có thể trả lời một lời nhắc đang chờ mà không cần mở tab của nó, từ chuông trên thanh trạng thái hoặc từ điện thoại.
- **Các việc song song luôn tách bạch.** Mỗi việc có thể có Git worktree riêng, phiên riêng và một ghi chú ngắn cho biết vì sao việc đó quan trọng. Board cho thấy mọi việc đi từ *Todo* tới *Done*.
- **Review là một phần của quy trình.** Bạn đọc diff, để lại ghi chú ở đúng dòng cần hỏi và gửi mọi ghi chú cho agent trong một yêu cầu. Khi việc đã xong, **Finish** commit, gộp nhánh, chạy các check bạn đã duyệt và dọn dẹp worktree.
- **Cập nhật không dừng công việc.** Daemon `kelpied` sở hữu mọi tiến trình. Khi Kelpie cài bản cập nhật, chỉ cửa sổ khởi động lại, và mọi agent vẫn đang chạy khi cửa sổ mở ra. (Ngoại lệ duy nhất được mô tả ở phần [Cài đặt và cập nhật](docs/user-guide.vi.md#cài-đặt-và-cập-nhật): lần cập nhật đầu tiên từ một bản 0.1 đời đầu sẽ dừng các phiên một lần.)

## Bên trong có gì

| | |
|---|---|
| **Không gian nhiều agent** | Tab, chia pane, lưới mẫu tới 3×3 hoặc lưới tuỳ ý, layout đã lưu, gõ đồng thời vào nhiều terminal, terminal nổi và một ô tìm kiếm cho phiên, tệp, lệnh và cài đặt. |
| **Terminal hay chat tuỳ bạn** | Các agent được hỗ trợ có thể chuyển giữa giao diện terminal riêng và chat view của Kelpie trong cùng một cuộc hội thoại, kèm thẻ công cụ, phê duyệt, chọn model và hình ảnh. |
| **Báo đúng lúc cần** | Dashboard, trả lời nhanh, thông báo trên máy tính và điện thoại, cùng tin nhắn ntfy, Telegram hoặc webhook cho biết khi một phiên đang chờ, đã xong hoặc bị lỗi. |
| **Worktree và board** | Worktree mới có thể tự chạy lệnh setup, sao chép tệp `.env` và dùng chung thư mục theo `.kelpie.yaml`, và mọi worktree đều hiện trên board. |
| **Review và Finish** | Ghi chú trên diff, nhờ agent của hãng khác review, hoàn tác từng lượt của agent, pull request qua `gh`, merge request GitLab và thao tác gộp có check bảo vệ. |
| **Điều phối** | Run chia một việc lớn thành các task có phụ thuộc và cần phê duyệt. *Try with several agents* giao một prompt cho tối đa năm agent rồi so sánh kết quả. Agent còn có thể mở **sub-session** mà bạn nhìn thấy, điều khiển và đóng được. |
| **Kế hoạch nằm trong repository** | Tab Plan đọc checklist Markdown trong `plans/`, `specs/`, `.kiro/specs/` và `docs/plans/`, cho phép bạn tích ô và giao task cho agent. |
| **Tệp, trình duyệt và tiến trình** | Trình soạn thảo có trình xem Markdown, Mermaid, CSV, PDF và notebook; pane trình duyệt có chế độ thiết kế; panel Processes cho thấy chương trình và cổng của từng phiên. |
| **Làm việc từ xa** | Project qua SSH, trang quản lý máy chủ kiểu Termius với khoá, chuyển tiếp cổng và truyền tệp, cùng pane agent trên bất kỳ máy chủ nào, được giữ sống bằng `tmux`. |
| **Điện thoại của bạn** | Ứng dụng điện thoại tuỳ chọn, mã hoá đầu cuối, liệt kê các phiên, rung khi có phiên cần bạn và cho phép bạn trả lời. |
| **Theo cách của bạn** | Agent setup cho MCP server và tệp hướng dẫn, plugin, quick command, automation, phím tắt, theme, đọc chính tả offline, xuất cài đặt, và giao diện tiếng Anh hoặc tiếng Việt. |

## Xem Kelpie hoạt động

Phần dưới đây theo chân một thay đổi nhỏ ở phần checkout qua một ngày làm việc. Các project và dữ liệu kế hoạch là dữ liệu mẫu. Output của shell và mọi câu trả lời của agent là kết quả chạy thật với tài khoản thật, và tệp [capture.json](docs/screenshots/capture.json) ghi rõ phần nào là dữ liệu mẫu.

### Cho mỗi việc một chỗ riêng

Bạn mở project, khởi động một agent và đặt một shell bên cạnh. Lời giải thích, kết quả test và nhánh Git giờ hiện cùng lúc. Khi việc lớn hơn, bạn chia pane tiếp hoặc sắp xếp tab thành lưới. Khi một chi tiết cần toàn bộ sự chú ý, bạn phóng to pane đó mà không dừng các pane bên cạnh.

Trong ảnh đầu trang, bốn agent cùng làm việc trên một phần checkout, trong ba CLI agent khác nhau: Claude Code, Codex và OMP trong giao diện terminal riêng, và một phiên Claude Code thứ hai trong chat view của Kelpie. Kelpie khởi chạy mỗi CLI đã cài đúng như bản gốc của nó, và một phiên PowerShell hoặc Bash bình thường cũng có chỗ trong không gian làm việc như một agent.

<details>
<summary><strong>Các phiên trong ảnh: Claude Code, Codex, OMP, Claude Code chat và PowerShell</strong></summary>

**Claude Code** giữ nguyên giao diện terminal gốc và chỉ ra các ranh giới rủi ro nhất của phần checkout.

![Câu trả lời thật của Claude Code trong giao diện terminal riêng bên trong Kelpie](docs/screenshots/claude.png)

**Codex** đọc các smoke check và gợi ý một assertion còn thiếu cho ngưỡng miễn phí vận chuyển.

![Câu trả lời thật của Codex CLI bên trong Kelpie](docs/screenshots/codex.png)

**OMP** đọc mã checkout và giải thích quy tắc vận chuyển và làm tròn.

![Câu trả lời thật của OMP trong terminal và hoạt động đọc tệp bên trong Kelpie](docs/screenshots/omp.png)

**Claude Code trong chat view** trả lời cùng loại câu hỏi dưới dạng hội thoại, kèm các dòng công cụ và nút chọn model; [phần chat view](#đọc-kết-quả-ở-dạng-phù-hợp-nhất) cho thấy giao diện này.

**PowerShell 7** chạy các check của bộ mẫu và in ra một báo giá checkout thật.

![PowerShell 7 hiển thị phiên bản, các check đạt và báo giá checkout](docs/screenshots/powershell.png)

</details>

<details>
<summary><strong>Bắt đầu từ màn hình chào, chọn agent, chọn layout</strong></summary>

![Màn hình chào của Kelpie với Kip, các nút project và SSH, và command palette](docs/screenshots/welcome.png)

![Trình khởi chạy phiên mới với các agent đã cài và tuỳ chọn vị trí](docs/screenshots/new-session.png)

![Các lựa chọn layout pane trên một không gian đã chia](docs/screenshots/layouts.png)

![Menu chuột phải của terminal với sao chép transcript, chia pane và quick command](docs/screenshots/terminal-menu.png)

![Terminal nổi kiểm tra Git worktree và chạy check của bộ mẫu](docs/screenshots/floating.png)

</details>

### Để các việc đi những con đường riêng

Thay đổi ở checkout tách thành hai câu hỏi: mã giảm giá nên được chuẩn hoá thế nào, và ngưỡng miễn phí vận chuyển bắt đầu từ đâu? Cả hai câu hỏi thuộc cùng một project, nhưng chúng không cần dùng chung một bản checkout.

Bạn tạo một Git worktree cho mỗi việc. Kelpie đặt các worktree dưới project cha, mỗi worktree có phiên riêng và một ghi chú giải thích công việc. Board biến các thư mục đó thành một bức tranh dễ đọc: **Todo → In progress → Review → Done**.

![Board với tám project mẫu và hai worktree trải qua các cột Todo, In progress, Review và Done](docs/screenshots/board.png)

<details>
<summary><strong>Từ worktree mới tới kế hoạch và run</strong></summary>

Hộp thoại tạo worktree hiển thị phần setup trong `.kelpie.yaml` của repository và chỉ chạy nó sau khi bạn duyệt đúng tệp đó.

![Hộp thoại tạo worktree với nhánh, điểm gốc, thư mục và tuỳ chọn setup](docs/screenshots/worktree-dialog.png)

Tab Plan đọc checklist từ Markdown trong repository. Khi bạn tích một task, tệp trên đĩa chỉ thay đổi đúng một ký tự.

![Các task đã xong và đang chờ trong tab Plan của Kelpie](docs/screenshots/plan.png)

Một run chia việc lớn thành các task cho agent. Trong run này, hai task chờ bạn phê duyệt và mỗi task sẽ có worktree riêng khi bắt đầu chạy, còn task review chéo chờ cả hai task đầu.

![Trang Runs với một run checkout gồm ba task đang chờ phê duyệt](docs/screenshots/runs.png)

</details>

### Biết phiên nào đang cần bạn

Trong lúc các agent làm việc, bạn xem việc khác. Khi một phiên đặt câu hỏi, phiên đó chuyển sang *Needs you* trên dashboard, dòng của nó trên sidebar chuyển màu hổ phách, và chuông trên thanh trạng thái có thêm một chấm. Bạn trả lời lời nhắc ngay từ dashboard mà không mở tab của nó, và `Ctrl+Shift+.` đưa bạn tới phiên tiếp theo đang chờ.

![Dashboard với một lời nhắc xác nhận thật của shell đang chờ trả lời](docs/screenshots/dashboard.png)

<details>
<summary><strong>Activity, automation và điện thoại</strong></summary>

![Hộp Activity với các thông báo mẫu được ghi nhãn rõ ràng](docs/screenshots/inbox.png)

Các automation mẫu dưới đây đang tắt, nên chúng không thể bất ngờ gọi model có tính phí.

![Ba automation mẫu đang tắt cùng agent và lịch chạy của chúng](docs/screenshots/automations.png)

Bạn cũng có thể [ghép đôi một điện thoại](docs/user-guide.vi.md#điện-thoại). Điện thoại liệt kê các phiên, rung khi có phiên cần bạn, hiển thị màn hình của phiên và, nếu bạn cho phép, gửi câu trả lời.

</details>

### Review kết quả, không chỉ đọc chữ "xong"

Agent báo đã xong. Khoảnh khắc đó là lúc bạn bắt đầu review, không phải lý do để bỏ qua review.

Bạn mở diff, xem dòng đã đổi và để lại ghi chú ở đúng chỗ cần hỏi. Kelpie gửi mọi ghi chú cho agent trong một yêu cầu tập trung. Bạn cũng có thể nhờ một agent thứ hai của hãng khác review cùng worktree và để lại ghi chú riêng trên diff.

![Diff phần checkout với một ghi chú review về chuẩn hoá mã giảm giá và panel Changes bên cạnh](docs/screenshots/review-notes.png)

Khi việc đã sẵn sàng, **Finish** commit phần còn lại, gộp nhánh theo cách bạn chọn, chạy các check trong `.kelpie.yaml` mà bạn đã duyệt và chỉ xoá worktree sau khi mọi bước khác thành công. Finish không bao giờ dùng force.

![Hộp thoại Finish của worktree coupon-validation với cách gộp và check đã duyệt](docs/screenshots/finish.png)

<details>
<summary><strong>Xem thay đổi, soạn yêu cầu tiếp theo, thử nhiều agent</strong></summary>

![Panel Changes với thay đổi checkout chưa commit của repository mẫu](docs/screenshots/changes.png)

![Composer nổi với một yêu cầu review checkout đang soạn](docs/screenshots/composer.png)

*Try with several agents* giao một prompt cho hai tới năm agent, mỗi agent trong worktree riêng. Màn hình Compare sau đó đặt thời gian, chi phí, số dòng thay đổi và kết quả check cạnh nhau.

![Hộp thoại Try with several agents với prompt về ranh giới miễn phí vận chuyển](docs/screenshots/try.png)

</details>

### Đọc kết quả ở dạng phù hợp nhất

Đôi khi bạn muốn giao diện terminal của agent. Đôi khi bạn muốn đọc như một cuộc trò chuyện, có Markdown được hiển thị, hoạt động công cụ và nút chọn model. Các agent được hỗ trợ có thể chuyển sang chat view của Kelpie và quay lại trong cùng cuộc hội thoại, và Kelpie bảo đảm chỉ một giao diện ghi vào cuộc hội thoại đó.

![Câu trả lời thật của Claude Code trong chat view của Kelpie, kèm hoạt động công cụ, vòng tròn context và nút chọn model](docs/screenshots/chat.png)

<details>
<summary><strong>Mã nguồn, sơ đồ, dữ liệu, notebook và tìm kiếm</strong></summary>

![Mã checkout trong trình soạn thảo của Kelpie](docs/screenshots/editor.png)

![Tài liệu kiến trúc Markdown với sơ đồ Mermaid đã hiển thị](docs/screenshots/markdown.png)

![Trình xem CSV hiển thị số liệu demo tổng hợp](docs/screenshots/csv.png)

![Notebook tổng hợp doanh thu theo ngày hiển thị trong Kelpie](docs/screenshots/notebook.png)

![Tìm kiếm toàn cục tìm thấy các project và phiên mẫu](docs/screenshots/search.png)

</details>

### Mang trình duyệt và máy chủ vào cùng một chỗ

Câu hỏi tiếp theo nằm trên một trang web. Bạn mở trang đó cạnh agent thay vì che khuất không gian làm việc sau một ứng dụng khác. Trong ảnh, trang web ChatGPT và CLI của OMP là hai pane riêng trong Kelpie. Trang web là trang thật, chưa đăng nhập; đó không phải một client model đi kèm.

![Trang web ChatGPT và bản review checkout thật của OMP đặt cạnh nhau trong pane trình duyệt và pane terminal](docs/screenshots/chatgpt.png)

Khi agent khởi động một dev server, trang web của nó được mở trong một tab nền riêng. Panel **Processes** cho thấy mỗi phiên đang chạy gì, kèm PID, bộ nhớ, CPU và cổng, và chỉ dừng một chương trình sau khi nói rõ những gì sẽ kết thúc.

![Panel Processes hiển thị server catalog mẫu và cổng 8765 của nó](docs/screenshots/processes.png)

Khi công việc chuyển lên máy chủ, trang **SSH hosts** giữ máy chủ, khoá, chuyển tiếp cổng và truyền tệp ở một nơi. Pane agent trên máy chủ chạy trong `tmux`, nên vẫn sống khi kết nối bị rớt.

![Trang SSH hosts với ba máy chủ demo được nhóm theo môi trường](docs/screenshots/hosts.png)

### Cập nhật mà không dừng công việc

Kelpie tự kiểm tra bản phát hành mới ở chế độ nền. Khi bạn chọn **Install now** hoặc **Install when idle**, chỉ cửa sổ khởi động lại. Daemon giữ mọi terminal và agent tiếp tục chạy, và cửa sổ kết nối lại đúng các màn hình cũ. Bạn không bao giờ phải chờ agent làm xong rồi mới dám cập nhật. Ngoại lệ duy nhất là lần cập nhật đầu tiên từ một bản 0.1 đời đầu, khi daemon còn chạy từ thư mục cài đặt: lần cập nhật đó dừng các phiên một lần, báo trước cho bạn, và agent tiếp tục sau đó.

Khi bạn thoát Kelpie, Kelpie hỏi trước nếu còn việc đang chạy, rồi dừng các phiên. Lần mở sau, agent tiếp tục đúng cuộc hội thoại cũ. Nếu bạn muốn automation, truy cập từ điện thoại hoặc việc sao chép tệp tiếp tục khi cửa sổ đóng, bạn có thể [giữ daemon chạy nền](docs/user-guide.vi.md#cài-đặt-và-cập-nhật).

### Biến Kelpie thành không gian của bạn

Bàn làm việc là nơi bạn ngồi nhiều giờ. Bạn có thể chọn giao diện tối hoặc sáng, chỉnh font và màu terminal, đổi mật độ hiển thị và cỡ chữ, và ghi lại phím tắt hợp với bàn phím của mình. Giao diện có tiếng Anh và tiếng Việt.

![Cài đặt giao diện với ngôn ngữ, theme, mật độ, cỡ chữ và linh vật](docs/screenshots/settings.png)

<details>
<summary><strong>Font, agent, plugin, phím tắt, theme sáng và tiếng Việt</strong></summary>

![Cài đặt terminal với font, shell và bảng màu](docs/screenshots/terminal-settings.png)

![Các agent đã cài và có sẵn trong Settings](docs/screenshots/agents.png)

![Plugin demo trong Settings đang chờ được review](docs/screenshots/plugins.png)

![Phím tắt có thể tuỳ chỉnh](docs/screenshots/shortcuts.png)

![Không gian nhiều pane với theme sáng](docs/screenshots/light.png)

![Không gian nhiều pane với giao diện tiếng Việt](docs/screenshots/vietnamese.png)

</details>

## Danh sách đầy đủ tính năng

<details>
<summary><strong>Mọi tính năng, kèm liên kết tới hướng dẫn sử dụng</strong></summary>

Liên kết dẫn tới [user guide tiếng Anh](docs/user-guide.md), nơi mô tả đầy đủ mọi chi tiết. [Hướng dẫn tiếng Việt](docs/user-guide.vi.md) tóm tắt cùng các nhóm tính năng theo từng việc.

**Không gian làm việc**
- [Project và worktree](docs/user-guide.md#everyday-use) hiện trên sidebar, kèm trạng thái, tiêu đề việc, model và lần hoạt động cuối của từng phiên.
- [Tab, chia pane, lưới và layout đã lưu](docs/user-guide.md#layouts) cho phép bạn sắp xếp bao nhiêu phiên tuỳ ý, và *Broadcast input* gõ cùng lúc vào mọi terminal của một tab.
- [Tìm kiếm mọi thứ](docs/user-guide.md#everyday-use) tìm phiên, project, tệp, lệnh, cài đặt và các prompt cũ trong một ô.
- [Hội thoại cũ](docs/user-guide.md#past-conversations) của Claude Code, Codex và OMP có thể được tìm và tiếp tục trong terminal hoặc chat view.

**Agent**
- Kelpie nhận diện [23 CLI agent](docs/user-guide.md#agents), trong đó có Claude Code, Codex, OMP, Gemini, Cursor, OpenCode và GitHub Copilot, và có thể thêm agent từ registry Agent Client Protocol.
- [Chat view](docs/user-guide.md#everyday-use) tiếp tục cùng cuộc hội thoại với thẻ công cụ, phê duyệt, bản đồ agent, câu trả lời đã lưu và hình ảnh.
- [Nhiều tài khoản](docs/user-guide.md#several-claude-code--codex-accounts) Claude Code hoặc Codex có thể dùng song song, và một phiên có thể chuyển sang tài khoản khác khi chạm giới hạn sử dụng.
- [Sub-session](docs/user-guide.md#sub-sessions) cho phép agent mở phiên agent khác, với sự đồng ý của bạn và trong giới hạn bạn đặt.
- [Hand-off, hibernation, compact và restart](docs/user-guide.md#agents) giữ cho các cuộc hội thoại dài luôn khoẻ.

**Theo dõi và chi phí**
- [Dashboard và trả lời nhanh](docs/user-guide.md#everyday-use) cho thấy mọi phiên đang chờ bạn, phiên chờ lâu nhất đứng đầu.
- [Activity và thông báo](docs/user-guide.md#activity-and-notifications) tới được máy tính, ntfy, Telegram hoặc một webhook.
- [Context, token và chi phí](docs/user-guide.md#context-tokens-and-cost) hiện trên từng phiên, kèm trang thống kê và giới hạn chi phí tuỳ chọn.

**Review và Git**
- [Panel Changes](docs/user-guide.md#source-control-and-review) push, pull, đổi nhánh, hiện lịch sử và stash, soạn thông điệp commit và mở pull request.
- [Ghi chú review, hàng đợi review và Finish](docs/user-guide.md#review-and-finish) đưa một worktree từ "xong" tới "đã gộp".
- [Review với agent khác và chọn kết quả tốt nhất](docs/user-guide.md#review-with-another-agent-and-best-of-several) so sánh các lần làm độc lập.
- [Lượt và hoàn tác](docs/user-guide.md#turns-and-undo) khôi phục những gì một lượt của agent đã thay đổi, sau khi cho bạn xem trước.

**Kế hoạch và tự động hoá**
- [Run](docs/user-guide.md#orchestration-runs) khởi động các task của agent theo thứ tự phụ thuộc.
- [Tab Plan](docs/user-guide.md#specs-and-tasks-the-plan-tab) đọc, tích và giao cho agent các checklist trong repository của bạn.
- [Automation](docs/user-guide.md#automations) khởi động agent hoặc lệnh đã duyệt theo lịch, hoặc khi một tệp, commit, check hay worktree thay đổi.
- [Issue tracker](docs/user-guide.md#issue-trackers-and-gitlab) kết nối Linear, Jira và GitLab với phiên và worktree mới.

**Tệp, trình duyệt và tiến trình**
- [Trình soạn thảo và trình xem tệp](docs/user-guide.md#files) mở mã nguồn, Markdown, Mermaid, CSV, PDF và notebook Jupyter.
- [Pane trình duyệt](docs/user-guide.md#browser-panes-and-design-mode) có công cụ nhà phát triển và chế độ thiết kế để gửi phần tử đã chọn cho agent.
- [Panel Processes](docs/user-guide.md#processes-ports-and-preview) liệt kê chương trình và cổng của từng phiên và dừng chúng một cách an toàn.

**Làm việc từ xa và điện thoại**
- [Project từ xa](docs/user-guide.md#remote-projects-ssh), [trang SSH hosts](docs/user-guide.md#ssh-hosts) và [pane SSH trong mọi project](docs/user-guide.md#ssh-panes-in-any-project) đưa máy chủ vào cùng không gian làm việc.
- [Giữ phiên chạy trên máy chủ](docs/user-guide.md#keeping-sessions-running-on-a-server) giải thích cách thiết lập `tmux`.
- [Truy cập từ điện thoại](docs/user-guide.md#phone-access) ghép đôi điện thoại qua relay của Kelpie, Tailscale hoặc mạng nội bộ.

**Thiết lập cá nhân**
- [Agent setup](docs/user-guide.md#agent-setup) quản lý MCP server, tệp hướng dẫn và skill của mọi agent ở một nơi.
- [Plugin](docs/user-guide.md#plugins), [quick command](docs/user-guide.md#quick-commands) và [composer](docs/user-guide.md#composer) mở rộng những gì bạn có thể khởi chạy và gửi đi.
- [Đọc chính tả](docs/user-guide.md#voice-dictation) nhận dạng giọng nói ngay trên máy; không có gì được gửi đi.
- [Xuất và nhập cài đặt](docs/user-guide.md#export-and-import-settings) chuyển thiết lập sang máy khác mà không mang theo bí mật.
- [Phím tắt](docs/user-guide.md#keyboard-shortcuts) đều có thể thay đổi.

</details>

## Bắt đầu trong ba bước

1. **[Tải Kelpie](https://github.com/baopeak/kelpie/releases).** Bạn chạy bộ cài trên Windows, kéo ứng dụng vào Applications trên macOS, hoặc dùng gói AppImage hay `.deb` trên Linux.
2. **Cài và đăng nhập các CLI agent bạn muốn dùng.** Kelpie tìm chúng qua `PATH` hoặc qua login shell. Settings → Agents cho biết Kelpie đã tìm thấy những agent nào.
3. **Thêm một thư mục project và mở một phiên.** Bạn đặt shell cạnh agent, giao việc cho agent và theo dõi kết quả ngay trên màn hình.

Kelpie hỗ trợ Windows 10 1809 trở lên và Windows 11, macOS 12 trở lên, và Linux x86-64 với glibc 2.35 trở lên. Windows là nền tảng được kiểm thử thực tế nhiều nhất. GitHub CLI (`gh`) là tuỳ chọn và bật các tính năng pull request. Phiên trên máy chủ dùng OpenSSH của bạn và, để phiên sống lâu trên máy chủ, dùng `tmux`.

## Dùng thử bộ demo

Bộ demo tạo **tám project Git nhỏ, hai worktree và một plugin mẫu** trong thư mục bạn chọn. Mọi project chạy bằng thư viện chuẩn của Python và có smoke check, tài liệu kiến trúc Markdown với sơ đồ Mermaid, bộ dữ liệu CSV tổng hợp và checklist phát hành. Các project minh hoạ nghiệp vụ; chúng không phải ứng dụng thật.

| Project | Vai trò trong câu chuyện |
|---|---|
| `atlas-commerce` | Báo giá checkout, chuẩn hoá mã giảm giá và ngưỡng miễn phí vận chuyển. Project có `.kelpie.yaml` đầy đủ, một kế hoạch hai phase, một tệp run, hai worktree (một worktree đã có commit hoàn chỉnh) và một thay đổi chưa commit để review. |
| `beacon-api` | Catalog sản phẩm có lọc theo danh mục và một server cục bộ tuỳ chọn ở cổng 8765. |
| `orbit-mobile` | Đồng bộ ghi chú offline có báo xung đột. |
| `prism-design` | Token màu và kiểm tra độ tương phản. |
| `harbor-infra` | Lập kế hoạch dung lượng và triển khai cho môi trường staging. |
| `pulse-analytics` | Tổng hợp doanh thu theo ngày, xuất CSV và một notebook. |
| `relay-workers` | Hàng đợi job xác định và kế hoạch chính sách thử lại. |
| `field-notes` | Chỉ mục ghi chú nghiên cứu Markdown có tìm kiếm. |

Bạn tạo bộ demo vào một thư mục mới, trống, nằm ngoài repository này:

```bash
python scripts/e2e/readme_demo.py ../kelpie-demo-projects
python ../kelpie-demo-projects/atlas-commerce/smoke.py
```

Sau đó, bạn thêm các thư mục vào Kelpie và làm theo **[kịch bản demo](docs/demo-scripts.vi.md)**. Kịch bản có phần giới thiệu ba phút, phần trình diễn mười lăm phút và các cảnh tuỳ chọn; mỗi cảnh có các bước thao tác, lời dẫn và kết quả cần thấy.

<details>
<summary><strong>Chụp lại ảnh màn hình</strong></summary>

[Script chụp ảnh](scripts/e2e/readme_shots.py) điều khiển ứng dụng thật qua WebDriver. Script cần bản build của app và daemon, `tauri-driver`, và `msedgedriver` khớp với phiên bản WebView2 đã cài:

```bash
cd app && pnpm tauri build --debug --no-bundle && cd .. && cargo build -p kelpied
python -X utf8 scripts/e2e/readme_shots.py docs/screenshots ../kelpie-demo-projects \
  --app target/debug/kelpie.exe --native-driver /path/to/msedgedriver.exe \
  --agents --browser
```

Script dùng thư mục dữ liệu, endpoint daemon và thư mục SSH trống của riêng nó, nên không bao giờ đụng tới phiên, khoá hay cài đặt của bạn. Ảnh được chụp ở độ phân giải Full HD (1920 × 1080). `--agents` khởi động Claude Code, Codex và OMP trong terminal và Claude Code trong chat view, mỗi agent làm một việc chỉ đọc, và gọi model thật bằng tài khoản của bạn; bạn bỏ `--agents` và `--browser` nếu muốn chụp mà không dùng agent. Claude Code và Codex có thể hỏi một lần có tin tưởng thư mục demo không, và script trả lời đồng ý. Các agent vẫn lưu transcript của những lượt demo này trong thư mục riêng của chúng, như mọi phiên khác của bạn. Trang ChatGPT không đăng nhập. Các máy chủ SSH dùng địa chỉ dành cho tài liệu và không bao giờ được kết nối. Script ghi [capture.json](docs/screenshots/capture.json) với phiên bản app, danh sách ảnh và mọi check đã chạy.

</details>

## Cách Kelpie hoạt động

Kelpie tách cửa sổ khỏi công việc. Giao diện SolidJS hiển thị không gian làm việc. Daemon Rust `kelpied` sở hữu terminal, tiến trình agent, automation, run và kết nối điện thoại, và cửa sổ giao tiếp với daemon qua named pipe hoặc Unix socket cục bộ.

Một terminal không hiện trên màn hình thì không cần vẽ. Khi bạn quay lại terminal đó, cửa sổ nhận phần output còn thiếu hoặc một ảnh chụp trạng thái, thay vì bắt agent làm lại từ đầu. Nhờ sự tách biệt này, một bản cập nhật có thể khởi động lại cửa sổ trong khi mọi agent vẫn làm việc, và không gian làm việc vẫn nhanh khi số phiên tăng lên. Điều đó không có nghĩa là thêm agent thì không tốn gì; mỗi agent vẫn dùng bộ nhớ, CPU và token.

Tài liệu [kiến trúc](docs/architecture.md) mô tả quyền sở hữu tiến trình, giao thức truyền và cơ chế khôi phục, còn [quy trình đo hiệu năng](scripts/perf/README.md) mô tả phương pháp đo và ngân sách hiệu năng.

## Công cụ của bạn, dữ liệu của bạn, quyết định của bạn

- **Không gian làm việc ở lại trên máy bạn.** Phiên, layout và cài đặt được lưu cục bộ, và Kelpie không gửi telemetry. Các CLI agent vẫn liên hệ nhà cung cấp model theo tài khoản và chính sách của chúng.
- **Mọi kết nối đều do bạn bật.** Máy chủ từ xa, issue tracker, kênh thông báo và truy cập từ điện thoại đều cần bạn thiết lập, và truy cập từ điện thoại mặc định tắt.
- **Kết nối điện thoại được mã hoá đầu cuối.** Bạn duyệt từng điện thoại trên máy tính bằng cách nhập mã điện thoại hiển thị và vượt qua bước xác thực của hệ điều hành, như Windows Hello hoặc Touch ID. Kelpie không cần tài khoản. Relay kết nối điện thoại qua internet chỉ chuyển tiếp các khung đã mã hoá mà nó không đọc được, nhưng relay thấy siêu dữ liệu kết nối, và mã của ứng dụng điện thoại được tải từ relay đó. [Hướng dẫn truy cập từ điện thoại](docs/user-guide.md#phone-access) giải thích mô hình tin cậy này, phương án Tailscale và cách kiểm tra những gì relay phục vụ.
- **Bí mật được xử lý cẩn thận, trong giới hạn.** Token tích hợp, mật khẩu SSH và token thông báo được lưu trong kho thông tin xác thực của hệ điều hành. Biến môi trường bạn nhập trong cài đặt agent được lưu dạng văn bản thường, vì vậy bạn không nên đặt bí mật ở đó hoặc trong project demo.
- **Review vẫn là việc của bạn.** Setup worktree, check khi Finish, lệnh automation, plugin và quyền của agent đều hỏi ý bạn. Kelpie sắp xếp công việc; Kelpie không tự làm cho output của agent trở nên đáng tin.

Nếu bạn nâng cấp từ tên cũ Erka, hãy thoát Erka trước khi mở Kelpie. Các tệp `.erka.yaml` hiện có vẫn hoạt động.

## Build từ mã nguồn

Bạn cần Rust stable, Node.js 22 trở lên, pnpm 11 và bộ công cụ build gốc của nền tảng: MSVC trên Windows, công cụ của Apple trên macOS, và các gói WebKitGTK liệt kê trong [user guide](docs/user-guide.md#build-from-source) trên Linux.

```bash
# Cài phụ thuộc frontend
cd app && pnpm install --frozen-lockfile && cd ..

# Shell 1: daemon phát triển tách biệt
bash scripts/dev-daemon.sh run

# Shell 2: ứng dụng desktop kết nối tới daemon đó
cd app && KELPIE_ENDPOINT=kelpie-dev pnpm tauri dev
```

Trong PowerShell, bạn đặt `$env:KELPIE_ENDPOINT = 'kelpie-dev'` trước lệnh desktop. Chỉ chạy Vite thì không dùng được các API desktop gốc.

Để build bộ cài, bạn dùng [script phát hành](scripts/build-release.sh):

```bash
KELPIE_KEEP_VERSION=1 bash scripts/build-release.sh --bundles nsis
# macOS: --bundles app,dmg    Linux: --bundles appimage,deb
```

Nếu không đặt `KELPIE_KEEP_VERSION=1`, script phát hành sẽ tăng số phiên bản patch. Script cũng build các sidecar gốc, nên đây không phải lệnh kiểm tra nhanh.

### Kiểm thử và xác minh trên ứng dụng thật

```bash
# Giao thức Rust, daemon và backend desktop
cargo test -p kelpie-proto -p kelpied -p kelpie

# Frontend, chạy trong app/
pnpm build
pnpm test
pnpm i18n:check
```

CI chạy bộ test Rust trên Windows và macOS. Một lần chạy đạt trên một nền tảng không thay thế được kiểm thử trên nền tảng kia. `cargo test --workspace` còn bao gồm trình hỗ trợ nhận dạng giọng nói. Các kịch bản chạy trên ứng dụng thật nằm trong [scripts/e2e](scripts/e2e); bạn nên đọc điều kiện tiên quyết của chúng trước, vì một số kịch bản dùng agent đã đăng nhập, máy chủ thử nghiệm hoặc lượt gọi model có tính phí. Bạn nên giữ daemon thử nghiệm tách biệt khỏi các phiên của mình.

### Điều khiển Kelpie bằng script

Agent và script cục bộ có thể xem phiên và báo tiến độ qua `kelpied`:

```bash
kelpied terminal list
kelpied terminal read 3 --lines 80
kelpied worktree set --comment "checkout ready for review" --status review
kelpied run create runs/checkout-hardening.json
kelpied sub spawn codex "check the catalog filter" --worktree
```

Lệnh trong phạm vi một phiên và lệnh quản trị chỉ dành cho người dùng có quyền khác nhau. Bạn nên đọc phần [tự động hoá và CLI](docs/user-guide.md#automation) trước khi viết script điều khiển các phiên khác.

## Tìm hiểu thêm

- [Hướng dẫn sử dụng tiếng Việt](docs/user-guide.vi.md) và [user guide tiếng Anh](docs/user-guide.md) mô tả công việc hằng ngày, worktree, chat, máy chủ từ xa, điện thoại, giọng nói và các tích hợp.
- [Kịch bản demo](docs/demo-scripts.vi.md) giúp bạn trình bày Kelpie bằng bộ demo.
- [Kiến trúc](docs/architecture.md) giải thích quyền sở hữu tiến trình, bảo mật, giao thức truyền, phát lại và khôi phục.
- [Plugin](docs/plugins.md) mô tả tiện ích mở rộng dạng khai báo và cách review chúng.
- [Tự host relay](docs/relay-self-host.md) và [giao thức liên kết](docs/link-protocol.md) mô tả kết nối điện thoại.
- [Thương hiệu](docs/brand.md) giới thiệu bộ nhận diện và Kip.
- [Hiệu năng](scripts/perf/README.md) ghi lại các phép đo có thể lặp lại thay cho con số quảng cáo.

## Giấy phép và ghi nhận

Kelpie được phát hành theo [giấy phép MIT](LICENSE). Các tệp nhị phân ConPTY cho Windows đi kèm thuộc về Microsoft và cũng theo giấy phép MIT; xem [ghi chú ConPTY](docs/third-party-conpty.md).

Claude, Codex, Gemini, Cursor, OpenCode, GitHub Copilot và logo của chúng là nhãn hiệu của chủ sở hữu tương ứng. Kelpie khởi chạy các công cụ này, nhưng Kelpie không liên kết với chủ sở hữu của chúng và không được các chủ sở hữu đó bảo trợ. Xem [ghi nhận logo agent](docs/third-party-agent-logos.md).
