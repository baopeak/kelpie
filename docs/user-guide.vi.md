# Hướng dẫn sử dụng Kelpie

Kelpie chạy nhiều agent lập trình (coding agent) như Claude Code, Codex, OMP và 20 CLI khác, cùng với các shell, trên nhiều project. Vì các phiên chạy trong daemon `kelpied`, một tiến trình nền tách khỏi cửa sổ, nên việc chuyển project, cập nhật Kelpie hay lỗi giao diện đều không làm phiên dừng lại. Ngoài ra, ứng dụng chỉ vẽ giao diện của project mà bạn đang xem.

Tài liệu này hướng dẫn các thao tác thường dùng theo từng việc, còn [bản hướng dẫn tiếng Anh](user-guide.md) mô tả đầy đủ mọi chi tiết, giới hạn và trường hợp đặc biệt. Cuối mỗi phần dưới đây đều có liên kết tới phần tương ứng của bản tiếng Anh. Nếu bạn muốn trình diễn Kelpie, hãy xem [kịch bản trình diễn](demo-scripts.vi.md).

Trong tài liệu này, phím tắt được ghi theo Windows và Linux. Trên macOS, `Ctrl+Shift` thường tương ứng với `⌘⇧`, và bảng [phím tắt](#phím-tắt) ghi đủ cả hai hệ.

## Cài đặt và cập nhật

**Cài đặt.**

- Trên Windows 10 1809+ và Windows 11, bạn chạy bộ cài `Kelpie_x.y.z_x64-setup.exe`. Bộ cài chỉ cài cho người dùng hiện tại nên không cần quyền quản trị.
- Trên macOS 12+, bạn mở tệp `.dmg` rồi kéo Kelpie vào thư mục Applications.
- Trên Linux x86-64 (glibc 2.35 trở lên), bạn dùng AppImage (`chmod +x` rồi chạy, cần FUSE 2) hoặc cài gói `.deb` bằng `sudo apt install ./Kelpie_x.y.z_amd64.deb`. Để lưu bí mật trên Linux, Kelpie cần một Secret Service như GNOME Keyring hoặc KWallet.

Kelpie không kèm sẵn agent nào, vì vậy bạn cài các CLI mình dùng (`claude`, `codex`, `omp`…) theo cách thông thường. Trên Windows, Kelpie tìm các CLI này qua `PATH`, còn trên macOS và Linux, ứng dụng tìm chúng qua login shell.

**Cập nhật.** Kelpie kiểm tra bản mới trên GitHub 30 giây sau khi khởi động, sau đó cứ 6 giờ lại kiểm tra một lần. Khi có bản mới, thanh trạng thái hiện *Update x.y.z*, và hộp thoại cập nhật hiển thị ghi chú phát hành (release notes) cùng ba lựa chọn: *Install now*, *Install when idle* (cài khi không còn phiên nào đang làm việc) và *Skip this version*. Mọi bản cập nhật đều được ký số.

**Cập nhật không làm gián đoạn agent.** Khi bạn cài bản mới, chỉ ứng dụng khởi động lại, còn mọi phiên vẫn chạy trong daemon và hiện lại đúng chỗ cũ khi Kelpie mở ra. Đây là lợi ích chính của việc tách daemon khỏi cửa sổ, vì bạn không phải chờ agent xong việc mới dám cập nhật. Ngoại lệ duy nhất là lần cập nhật đầu tiên từ một bản 0.1 đời đầu, khi daemon vẫn chạy từ thư mục cài đặt. Khi đó, các phiên dừng một lần, hộp thoại báo trước, và agent tiếp tục làm việc sau đó.

**Đóng Kelpie.** Theo mặc định, khi bạn đóng Kelpie thì daemon cũng dừng, kéo theo mọi phiên, automation và việc truyền tệp. Nếu vẫn còn việc đang chạy, Kelpie liệt kê các việc đó và hỏi bạn chọn *Close and stop* hay *Cancel*. Ở lần mở sau, các phiên hiện lại với trạng thái *interrupted* và agent tiếp tục đúng cuộc hội thoại cũ. Nếu bạn muốn automation, run theo lịch, truy cập từ điện thoại hoặc việc sao chép tệp vẫn tiếp tục khi cửa sổ đã đóng, hãy đặt Settings → Session Daemon → *When Kelpie closes* thành *Keep it running in the background*.

Chi tiết nằm ở các phần [Install](user-guide.md#install) và [Resuming after a restart](user-guide.md#resuming-after-a-restart).

## Bắt đầu nhanh

1. Để thêm một thư mục, bạn nhấn `+` ở thanh bên (sidebar) hoặc mở ô tìm kiếm và chọn *Add Project Folder…*. Nếu thư mục đó là một Git repository, Kelpie tự thêm các worktree khác của nó (những thư mục làm việc khác của cùng repository) vào dưới project.
2. Trên màn hình trống của project, bạn nhấn *Claude Code*, *Codex*, *OMP* hoặc *Terminal* để mở phiên. Muốn chỉnh thêm, bạn dùng nút `+` trên thanh tab: nút này mở hộp thoại phiên mới, nơi bạn đặt tham số, tên, vị trí của khung (pane, tức một ô trong tab), tài khoản, *Chat view* và yêu cầu đầu tiên (prompt) cho agent.
3. Khi cần thêm một khung, bạn nhấn `Ctrl+Shift+D` để chia sang phải rồi chọn thứ cần chạy trong khung mới.
4. Sau khi giao việc cho agent, bạn theo dõi chấm trạng thái trên thanh bên. Chấm hổ phách nghĩa là phiên cần bạn, chấm xanh dương nghĩa là phiên đang làm việc, còn chấm xanh lá nghĩa là phiên đã xong nhưng bạn chưa xem.

Khi chưa có project nào, Kelpie hiện màn hình chào với Kip, linh vật của ứng dụng, cùng các nút *Add project folder*, *Add remote project (SSH)*, *SSH hosts* và *Command palette*. Trong vài lần mở đầu tiên, mỗi lần Kelpie hiện một mẹo ngắn về tìm kiếm, bảng điều khiển, terminal hoặc chat, hay các hội thoại cũ. Bạn có thể tắt hoặc bật lại mẹo trong Settings → Notifications.

## Không gian làm việc

**Thanh bên.** Thanh bên liệt kê mọi project kèm nhánh Git và các phiên của từng project, theo đúng thứ tự khung trên màn hình. Ở nhánh Git, dấu `*` cho biết có thay đổi chưa commit, còn ↑↓ là số commit đi trước hoặc đi sau. Mỗi phiên hiện chấm trạng thái, tiêu đề việc, model và thời gian từ lần hoạt động cuối. Khi nhấp chuột phải vào một phiên, bạn có thể đổi tên, đánh dấu chưa đọc, khởi động lại, tiếp tục (resume), chuyển sang chat view hoặc dừng phiên; với project, menu chuột phải cho phép tạo worktree, đổi tên hoặc sắp xếp. Ngoài ra, `Alt+1…9` chuyển nhanh giữa các project, còn `Alt+Shift+←/→` đưa bạn lùi hoặc tới qua các khung vừa xem.

**Tab và khung.** Mỗi tab tương ứng với một việc, còn các khung trong tab là những thứ bạn cần nhìn cùng lúc khi làm việc đó. Khi bạn rê chuột lên một khung, các nút chia phải, chia xuống, phóng to (`Ctrl+Shift+Enter`) và đóng sẽ hiện ra. Mỗi khung mới đều hỏi bạn muốn chạy gì, và bạn nhấp một agent hoặc *Terminal*, hoặc nhấn `1`–`9` để chọn.

**Bố cục.** Nút lưới trên thanh tab (`Ctrl+Shift+L`) sắp xếp tab theo các mẫu từ 2 cột tới 3×3, hoặc theo số khung trên mỗi hàng như `3,2`. Lệnh *Tile all* gom các tab chỉ chứa một CLI vào một lưới đều, còn *Spread into tabs* làm điều ngược lại. Bạn kéo thanh tiêu đề của khung để đổi chỗ, kéo khung sang tab khác để chuyển tab, và kéo đường chia để đổi kích thước. Khi bật *Broadcast input*, những gì bạn gõ được gửi tới mọi terminal trong tab. Còn *Save this layout* lưu cách sắp xếp kèm thứ đang chạy trong từng khung.

**Terminal nổi.** Phím `` Ctrl+Shift+` `` mở một shell ở thư mục home trong một terminal nổi phía trên mọi project. Khi bạn ẩn terminal này, shell vẫn tiếp tục chạy.

**Liên kết tệp.** Khi agent in ra một đường dẫn như `src/a.ts:42`, `D:\w\x.py` hay chỉ một tên tệp như `name.py`, bạn có thể `Ctrl+click` để mở tệp trong trình xem của Kelpie ở đúng dòng đó. Kelpie tìm tệp lần lượt trong thư mục của phiên, trong project, rồi theo tên tệp. Menu chuột phải trên đường dẫn có *Open*, *Copy Relative*, *Copy Path*, *Copy Name* và *Reveal in Explorer*. Trong chat view, bạn chỉ cần nhấp tên tệp trong câu trả lời để mở tệp.

**Hội thoại dài trong terminal.** Khi bạn cuộn lên trong một cuộc hội thoại dài, một nhãn ở góc màn hình đếm số dòng mới và đưa bạn về cuối. Phím `Ctrl+Shift+↑/↓` nhảy tới yêu cầu trước hoặc sau mà bạn đã gửi. Menu chuột phải của terminal cho phép sao chép, dán, xoá màn hình, chép bối cảnh bằng *Copy Context* (400 dòng cuối), chia khung, chạy quick command hoặc chuyển giao cho agent khác.

Chi tiết nằm ở các phần [Everyday use](user-guide.md#everyday-use), [Layouts](user-guide.md#layouts) và [Terminals](user-guide.md#terminals).

## Tìm kiếm, dashboard và Activity

**Tìm mọi thứ (`Ctrl+Shift+P`).** Một ô tìm kiếm duy nhất tìm các mục gần đây, phiên đang cần bạn, phiên, project, tệp, lệnh và cài đặt. Để thu hẹp phạm vi, bạn gõ tiền tố `@` cho phiên, `#` cho tệp, `>` cho lệnh, `/` cho cài đặt và `!` cho các yêu cầu đã gửi trước đây, hoặc nhấn `Tab` để đổi phạm vi.

**Bảng điều khiển (`Ctrl+Shift+M`).** Bảng điều khiển (Dashboard) nhóm mọi phiên trong mọi project thành *Needs you*, *Failed*, *Working*, *Done* và *Idle*, trong đó nhóm *Needs you* xếp phiên chờ lâu nhất lên đầu. Nút *Answer* mở phần cuối màn hình của phiên cùng các nút cho từng phím mà lời nhắc chấp nhận (1 2 3, y n), Enter, Esc và một dòng để gõ văn bản. Kelpie chỉ gửi câu trả lời nếu phiên vẫn đang chờ đúng lời nhắc đó. Ngoài ra, `Ctrl+Shift+.` đưa bạn tới phiên chờ lâu nhất, rồi tới phiên tiếp theo.

Với các agent không có hook để báo trạng thái (Codex, Gemini, Aider, OpenCode…), Kelpie coi phiên là *needs you* khi màn hình hiện một menu hoặc một câu hỏi có/không.

**Activity.** Chuông trên thanh trạng thái liệt kê những gì đã xảy ra trong lúc bạn nhìn chỗ khác, chẳng hạn agent đã xong hoặc cần bạn, automation đã chạy, hay có worktree mới. Bạn nhấp một mục để tới đó, hoặc nhấp chuột phải để đổi trạng thái đã đọc. Riêng phiên mà bạn đang nhìn thì không tạo thông báo.

Chi tiết nằm ở các phần [Everyday use](user-guide.md#everyday-use) và [Activity and notifications](user-guide.md#activity-and-notifications).

## Agent và chat view

**Các agent được hỗ trợ.** Ngoài Claude Code, Codex và OMP, Kelpie nhận diện 20 CLI khác: Gemini, Cursor CLI, OpenCode, GitHub Copilot, Grok, Qwen Code, Droid, Aider, Amp, Goose, Kimi, Crush, Cline, Kiro, Continue, Devin, Auggie, CodeBuddy, Mistral Vibe và Pi. Trong Settings → Agents, bạn có thể ẩn agent, sửa tham số và biến môi trường của từng agent. Kelpie lưu biến môi trường dưới dạng văn bản thường.

**Quyền.** Ở chế độ *Permissions: Manual*, agent hỏi bạn trước khi hành động. Còn ở chế độ *Permissions: Yolo*, Kelpie khởi động agent với cờ bỏ qua bước xin quyền của chính agent đó.

**Chat view.** Để tiếp tục *cùng cuộc hội thoại* trong giao diện chat, bạn nhấp biểu tượng chat trên thanh tiêu đề của khung agent; biểu tượng terminal sẽ đưa bạn trở lại. Vì Kelpie dừng giao diện này trước khi mở giao diện kia, nên một cuộc hội thoại không bao giờ có hai nơi cùng ghi vào. Trong ô nhập, `/` liệt kê lệnh của agent, `@` liệt kê tệp, còn với Codex thì `$` liệt kê skill. Các nút nhỏ dưới ô nhập đổi model, mức effort và chế độ quyền. Bản ghi hội thoại hiện mỗi lần dùng công cụ trên một dòng, gộp các lần đọc và tìm kiếm liên tiếp, đồng thời liệt kê các tệp đã đổi sau mỗi lượt. Ngoài ra, bạn có thể dán ảnh, lưu câu trả lời vào tab *Saved* và mở *Agent map* để xem các subagent của cuộc hội thoại.

**Phê duyệt trong chat.** Thẻ phê duyệt hiện lệnh hoặc phần sửa mà agent muốn thực hiện. Bạn nhấn `1`–`9` để chọn một phương án, hoặc gõ điều agent nên làm thay thế rồi nhấn *Deny*.

**Hội thoại cũ.** Tab *Sessions* trong panel bên phải liệt kê các cuộc hội thoại Claude, Codex và OMP trước đây, nhóm theo ngày, và bạn có thể tìm chúng theo tiêu đề hoặc yêu cầu đầu tiên. Từ đây, bạn mở lại (resume) một cuộc hội thoại trong terminal hoặc chat view.

**Duy trì cuộc hội thoại.** Menu của phiên có *Hand off to…* để mở agent khác với phần cuối bản ghi hội thoại làm bối cảnh. Menu này còn có *Compact*, *Clear* và *Restart*, và cả ba đều giữ nguyên cuộc hội thoại. Ngoài ra, khi bật *Hibernation*, Kelpie dừng những agent đã xong và nằm yên trong một project chạy nền quá N phút, rồi cho agent tiếp tục khi bạn mở lại.

**Nhiều tài khoản.** Bạn thêm tài khoản Claude Code hoặc Codex trong Settings → Accounts. Khi một phiên Claude Code chạm giới hạn sử dụng, dải thông báo phía trên khung cho bạn ba lựa chọn: *Continue at reset*, *Switch account…* hoặc *Hand off to…*.

**Context và chi phí.** Mỗi phiên Claude Code, Codex và OMP có một vòng tròn context cho biết cửa sổ ngữ cảnh đã dùng bao nhiêu. Trang *Usage & cost* thống kê chi phí và token theo ngày, project, agent hoặc model, còn Settings → Usage & Resources cho phép đặt giới hạn chi phí cho mỗi phiên.

Chi tiết nằm ở các phần [Agents](user-guide.md#agents), [Everyday use](user-guide.md#everyday-use) (mục Chat view), [Past conversations](user-guide.md#past-conversations), [Several accounts](user-guide.md#several-claude-code--codex-accounts) và [Context, tokens and cost](user-guide.md#context-tokens-and-cost).

## Sub-session

Một agent có thể mở các phiên agent khác để chia việc. Khác với subagent chạy bên trong tiến trình của agent, mỗi **sub-session** là một phiên Kelpie thật, nên phiên đó hiện trên thanh bên và bạn có thể mở, gõ vào hoặc đóng nó. Sub-session gửi kết quả về phiên cha.

**Cách dùng.** Trong một phiên Claude, Codex hoặc agent khác do Kelpie khởi động, bạn chỉ cần yêu cầu bằng lời thường, ví dụ *"mở sub-session để sửa #1 #2 #3 bằng Codex"*. Agent sẽ mở một phiên con cho mỗi việc. Khi phiên con báo xong, bị chặn hoặc cần bạn, kết quả hiện thành một tin nhắn mới trong phiên cha.

**Phê duyệt.** Theo mặc định, mỗi sub-session đều hỏi bạn trước bằng một thẻ ở góc dưới bên phải. Thẻ ghi tên phiên yêu cầu, agent, thư mục, việc và model, để bạn chọn *Allow once*, *Allow for this session*, *Always allow* hoặc *Deny*. Nếu có yêu cầu đang mở, `Ctrl+Shift+A` đưa yêu cầu đó lên trước.

**Giới hạn.** Sub-session được lồng sâu tối đa 3 cấp, mỗi phiên có tối đa 10 phiên con đang chạy, và toàn máy còn có một giới hạn chung do Kelpie đặt tự động hoặc do bạn chọn. Một sub-session không bao giờ có nhiều quyền hơn phiên đã mở nó. Trong Settings → Agents → *Sub-sessions*, bạn chọn một trong ba chế độ Off, Ask (mặc định) và Allow.

**Công cụ mở trang web.** Cùng với các công cụ sub-session, agent còn có công cụ `kelpie_open_page` để mở một trang web cho bạn xem trong tab nền của project. Cách công cụ này hoạt động được mô tả ở mục *Trang web do agent mở* trong phần [Tệp, trình duyệt và tiến trình](#tệp-trình-duyệt-và-tiến-trình).

Chi tiết nằm ở phần [Sub-sessions](user-guide.md#sub-sessions).

## Worktree và board

**Tạo worktree.** Bạn nhấp chuột phải vào một repository và chọn *New worktree…*. Hộp thoại gợi ý sẵn tên nhánh, nhánh gốc và thư mục (`<repo>-worktrees/<branch>`). Worktree mới hiện dưới repository với phiên và bố cục riêng. Khi bạn chọn *Remove worktree…*, Kelpie liệt kê trước những gì sẽ mất rồi mới dừng phiên và xoá worktree, nhưng luôn giữ lại nhánh.

**Tệp `.kelpie.yaml`.** Một repository có thể chứa tệp này để chuẩn bị cho worktree mới. Kelpie hiển thị nội dung tệp theo commit gốc và chỉ chạy các bước trong đó khi bạn đánh dấu đồng ý. Tệp có dạng như sau:

```yaml
setup:            # lệnh gõ vào một terminal "setup"
  - pnpm install
tabs:             # phiên cần mở: claude, codex, omp, shell hoặc một lệnh
  - claude
  - command: pnpm dev
copy:             # bản sao riêng của tệp bị Git bỏ qua, ví dụ .env
  - .env
link:             # thư mục dùng chung với repository
  - node_modules
check:            # check mà Finish chạy, xem phần Review
  - pnpm test
```

**Agent mới trong worktree riêng.** Khi bạn bật Settings → Agents → *Worktree for new agent sessions*, mỗi agent mới sẽ chạy trong worktree riêng, với tên nhánh được tạo từ yêu cầu đầu tiên.

**Board (`Ctrl+Shift+J`).** Board là bảng công việc hiện mọi project và worktree thành thẻ trong các cột *Todo*, *In progress*, *Review* và *Done*, kèm các agent đang chạy. Bạn kéo thẻ giữa các cột, ghim thẻ quan trọng và nhấp thẻ để mở. Mỗi worktree có thể có một ghi chú về việc đang làm. Ghi chú này do bạn đặt từ menu thanh bên, hoặc do agent tự đặt bằng `kelpied worktree set --comment "…" --status review`.

Chi tiết nằm ở các phần [Worktrees and the workspace board](user-guide.md#worktrees-and-the-workspace-board) và [New agent sessions in a worktree](user-guide.md#new-agent-sessions-in-a-worktree).

## Review, Git và Finish

**Panel Changes (`Ctrl+Shift+G`).** Panel Changes cho phép push, pull (chỉ fast-forward) và sync, hiện xung đột khi đang merge hoặc rebase, và có nút *Resolve with AI*. Nút ✦ soạn thông điệp commit dựa trên diff. Khi muốn đổi nhánh, bạn nhấp tên nhánh ở đầu khung, và Kelpie chỉ đổi khi không có gì bị mất. Phần *History* hiện 100 commit gần nhất, còn phần *Stashes* cho phép áp dụng hoặc xoá stash. Với GitHub, khung này hiện pull request của nhánh cùng trạng thái các check (bước kiểm tra tự động) và có nút *Create pull request…*; với GitLab, khung hiện merge request và pipeline.

**Ghi chú khi duyệt.** Trong diff, bạn nhấp dấu `+` ở lề để ghi chú cho một dòng. Ghi chú của mọi tệp được gom vào một thanh phía trên diff, và nút *Send to agent ▾* gửi tất cả thành một yêu cầu cho agent đang chạy hoặc cho một agent mới. Để chuyển giữa các thay đổi, bạn dùng `F7` và `Shift+F7`.

**Nhờ agent khác duyệt.** Lệnh *Review with…* nhờ một agent khác duyệt worktree, và theo mặc định Kelpie chọn agent không viết ra thay đổi đó. Agent duyệt có thể để lại ghi chú trên diff bằng `kelpied review add <file>:<line> "<comment>"`.

**Hàng đợi duyệt và Finish.** Cột *Review* trên board đóng vai trò hàng đợi duyệt. Mỗi thẻ worktree trong cột này hiện số tệp, số dòng thêm và bớt, số commit, check, chi phí và số lượt. Khi bạn nhấn *Approve → Finish*, hộp thoại Finish mở ra với danh sách tệp chưa commit (sẽ được lưu thành commit "WIP"), cách gộp (squash, merge commit hoặc rebase), thông điệp và các check. Sau đó, Kelpie dừng các phiên của worktree, commit, gộp, chạy check, chỉ di chuyển nhánh đích nếu nhánh đó không đổi trong lúc chờ, rồi xoá worktree sau cùng. Finish không bao giờ dùng thao tác force. Kelpie đọc danh sách check từ `.kelpie.yaml` trên nhánh đích chứ không bao giờ từ nhánh đang được gộp, và chỉ chạy chúng sau khi bạn duyệt đúng danh sách đó.

**Lượt và hoàn tác.** Khi bạn bật Settings → Agents → *Turn snapshots*, Kelpie chụp lại trạng thái thư mục sau mỗi lượt của agent. Tab *Turns* liệt kê các lượt, và nút hoàn tác cho bạn xem trước mọi tệp sẽ được khôi phục trước khi thay đổi bất cứ điều gì.

Chi tiết nằm ở các phần [Source control and review](user-guide.md#source-control-and-review), [Review and Finish](user-guide.md#review-and-finish), [Review with another agent](user-guide.md#review-with-another-agent-and-best-of-several) và [Turns and undo](user-guide.md#turns-and-undo).

## Kế hoạch, run và automation

**Tab Plan.** Tab Plan đọc kế hoạch và spec có sẵn trong repository, gồm `plans/<tên>/plan.md` cùng các tệp `phase-*.md`, `specs/**`, `.kiro/specs/**` và `docs/plans/**`. Kelpie không áp đặt định dạng riêng mà chỉ đọc checklist Markdown `- [ ]` và `- [x]`, nên khi bạn tích một ô, tệp chỉ thay đổi đúng một ký tự. Nút tia sét ở cuối mỗi việc (*Run with…*) giao việc đó cho một agent chạy ngay tại thư mục, trong worktree riêng hoặc trên một máy chủ SSH đã liên kết. Còn nút *New spec* tạo `requirements.md`, `design.md` và `tasks.md` cho một spec mới.

**Run (`Ctrl+Shift+R`).** Một run chia việc lớn thành nhiều nhiệm vụ. Mỗi nhiệm vụ có yêu cầu, agent, các nhiệm vụ phải chờ, tuỳ chọn cần phê duyệt trước khi chạy và tuỳ chọn chạy trong worktree riêng. Từ một mục tiêu, *Plan tasks with AI* soạn sẵn danh sách nhiệm vụ để bạn xem lại. Daemon chạy các nhiệm vụ theo thứ tự phụ thuộc. Bạn cũng có thể tạo run từ một tệp JSON bằng `kelpied run create run.json`.

**Thử nhiều agent.** Lệnh *Try with several agents…* gửi một yêu cầu tới 2–5 agent, mỗi agent làm trong worktree riêng. Tab *Compare* của run đặt các kết quả cạnh nhau, gồm trạng thái, thời gian, chi phí, số tệp, số dòng thay đổi và check. Khi bạn chọn *Use this one*, Kelpie mở Finish cho kết quả đó và dọn các kết quả còn lại.

**Automation.** Trong Settings → Automations, bạn cho một agent chạy một yêu cầu theo lịch (mỗi giờ, mỗi ngày, ngày làm việc hoặc mỗi tuần) hoặc khi có sự kiện. Các sự kiện gồm tệp thay đổi, agent xong một lượt, có commit mới, check của pull request thất bại, hoặc một worktree được Finish. Automation cũng có thể chạy một lệnh, nhưng lệnh đó phải được bạn duyệt và chạy trực tiếp, không qua shell. Chế độ *Dry run* cho biết automation sẽ làm gì mà không chạy thật.

**Quick command.** Trong Settings → Quick Commands, bạn lưu các lệnh và yêu cầu hay dùng để chạy lại từ ô tìm kiếm (`>`), từ menu phiên mới hoặc bằng `Ctrl+1…9`.

**Issue tracker.** Settings → Integrations kết nối Kelpie với Linear, Jira và GitLab. Khi đó, panel *Issues* liệt kê các issue đang mở, và bạn có thể giao issue cho agent, tạo worktree cho issue hoặc đổi trạng thái của issue.

Chi tiết nằm ở các phần [Specs and tasks](user-guide.md#specs-and-tasks-the-plan-tab), [Orchestration runs](user-guide.md#orchestration-runs), [Automations](user-guide.md#automations), [Quick commands](user-guide.md#quick-commands) và [Issue trackers](user-guide.md#issue-trackers-and-gitlab).

## Tệp, trình duyệt và tiến trình

**Tệp.** Khung bên phải có các tab Files, Changes và Sessions, trong đó tab Files cho phép lọc theo tên hoặc tìm theo nội dung. Tệp bạn nhấp sẽ mở trong trình soạn thảo, nơi `Ctrl+S` lưu thay đổi và Kelpie phát hiện khi tệp bị sửa từ bên ngoài. Tệp CSV và TSV mở thành bảng có thể sắp xếp và lọc, còn PDF, notebook Jupyter, sơ đồ Mermaid và Markdown đều được hiển thị trực tiếp. Phím `Alt+Z` bật hoặc tắt ngắt dòng. Menu của project mở thư mục trong VS Code, Cursor, Windsurf, Zed, Sublime Text hoặc IntelliJ IDEA nếu máy có cài.

**Composer (`Ctrl+Shift+I`).** Composer là ô soạn nhiều dòng nằm trên phiên đang chọn. Trong ô này, bạn gõ `@` để nhắc tới tệp, thư mục, `@selection`, `@diff`, `@note`, `@terminal:<phiên>` hoặc `@issue:<mã>`. Trước khi gửi, Kelpie che các bí mật có trong nội dung.

**Trình duyệt (`Ctrl+Shift+U`).** Khung trình duyệt có thanh địa chỉ, công cụ nhà phát triển và chức năng chụp màn hình trang. Ở chế độ *Design mode*, bạn nhấp các phần tử trên trang, thêm ghi chú rồi gửi tất cả cho agent trong một yêu cầu.

**Tiến trình.** Nếu bạn bật Settings → Terminal → *Mở trang web của agent trong một tab* (khi mới cài thì mục này tắt), mỗi khi một agent trên màn hình khởi động một trang web (dev server, bảng điều khiển…), Kelpie mở trang đó trong một tab nền và báo ngắn cho bạn. Tab *Processes* trong panel bên phải liệt kê những gì mỗi phiên đang chạy dưới dạng cây, kèm PID, bộ nhớ, CPU và cổng. Bạn nhấp một cổng để mở trang tương ứng trong tab trình duyệt. Nút *Kill* dừng một chương trình cùng những gì nó đã khởi động, còn *Kill children* dừng những gì một phiên đã khởi động nhưng vẫn giữ phiên đó. Cả hai nút đều hỏi trước và nói rõ cổng nào sẽ được giải phóng.

**Trang web do agent mở.** Dù cài đặt mở tab tự động đang bật hay tắt, agent vẫn có thể tự mở một trang web khi bạn yêu cầu, ví dụ *"mở trang đó cho tôi xem"*, hoặc khi agent vừa khởi động một server mà bạn cần xem. Để làm việc này, agent dùng công cụ `kelpie_open_page` (hoặc lệnh `"$KELPIE_CLI" sub open <url>`), và Kelpie mở trang trong một tab nền của project chứa phiên đó, kèm một thông báo ngắn có nút *View*. Kelpie chỉ mở địa chỉ `http://` hoặc `https://` không chứa tên đăng nhập hay mật khẩu. Nếu project đã có một tab hiện đúng địa chỉ đó, Kelpie không mở thêm tab mà chỉ báo cho bạn biết; một trang khác của cùng server vẫn được mở trong tab riêng, và tab tự động mà agent yêu cầu theo cách này sẽ được giữ lại khi server dừng. Khi cửa sổ không mở được trang được gửi tới, chẳng hạn vì project đã bị xóa, thông báo sẽ cho bạn biết agent đã yêu cầu gì và vì sao trang không được mở. Mỗi phiên chỉ được mở tối đa 10 trang trong một phút. Công cụ này chỉ có ở các phiên chạy trên máy này, còn agent trong phiên SSH thì không có, vì vậy `localhost` trong địa chỉ luôn là máy của bạn. Khi cửa sổ Kelpie đang đóng, agent nhận được thông báo lỗi và sẽ đưa địa chỉ trang cho bạn.

Chi tiết nằm ở các phần [Files](user-guide.md#files), [Composer](user-guide.md#composer), [Browser panes](user-guide.md#browser-panes-and-design-mode) và [Processes](user-guide.md#processes-ports-and-preview).

## Làm việc qua SSH

**Project trên máy chủ.** Lệnh *Add Remote Project (SSH)…* cho bạn chọn một máy chủ và một thư mục trên máy chủ đó. Terminal và agent chạy trên máy chủ qua `ssh -t`, bên trong `tmux` nếu máy chủ có cài, nhờ đó phiên vẫn sống khi kết nối bị rớt. Cây tệp, trình soạn thảo, tìm kiếm và Git cũng hoạt động qua SSH. Kelpie dùng OpenSSH và cấu hình SSH sẵn có của bạn.

**Trang SSH hosts (`Ctrl+Alt+H`).** Trang này quản lý máy chủ theo kiểu Termius, với các tính năng sau:

- Mỗi máy chủ có tên, nhóm, thẻ, màu, ghi chú, địa chỉ, người dùng và cổng. Bạn đăng nhập bằng khoá, SSH agent hoặc mật khẩu được lưu trong kho thông tin xác thực của hệ điều hành.
- Từng máy chủ có thể dùng màu và theme terminal riêng, nhờ đó môi trường production không bao giờ trông giống máy tính của bạn.
- Một cú nhấp là bạn mở được *Terminal*, *Claude Code here* hoặc agent khác trên máy chủ.
- Tab *Files* cho phép duyệt, tải lên, tải xuống, đổi tên và xoá tệp, kèm một hàng đợi truyền tệp.
- Ngoài ra, bạn có thể chạy một lệnh trên nhiều máy chủ cùng lúc, quản lý khoá trong tab *Keychain*, xem *Known hosts* và tạo quy tắc *Port forwarding*.

**Khung SSH trong mọi project.** Một project trên máy tính của bạn có thể chứa terminal và agent chạy trên máy chủ, đặt cạnh các khung cục bộ. Khung mới và hộp thoại *New session…* có dòng *Run on* để chọn *This computer*, một máy chủ đã liên kết với project, hoặc *Other host…*. Mỗi khung chạy trên máy khác có một sọc mang màu của máy chủ đó.

**Giữ phiên chạy trên máy chủ.** Bạn chỉ cần cài `tmux` trên máy chủ, ví dụ bằng `sudo apt-get install -y tmux` trên Ubuntu. Khi đó, phiên vẫn chạy dù mạng bị rớt, dù bạn đóng Kelpie hay máy tính của bạn chuyển sang chế độ ngủ.

Chi tiết nằm ở các phần [Remote projects](user-guide.md#remote-projects-ssh), [SSH hosts](user-guide.md#ssh-hosts), [SSH panes in any project](user-guide.md#ssh-panes-in-any-project) và [Keeping sessions running on a server](user-guide.md#keeping-sessions-running-on-a-server).

## Điện thoại

Truy cập từ điện thoại mặc định bị tắt, và bạn bật nó trong Settings → Mobile → *Mobile access*. Điện thoại có thể kết nối theo ba cách. Cách mặc định của thiết lập mới là dùng ứng dụng Kelpie tải từ máy chủ chuyển tiếp (relay) của Kelpie, không cần Tailscale. Ngoài ra, điện thoại có thể dùng ứng dụng do chính máy tính phục vụ qua Tailscale, hoặc mở trình duyệt trên cùng mạng.

**Các cách kết nối điện thoại.** Mọi lần ghép đôi đều bắt đầu từ nút *Pair a phone* trong Settings → Mobile. Nút này liệt kê ba cách kết nối, và mỗi cách cho biết nó cần gì, cho phép điện thoại làm gì và bạn phải tin cậy ai. Khi một cách chưa dùng được, Kelpie nói rõ lý do và có nút đưa bạn tới mục cần bật. Kelpie đánh dấu *Recommended* cho Tailscale khi điện thoại dùng được nó; nếu không, Kelpie đánh dấu ứng dụng trên relay khi relay đang phục vụ ứng dụng, còn lại thì đánh dấu mạng này.

| Cách | Cần | Cho phép | Tin cậy |
| --- | --- | --- | --- |
| *Tailscale* | Tailscale trên máy tính và trên điện thoại, cùng với *Reach this computer with Tailscale* đang bật | Đọc, gõ, duyệt yêu cầu và khởi động agent từ bất cứ đâu; xem màn hình và các khung trình duyệt khi bạn cấp quyền điều khiển và khoá ứng dụng của điện thoại đang bật | Máy tính này, nơi cung cấp ứng dụng, và Tailscale |
| *Kelpie's relay* (hoặc *Your relay at …* với relay khác) | Relay đang bật và đang phục vụ ứng dụng điện thoại | Đọc, gõ, duyệt yêu cầu và khởi động agent từ bất cứ đâu; xem màn hình chỉ khi bạn cho phép qua relay, và không bao giờ dùng các khung trình duyệt | Người triển khai ứng dụng trên relay |
| *This network* | Cùng mạng Wi-Fi với máy tính, cùng với *Pair browsers on this network* đang bật | Đọc, gõ, duyệt yêu cầu và khởi động agent qua HTTPS bằng chứng chỉ của bạn, còn qua HTTP thường thì chỉ đọc; màn hình và các khung trình duyệt chỉ có qua HTTPS, khi bạn cấp quyền điều khiển và khoá ứng dụng đang bật | Máy tính này và chứng chỉ của bạn; qua HTTP thường, bạn còn phải tin rằng không ai trong mạng tráo trang |

Mỗi cách ghép đôi điện thoại thành một thiết bị riêng, với khoá, trang và quyền riêng. Vì trình duyệt giữ việc ghép đôi theo từng địa chỉ nên ứng dụng trên relay và ứng dụng qua Tailscale trên cùng một điện thoại là hai ứng dụng đã cài khác nhau. Khi bạn duyệt một điện thoại, hộp thoại của Kelpie hỏi *Same phone as* và gợi ý điện thoại đã ghép đôi theo cách khác với cùng tên, hoặc bạn chọn *A new device*. Sau đó Settings liệt kê các bản ghi này như một điện thoại, trong đó mỗi cách có một dòng với quyền riêng và nút *Unpair* riêng. Để chuyển điện thoại sang cách khác, bạn ghép đôi theo cách mới và đánh dấu *Replace that pairing*. Khi đó Kelpie hỏi bạn xác nhận và chỉ gỡ bản ghi cũ sau khi bản ghi mới đã được duyệt; nếu việc gỡ thất bại thì cả hai vẫn còn và Kelpie báo cho bạn biết. Việc gộp các bản ghi thành một điện thoại chỉ được lưu trong ứng dụng Kelpie trên máy tính này. Trước khi được ghép đôi, và trong mục *Connection* ở phần cài đặt, ứng dụng điện thoại cho biết nó kết nối theo cách nào và liệt kê các cách còn lại.

Danh sách thiết bị cũng cho biết lúc này mỗi điện thoại đang kết nối theo đường nào, chẳng hạn *connected now · through Tailscale* (hoặc qua relay, qua mạng này, qua chính máy tính này), ngay cạnh cách nó đã ghép đôi, còn khi điện thoại ngắt kết nối thì dòng đó chuyển thành *last seen …*. Dòng này tự đổi khi điện thoại kết nối, ngắt kết nối hoặc chuyển sang đường khác, nên bạn không cần tải lại trang. Trên điện thoại, mục *Computer screen* trong Settings → *Connection* nêu đúng lý do mà máy tính đưa ra khi điện thoại không xem được màn hình, chẳng hạn điện thoại đang dùng ứng dụng trên relay, điện thoại chưa được cấp quyền điều khiển, điện thoại được ghép đôi qua HTTP thường, điện thoại đang kết nối qua relay, hoặc khoá ứng dụng chưa sẵn sàng. Khi điện thoại dùng ứng dụng trên relay mà máy tính này cũng có Tailscale, mục đó còn hướng dẫn cách ghép đôi ứng dụng qua Tailscale để xem được màn hình.

**Ghép đôi bằng ứng dụng Kelpie.**

1. Đầu tiên, bạn bật *Mobile access*; với thiết lập mới, Kelpie sẽ bật relay và hỏi bạn xác nhận một lần.
2. Với thiết lập mới, *Pair a phone* tự mở sau khi bạn bật công tắc và đặt relay lên đầu; nếu không, bạn tự nhấn *Pair a phone*. Bạn nhấn *Pair this way* dưới *Kelpie's relay*, rồi ở bước **Install** quét mã bằng camera điện thoại và cài Kelpie lên màn hình chính (Share → Add to Home Screen trên iPhone, nút Install trên Android).
3. Ở bước **Pair**, bạn mở ứng dụng trên điện thoại, nhấn *Scan the code*, rồi nhập vào Kelpie sáu chữ số mà điện thoại hiện ra. Mã ghép đôi chỉ dùng được một lần và hết hạn sau hai phút.
4. Tiếp theo, bạn chọn *Allow reading* hoặc *Allow reading and typing* rồi xác nhận bằng Windows Hello, mật khẩu hoặc Touch ID.
5. Cuối cùng, điện thoại yêu cầu bạn thiết lập khoá ứng dụng bằng Face ID, vân tay hoặc mã máy, và bạn nên làm việc này ngay.

Khi bạn rời ứng dụng, Kelpie che nội dung ngay lập tức. Nếu bạn quay lại trong thời gian đã chọn ở phần cài đặt trên điện thoại (mặc định là 5 phút), ứng dụng không hỏi lại Face ID. Ngược lại, khi bạn quay lại sau thời gian đó, sau khi tải lại trang hay cập nhật ứng dụng, hoặc khi máy tính đã khoá điện thoại vì hết thời gian chờ, ứng dụng sẽ hỏi lại. Riêng việc xem màn hình máy tính hay điều khiển một khung trình duyệt luôn hỏi lại Face ID sau mỗi lần bạn rời ứng dụng, và việc rời ứng dụng cũng kết thúc chúng ngay. Trên iPhone, ảnh xem trước trong trình chuyển ứng dụng vẫn có thể hiện màn hình cuối cùng.

**Cài đặt trên điện thoại.** Bạn mở phần cài đặt bằng biểu tượng bánh răng ở đầu ứng dụng điện thoại, hoặc bằng nút *Settings* trong bảng kết nối, và phần này cũng nằm sau khoá ứng dụng như mọi nội dung khác. Tại đây bạn chọn thời gian ứng dụng được rời đi trước khi hỏi lại Face ID, khoá ngay khi cần, bật hoặc tắt cảnh báo, đổi giao diện, ngôn ngữ, cỡ chữ và cách ngắt dòng dài trong terminal. Bạn cũng chọn được những phím hiện trên thanh phím cùng thứ tự của chúng, trong đó Ctrl luôn đứng đầu. Vì Ctrl+C dừng việc agent đang làm nên mặc định ứng dụng hỏi lại trước khi gửi phím này, dù bạn bấm trên thanh phím hay gõ Ctrl rồi C trong ô trả lời. Phần kết nối cho biết tên máy tính, cách điện thoại đã ghép đôi, đường kết nối hiện tại (Tailscale, relay, mạng này hoặc chính máy này), thời gian khứ hồi, quyền của điện thoại và lý do khi màn hình máy tính không xem được từ đây. Phần này còn liệt kê các cách khác để kết nối điện thoại và chỉ chỗ chọn chúng trên máy tính. Nút *Clear what this phone keeps* xoá danh sách phiên và các màn hình mà điện thoại đã lưu, còn bản nháp và tin nhắn chưa gửi chỉ bị xoá khi bạn đánh dấu ô tương ứng, và việc ghép đôi vẫn giữ nguyên. Các thiết lập của khoá ứng dụng như thời gian chờ hay việc hỏi lại cho lệnh nguy hiểm chỉ hiện để xem, vì bạn chỉ đổi được chúng trong Settings → Mobile trên máy tính.

**Chọn cảnh báo cho từng điện thoại.** Khi cảnh báo đang bật, phần cài đặt trên điện thoại liệt kê từng loại cảnh báo kèm một ô đánh dấu, và máy tính chỉ gửi đến điện thoại đó những loại bạn giữ lại. Loại nào đã bị tắt trong Settings → Notifications trên máy tính thì hiện mờ trên điện thoại kèm lý do. Mục *Quiet hours* chặn mọi cảnh báo đến điện thoại đó trong một khoảng giờ, ví dụ từ 22:00 đến 07:00, trừ khi bạn đánh dấu *Still alert when an agent needs you*. Ô này chỉ cho qua cảnh báo khi agent cần bạn, nên run chờ phê duyệt vẫn không gửi cảnh báo trong khung giờ yên lặng, và run đó vẫn tiếp tục chờ bạn phê duyệt. Giờ được tính theo đồng hồ của điện thoại, và điện thoại tự báo lại cho máy tính khi múi giờ thay đổi. Mỗi điện thoại giữ lựa chọn riêng của nó, kể cả khi cảnh báo đang tắt, còn cảnh báo thử thì luôn được gửi đi. Nếu máy tính chạy phiên bản Kelpie cũ hơn, máy tính vẫn gửi mọi cảnh báo, còn điện thoại làm mờ các mục này và nhắc bạn cập nhật Kelpie trên máy tính.

**Danh sách phiên trên điện thoại.** Hai nút chuyển phía trên danh sách cho bạn xem các phiên theo trạng thái (*By status*, cách mặc định) hoặc theo dự án (*By workspace*). Khi xem theo dự án, mỗi dự án có một phần riêng mang tên của nó, còn phiên không thuộc dự án nào được xếp theo thư mục mà phiên đang chạy, và phiên không có thư mục nằm trong phần *Other folders*. Phần nào có phiên cần bạn thì đứng đầu, sau đó đến các phần vừa có hoạt động gần nhất, và mỗi dòng ghi trạng thái của phiên bằng một chữ. Phiên đã dừng có chấm màu xám và cho biết nó kết thúc cách đây bao lâu. Trong mục *Inbox* của phần cài đặt, bạn chọn được cách xem này và thứ tự trong mỗi nhóm (*Order inside a group*), theo hoạt động gần nhất hoặc theo tên từ A đến Z. Mục này còn có công tắc *Show stopped sessions*, vốn được bật sẵn. Nếu bạn tắt công tắc này thì danh sách sẽ ẩn các phiên đã dừng và hiện một dòng cho biết số phiên bị ẩn, còn khi bạn nhấn vào dòng đó thì các phiên ấy hiện lại. Dù bạn chọn thứ tự nào, các phiên cần bạn vẫn luôn đứng đầu nhóm và phiên chờ lâu nhất được xếp trước.

**Khởi động phiên từ điện thoại.** Khi điện thoại được phép gõ, đầu danh sách phiên có nút *+* để mở bảng *Start a session*. Tại đó bạn chọn dự án, trong đó các dự án đang có phiên được xếp trước, rồi chọn agent và nhập prompt đầu tiên nếu muốn. Sau khi bạn nhấn *Start session*, phiên mới chạy trên máy tính và mở ngay trên điện thoại. Kelpie khởi động agent đúng như các nút khởi động trên máy tính, tức là dùng tham số trong hồ sơ agent, chế độ quyền hoặc *Yolo* trong Settings → Agents, biến môi trường của hồ sơ và tài khoản đang dùng. **Điều này bao gồm cả chế độ bỏ qua quyền**, nên agent nào trên máy tính hành động mà không hỏi trước thì khi được điện thoại khởi động cũng vậy, và bảng chọn ghi rõ điều đó cho agent ấy. Kelpie trên máy tính hiện thông báo *<điện thoại> started <agent> in <dự án>* kèm nút *Show* và ghi dòng này vào Activity. Khi cửa sổ Kelpie không ở phía trước và bạn đã bật thông báo trong Settings → Notifications, hệ điều hành cũng hiện một thông báo về phiên đó. Ngoài ra, *What phones did* lưu việc này dưới tên *session_new* cùng tên agent, dự án và phiên nhưng không lưu prompt. Việc khởi động vẫn chạy khi cửa sổ Kelpie đã đóng, vì daemon phiên giữ lại những gì ứng dụng báo lần gần nhất, và phiên được khởi động trong lúc cửa sổ đóng sẽ được thêm vào Activity ở lần mở Kelpie kế tiếp mà không bật thông báo. Nếu có thể Kelpie đã bỏ sót vài phiên như vậy, Activity sẽ báo một lần và chỉ bạn tới *What phones did*. Tuy nhiên, sau khi máy tính khởi động lại, bạn cần mở một bản Kelpie mới nhất, và cho tới lúc đó điện thoại sẽ nhắc bạn. Nếu Kelpie trên máy tính không chuyển được cài đặt sang điện thoại, chẳng hạn vì có tên dự án không hợp lệ, Kelpie sẽ báo lý do và điện thoại không khởi động được phiên nào cho tới khi lỗi được khắc phục, nhờ vậy điện thoại không bao giờ khởi động agent bằng cài đặt mà bạn đã đổi. Điện thoại chỉ được chọn dự án trên chính máy tính này và agent đã cài ở đây có giao diện terminal, nên dự án trên máy khác qua SSH và agent chỉ dùng dạng chat không có trong danh sách. Mỗi điện thoại khởi động được ba phiên liền nhau, sau đó mỗi mười giây thêm một phiên, còn khi máy tính quá bận thì Kelpie từ chối cho tới khi máy rảnh hơn. Ứng dụng trên relay cũng khởi động được phiên nếu điện thoại được phép gõ.

**Những việc điện thoại làm được.** Theo mặc định, ứng dụng điện thoại liệt kê mọi phiên theo nhóm *Needs you*, *Working*, *Done*, *Idle* và *Stopped*, rung khi một phiên cần bạn và hiển thị màn hình của phiên. Nếu được phép gõ, điện thoại còn gửi được câu trả lời, phím (Enter, Esc, Ctrl+C, mũi tên, y/n, 1–3) và tin nhắn chat. Kelpie gắn nhãn rủi ro (*Low risk*, *Medium risk*, *Dangerous*, *Risk unknown*) cho mỗi yêu cầu phê duyệt và hiện kèm toàn bộ lệnh. Trên Windows, một điện thoại được cấp quyền điều khiển qua Tailscale hoặc HTTPS còn có thể tiếp quản một khung trình duyệt hoặc xem màn hình máy tính. Nếu bạn bật *See and control the screen without Tailscale* trong Settings → Mobile, điện thoại còn xem và điều khiển được màn hình qua relay, kể cả ứng dụng trên relay, và mỗi lần bắt đầu xem đều phải xác nhận bằng Face ID. Khi dùng relay của Kelpie cho màn hình, bạn tin tưởng người vận hành relay, giống như khi dùng Chrome Remote Desktop bạn tin tưởng Google. Vì vậy, bạn nên bật xác thực hai bước cho tài khoản GitHub và Cloudflare của người vận hành, hoặc tự vận hành relay của mình. Màn hình qua relay bị giới hạn ở 500 kbps và 1 GB mỗi tháng, và khi hạn mức của tháng đã dùng hết thì màn hình đang xem qua relay cũng dừng lại. Khi bạn tắt công tắc này, mọi màn hình đang xem qua relay sẽ dừng trong vòng một giây. Qua relay, điện thoại chỉ liệt kê các cửa sổ theo tên chương trình chứ không hiện tiêu đề cửa sổ. Nếu điện thoại đang xem màn hình qua Tailscale rồi kết nối lại qua relay (hoặc ngược lại), màn hình sẽ dừng và bạn cần mở lại, kèm Face ID khi đi qua relay. Khi kết nối chậm, Kelpie giảm số khung hình trước rồi mới giảm kích thước hình, và nút *Sharpen* gửi lại phần bạn đang xem để chữ rõ nét. Nếu kết nối bị rớt, chẳng hạn khi điện thoại chuyển từ Wi-Fi sang dữ liệu di động, máy tính vẫn giữ màn hình thêm 45 giây để điện thoại tự kết nối lại.

**Bảo mật.** Nội dung truyền giữa điện thoại và máy tính được mã hoá đầu cuối, và bạn không cần tài khoản Kelpie. Relay chỉ chuyển tiếp các khung dữ liệu đã mã hoá, tuy nhiên relay vẫn thấy siêu dữ liệu kết nối. Quyền gõ còn cho điện thoại khởi động agent trong các dự án của bạn với cài đặt của máy tính, kể cả chế độ bỏ qua quyền, nên với ứng dụng trên relay, mã do relay phục vụ cũng có thể làm việc này trong lúc ứng dụng đang mở khoá. Kelpie chỉ cho ứng dụng trên relay biết cách tới relay, còn tên Tailscale và các địa chỉ trong mạng của bạn thì không bao giờ được gửi tới ứng dụng đó. Tuy vậy, ứng dụng vẫn biết tên máy tính, vì lúc ghép đôi Kelpie gửi tên này để ứng dụng hiển thị. Khi máy tính đang dùng Tailscale, relay cũng được báo địa chỉ Tailscale của máy như một trang mà relay cho phép vào phòng. Khi ứng dụng điện thoại đang khoá, dù được mở từ trang nào, máy tính cũng chỉ cho điện thoại biết cách tới relay cho tới khi bạn mở khoá. Vì mã của ứng dụng điện thoại được tải từ relay, [phần Phone access](user-guide.md#phone-access) giải thích mô hình tin cậy và cách kiểm tra những gì relay phục vụ. Để cắt quyền một điện thoại, bạn dùng *Unpair*; còn để gỡ mọi điện thoại, bạn dùng *Turn off phone access and revoke all*.

Chi tiết nằm ở phần [Phone access](user-guide.md#phone-access) và hướng dẫn [tự vận hành relay](relay-self-host.md).

## Thông báo

**Trên máy tính.** Kelpie chỉ dùng thông báo của hệ điều hành cho những việc cần bạn, gồm agent chờ trả lời, run chờ phê duyệt, lỗi, giới hạn chi phí và agent do điện thoại khởi động trên máy này. Nếu bật Settings → Notifications → *Also when a turn finishes*, bạn sẽ nhận thêm thông báo mỗi khi một lượt kết thúc, kèm âm báo tuỳ chọn. Biểu tượng thẻ cạnh chuông tắt hoặc bật thông báo trên máy tính.

**Ngoài máy tính.** Với Settings → Notifications → *Off this computer*, Kelpie gửi cùng các sự kiện đó qua ntfy, Telegram hoặc webhook. Bạn chọn loại sự kiện cần gửi, bỏ qua các lượt ngắn, và có thể chỉ gửi khi bạn không ngồi trước máy. Kelpie lưu token trong kho thông tin xác thực của hệ điều hành.

Chi tiết nằm ở các phần [Activity and notifications](user-guide.md#activity-and-notifications) và [On your phone](user-guide.md#on-your-phone-with-the-window-closed).

## Cá nhân hoá

**Giao diện.** Trong Settings → Appearance, bạn chọn ngôn ngữ (English, Tiếng Việt hoặc theo hệ thống), giao diện tối hoặc sáng, mật độ hiển thị, cỡ chữ giao diện, cũng như việc hiện Kip và chuyển động của Kip. Còn Settings → Terminal cho phép chọn phông chữ, cỡ chữ, số dòng cuộn lại, shell mặc định và bảng màu (Dracula, Nord, One Dark, Tokyo Night, Catppuccin, Gruvbox, Solarized, GitHub Light).

**Agent setup.** Trang Settings → Agent setup gom vào một chỗ những gì agent được biết và được dùng: MCP server của Claude Code, Codex, Gemini, OpenCode và Cursor; các tệp hướng dẫn như `CLAUDE.md` và `AGENTS.md`; danh sách skill và lệnh; và registry Agent Client Protocol. Mọi thay đổi đều có bản xem trước, bản sao lưu và bước kiểm tra tệp sau khi ghi.

**Plugin.** Một plugin là một thư mục chứa tệp `kelpie-plugin.json`, có thể thêm agent, lệnh, quick command, theme terminal, bố cục và mục trên thanh trạng thái. Plugin không làm gì cho tới khi bạn nhấn *Review & enable*, và màn hình duyệt khi đó liệt kê mọi chương trình mà plugin có thể chạy. Định dạng plugin được mô tả trong [plugins.md](plugins.md).

**Đọc chính tả.** Settings → Voice tải một mô hình nhận dạng giọng nói về máy một lần, và bạn chọn giữa mô hình tiếng Việt nhanh (26 MB) và Whisper cho khoảng 100 ngôn ngữ. Phím `Ctrl+Shift+Space` bắt đầu và kết thúc việc nghe, còn chữ được chèn vào đúng chỗ bạn đang gõ. Kelpie nhận dạng giọng nói ngay trên máy và không gửi đi đâu.

**Xuất và nhập cài đặt.** Với *Export and import settings* trong Settings → Session Daemon, bạn chuyển cài đặt, quick command, bố cục, automation, máy chủ SSH, danh sách plugin và tham số agent sang máy khác trong một tệp JSON. Kelpie không bao giờ xuất mật khẩu, token hay thông tin đăng nhập, và văn bản trông giống bí mật được thay bằng `[redacted]`. Khi nhập, ứng dụng liệt kê mọi mục quyết định chương trình nào sẽ chạy và chờ bạn xác nhận.

Chi tiết nằm ở các phần [Agent setup](user-guide.md#agent-setup), [Plugins](user-guide.md#plugins), [Voice dictation](user-guide.md#voice-dictation), [Export and import settings](user-guide.md#export-and-import-settings) và [Settings](user-guide.md#settings).

## Phím tắt

Trên Windows, phím tắt của Kelpie dùng `Ctrl+Shift` để các tổ hợp `Ctrl+chữ` vẫn tới được giao diện terminal của agent. Bạn có thể đổi mọi phím tắt trong Settings → Shortcuts.

| Thao tác | Windows / Linux | macOS |
|---|---|---|
| Tìm mọi thứ | Ctrl+Shift+P | ⌘⇧P |
| Mở tệp | Ctrl+Shift+O | ⌘P |
| Nhảy tới phiên hoặc project | Ctrl+Shift+K | ⌘K |
| Dashboard | Ctrl+Shift+M | ⌘⇧M |
| Phiên tiếp theo đang chờ bạn | Ctrl+Shift+. | ⌘⇧. |
| Board | Ctrl+Shift+J | ⌘⇧J |
| Terminal mới | Ctrl+Shift+T | ⌘T |
| Phiên mới… | Ctrl+Shift+N | ⌘⇧N |
| Chia phải / chia xuống | Ctrl+Shift+D / Ctrl+Shift+S | ⌘D / ⌘⇧D |
| Đóng khung | Ctrl+Shift+W | ⌘W |
| Phóng to / thu nhỏ khung | Ctrl+Shift+Enter | ⌘⇧↩ |
| Menu layout | Ctrl+Shift+L | ⌘⇧L |
| Run | Ctrl+Shift+R | ⌘⇧R |
| Mở trình duyệt | Ctrl+Shift+U | ⌘⇧U |
| Composer | Ctrl+Shift+I | ⌘⇧I |
| Đọc chính tả | Ctrl+Shift+Space | ⌘⇧Space |
| Terminal nổi | Ctrl+Shift+` | ⌘⌥A |
| Máy chủ SSH | Ctrl+Alt+H | ⌘⌥H |
| Yêu cầu sub-session đang mở | Ctrl+Shift+A | ⌘⇧A |
| Cài đặt | Ctrl+, | ⌘, |

Trong terminal, `Ctrl+C` sao chép khi có vùng chọn và gửi tín hiệu ngắt khi không có vùng chọn. Phím `Ctrl+V` dán, `Shift+Enter` xuống dòng khi bạn gõ yêu cầu cho agent, còn `Ctrl+click` mở liên kết. Nếu kéo tệp từ Explorer hoặc Finder vào terminal, bạn sẽ dán được đường dẫn của tệp đó. Bảng đầy đủ nằm ở phần [Keyboard shortcuts](user-guide.md#keyboard-shortcuts).

## Điều khiển bằng CLI

Agent và script có thể điều khiển Kelpie qua `kelpied` bằng các lệnh như sau:

```bash
kelpied terminal list                         # liệt kê phiên
kelpied terminal read <phiên> --lines 80      # đọc màn hình
kelpied terminal send <phiên> "chạy test" --enter
kelpied terminal wait <phiên>                 # chờ tới khi phiên ngừng làm việc
kelpied worktree set --comment "đã tái hiện lỗi" --status progress
kelpied review add src/app.ts:42 "kiểm tra giá trị rỗng"
kelpied run create run.json                   # tạo run từ tệp JSON
kelpied run report done "các check đã đạt"   # agent báo kết quả task
kelpied sub spawn codex "sửa #1" --worktree   # mở sub-session
kelpied call session.list                     # gọi trực tiếp daemon
```

Khi chạy bên trong một phiên Kelpie, `kelpied` hành động thay mặt phiên đó, còn cài đặt, automation, agent tuỳ chỉnh và truy cập từ điện thoại thì không bao giờ điều khiển được từ bên trong một phiên. Với những việc này, bạn cần chạy `kelpied` từ một terminal bên ngoài Kelpie.

Chi tiết nằm ở các phần [Terminals](user-guide.md#terminals) và [Automation](user-guide.md#automation).

## Khi gặp sự cố

- **Nếu agent không hiện trong danh sách khởi chạy**, bạn hãy kiểm tra xem CLI có chạy được trong terminal không, rồi mở Settings → Agents để xem Kelpie có tìm thấy nó không. Trên macOS và Linux, Kelpie tìm agent qua login shell, nên `PATH` phải được đặt trong profile của shell.
- **Nếu phiên hiện *interrupted* sau khi khởi động lại máy**, đó là hành vi bình thường. Tuỳ theo Settings → Agents → *Resume after a restart*, agent sẽ tiếp tục khi bạn mở phiên, tiếp tục tất cả cùng lúc, hoặc chờ bạn nhấn *Resume*.
- **Nếu một tính năng mới chưa xuất hiện sau khi cập nhật**, có thể tính năng đó cần daemon mới. Bạn khởi động lại daemon trong Settings → Session Daemon hoặc qua thông báo *Switch now*, và agent nào có khả năng resume sẽ tiếp tục cuộc hội thoại.
- **Nếu điện thoại báo "Not paired here"**, điều đó nghĩa là máy tính không nhận ra điện thoại hoặc không trả lời. Điện thoại sẽ tự thử lại; tuy nhiên, nếu bạn đã gỡ điện thoại này có chủ đích, hãy nhấn *Forget this device* rồi ghép đôi lại.

Nếu cần chi tiết hơn, [bản hướng dẫn tiếng Anh](user-guide.md) và tài liệu [kiến trúc](architecture.md) mô tả đầy đủ mọi hành vi.
