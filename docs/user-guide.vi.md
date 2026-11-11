# Hướng dẫn sử dụng Kelpie

Kelpie chạy nhiều coding agent (Claude Code, Codex, OMP và 20 CLI khác) cùng các shell trên nhiều project. Các phiên chạy trong daemon nền `kelpied`, tách khỏi cửa sổ: việc chuyển project, cập nhật Kelpie hay lỗi giao diện không làm dừng phiên. Kelpie chỉ vẽ project mà bạn đang xem.

Tài liệu này hướng dẫn các thao tác thường dùng theo từng việc. [User guide tiếng Anh](user-guide.md) mô tả đầy đủ mọi chi tiết, giới hạn và trường hợp đặc biệt; mỗi phần dưới đây đều có liên kết tới phần tương ứng. Nếu bạn muốn trình diễn Kelpie, hãy xem [kịch bản demo](demo-scripts.vi.md).

Trong tài liệu này, phím tắt được ghi theo Windows và Linux. Trên macOS, `Ctrl+Shift` thường tương ứng với `⌘⇧`; bảng [phím tắt](#phím-tắt) ghi đủ cả hai.

## Cài đặt và cập nhật

**Cài đặt.**

- **Windows 10 1809+ và Windows 11:** bạn chạy bộ cài `Kelpie_x.y.z_x64-setup.exe`. Bộ cài chỉ cài cho người dùng hiện tại và không cần quyền quản trị.
- **macOS 12+:** bạn mở tệp `.dmg` và kéo Kelpie vào Applications.
- **Linux x86-64 (glibc 2.35 trở lên):** bạn dùng AppImage (`chmod +x` rồi chạy; cần FUSE 2) hoặc cài gói `.deb` bằng `sudo apt install ./Kelpie_x.y.z_amd64.deb`. Trên Linux, việc lưu bí mật cần một Secret Service như GNOME Keyring hoặc KWallet.

Kelpie không kèm agent. Bạn cài các CLI mình dùng (`claude`, `codex`, `omp`…) như bình thường. Kelpie tìm chúng qua `PATH` trên Windows, và qua login shell trên macOS và Linux.

**Cập nhật.** Kelpie kiểm tra bản mới trên GitHub 30 giây sau khi khởi động và sau đó mỗi 6 giờ. Khi có bản mới, thanh trạng thái hiện *Update x.y.z*. Hộp thoại cập nhật hiển thị release notes và ba lựa chọn: **Install now**, **Install when idle** (cài khi không còn phiên nào đang làm việc) và **Skip this version**. Bản cập nhật được ký số.

**Cập nhật không làm gián đoạn agent.** Khi cài bản mới, chỉ ứng dụng khởi động lại. Mọi phiên vẫn chạy trong daemon và hiện lại đúng chỗ cũ khi Kelpie mở ra. Đây là lợi ích chính của việc tách daemon khỏi cửa sổ: bạn không phải chờ agent làm xong mới dám cập nhật. Ngoại lệ duy nhất là lần cập nhật đầu tiên từ một bản 0.1 đời đầu, khi daemon còn chạy từ thư mục cài đặt: lần đó các phiên dừng một lần, hộp thoại báo trước, và agent tiếp tục sau đó.

**Đóng Kelpie.** Theo mặc định, đóng Kelpie sẽ dừng daemon cùng mọi phiên, automation và việc truyền tệp. Nếu còn việc đang chạy, Kelpie liệt kê chúng và hỏi **Close and stop** hoặc **Cancel**. Lần mở sau, các phiên hiện lại ở trạng thái *interrupted* và agent tiếp tục đúng cuộc hội thoại cũ. Nếu bạn muốn automation, run theo lịch, truy cập từ điện thoại hoặc việc sao chép tệp tiếp tục khi cửa sổ đóng, bạn đặt Settings → Session Daemon → *When Kelpie closes* thành **Keep it running in the background**.

Chi tiết: [Install](user-guide.md#install), [Resuming after a restart](user-guide.md#resuming-after-a-restart).

## Bắt đầu nhanh

1. Bạn nhấn `+` ở sidebar (hoặc mở tìm kiếm và chọn *Add Project Folder…*) để thêm một thư mục. Nếu thư mục là một Git repository, Kelpie tự thêm các worktree khác của nó vào dưới project.
2. Trên màn hình trống của project, bạn nhấn **Claude Code**, **Codex**, **OMP** hoặc **Terminal**. Nút `+` trên thanh tab mở hộp thoại phiên mới với tham số, tên, vị trí đặt pane, tài khoản, *Chat view* và một prompt đầu tiên.
3. Bạn nhấn `Ctrl+Shift+D` để chia pane sang phải và chọn thứ cần chạy trong pane mới.
4. Bạn giao việc cho agent và theo dõi sidebar: chấm hổ phách nghĩa là phiên cần bạn, chấm xanh dương nghĩa là đang làm việc, chấm xanh lá nghĩa là đã xong nhưng bạn chưa xem.

Khi chưa có project nào, Kelpie hiện màn hình chào với Kip, linh vật của Kelpie, cùng các nút *Add project folder*, *Add remote project (SSH)*, *SSH hosts* và *Command palette*. Trong vài lần mở đầu, Kelpie hiện một mẹo ngắn mỗi lần (tìm kiếm, dashboard, terminal hoặc chat, hội thoại cũ). Bạn có thể tắt mẹo hoặc bật lại trong Settings → Notifications.

## Không gian làm việc

**Sidebar.** Sidebar liệt kê mọi project cùng nhánh Git (`*` nghĩa là có thay đổi chưa commit; ↑↓ là số commit đi trước hoặc đi sau) và các phiên của nó theo thứ tự pane trên màn hình. Mỗi phiên hiện chấm trạng thái, tiêu đề việc, model và thời gian từ lần hoạt động cuối. Bạn nhấp chuột phải vào phiên để đổi tên, đánh dấu chưa đọc, khởi động lại, resume, chuyển sang chat view hoặc dừng. Bạn nhấp chuột phải vào project để tạo worktree, đổi tên hoặc sắp xếp. `Alt+1…9` chuyển nhanh giữa các project, còn `Alt+Shift+←/→` đi lùi và đi tới qua các pane bạn vừa xem.

**Tab và pane.** Mỗi tab là một việc; các pane là những thứ bạn cần nhìn cùng lúc trong việc đó. Bạn rê chuột lên một pane để thấy nút chia phải, chia xuống, phóng to (`Ctrl+Shift+Enter`) và đóng. Một pane mới hỏi bạn muốn chạy gì: bạn nhấp một agent hoặc *Terminal*, hoặc nhấn `1`–`9`.

**Layout.** Nút lưới trên thanh tab (`Ctrl+Shift+L`) sắp xếp tab theo các mẫu từ 2 cột tới 3×3, hoặc theo số pane mỗi hàng như `3,2`. **Tile all** gom các tab chỉ có một CLI vào một lưới đều; **Spread into tabs** làm ngược lại. Bạn kéo header của pane để đổi chỗ, kéo sang tab khác để chuyển pane, và kéo đường chia để đổi kích thước. **Broadcast input** gửi những gì bạn gõ tới mọi terminal của tab. *Save this layout* lưu cách sắp xếp kèm thứ đang chạy trong từng pane.

**Terminal nổi.** `` Ctrl+Shift+` `` mở một shell ở thư mục home, nằm trên mọi project. Khi bạn ẩn nó, shell vẫn chạy.

**Terminal dài.** Khi bạn cuộn lên trong một cuộc hội thoại dài, một nhãn ở góc đếm số dòng mới và đưa bạn về cuối. `Ctrl+Shift+↑/↓` nhảy tới prompt trước hoặc sau mà bạn đã gửi. Nhấp chuột phải vào terminal để sao chép, dán, xoá màn hình, **Copy Context** (400 dòng cuối), chia pane, chạy quick command hoặc hand-off cho agent khác.

Chi tiết: [Everyday use](user-guide.md#everyday-use), [Layouts](user-guide.md#layouts), [Terminals](user-guide.md#terminals).

## Tìm kiếm, dashboard và Activity

**Tìm mọi thứ (`Ctrl+Shift+P`).** Một ô tìm kiếm tìm các mục gần đây, phiên đang cần bạn, phiên, project, tệp, lệnh và cài đặt. Bạn gõ tiền tố `@` cho phiên, `#` cho tệp, `>` cho lệnh, `/` cho cài đặt và `!` cho các prompt cũ, hoặc nhấn `Tab` để đổi phạm vi.

**Dashboard (`Ctrl+Shift+M`).** Dashboard nhóm mọi phiên của mọi project thành *Needs you*, *Failed*, *Working*, *Done* và *Idle*. Nhóm *Needs you* xếp phiên chờ lâu nhất lên đầu. Nút **Answer** mở phần cuối màn hình của phiên, các nút cho từng phím mà lời nhắc chấp nhận (1 2 3, y n), Enter, Esc và một dòng văn bản. Câu trả lời chỉ được gửi nếu phiên vẫn đang chờ đúng lời nhắc đó. `Ctrl+Shift+.` nhảy tới phiên chờ lâu nhất, rồi phiên tiếp theo.

Với các agent không có hook (Codex, Gemini, Aider, OpenCode…), Kelpie coi phiên là *needs you* khi màn hình hiện một menu hoặc một câu hỏi có/không.

**Activity.** Chuông trên thanh trạng thái liệt kê những gì đã xảy ra khi bạn nhìn chỗ khác: agent đã xong hoặc cần bạn, automation đã chạy, worktree mới. Bạn nhấp một mục để tới đó và nhấp chuột phải để đổi trạng thái đã đọc. Phiên mà bạn đang nhìn không tạo thông báo.

Chi tiết: [Everyday use](user-guide.md#everyday-use), [Activity and notifications](user-guide.md#activity-and-notifications).

## Agent và chat view

**Các agent được hỗ trợ.** Ngoài Claude Code, Codex và OMP, Kelpie nhận diện 20 CLI khác: Gemini, Cursor CLI, OpenCode, GitHub Copilot, Grok, Qwen Code, Droid, Aider, Amp, Goose, Kimi, Crush, Cline, Kiro, Continue, Devin, Auggie, CodeBuddy, Mistral Vibe và Pi. Settings → Agents cho phép ẩn agent, sửa tham số và biến môi trường của từng agent. Biến môi trường được lưu dạng văn bản thường.

**Quyền.** Chế độ *Permissions: Manual* khiến agent hỏi bạn trước khi hành động. *Permissions: Yolo* khởi động agent với cờ bỏ qua xin quyền của chính agent đó.

**Chat view.** Trong header của pane agent, bạn nhấp biểu tượng chat để tiếp tục *cùng cuộc hội thoại* trong giao diện chat; biểu tượng terminal đưa bạn trở lại. Kelpie dừng giao diện này trước khi mở giao diện kia, nên một cuộc hội thoại không bao giờ có hai người ghi. Trong ô nhập, `/` liệt kê lệnh của agent, `@` liệt kê tệp, và với Codex `$` liệt kê skill. Các chip dưới ô nhập đổi model, mức effort và chế độ quyền. Transcript hiện mỗi lần dùng công cụ trên một dòng, gộp các lần đọc và tìm kiếm liên tiếp, và liệt kê các tệp đã đổi sau mỗi lượt. Bạn có thể dán ảnh, lưu câu trả lời vào tab *Saved*, và mở *Agent map* để xem các subagent của cuộc hội thoại.

**Phê duyệt trong chat.** Thẻ phê duyệt hiện lệnh hoặc nội dung sửa mà agent muốn làm. Bạn nhấn `1`–`9` để chọn, hoặc gõ điều agent nên làm thay thế rồi nhấn *Deny*.

**Hội thoại cũ.** Tab **Sessions** của panel bên phải liệt kê các cuộc hội thoại Claude, Codex và OMP trước đây, nhóm theo ngày và tìm được theo tiêu đề hoặc prompt đầu tiên. Bạn có thể resume trong terminal hoặc chat view.

**Giữ cuộc hội thoại khoẻ.** Menu của phiên có **Hand off to…** (mở agent khác với phần cuối transcript làm bối cảnh), **Compact**, **Clear** và **Restart** giữ nguyên cuộc hội thoại. *Hibernation* dừng agent đã xong và nằm yên trong project nền sau N phút; agent tiếp tục khi bạn mở lại.

**Nhiều tài khoản.** Settings → Accounts thêm tài khoản Claude Code hoặc Codex. Khi một phiên Claude Code chạm giới hạn sử dụng, dải thông báo phía trên pane cho bạn **Continue at reset**, **Switch account…** hoặc **Hand off to…**.

**Context và chi phí.** Mỗi phiên Claude Code, Codex và OMP có vòng tròn context cho biết cửa sổ ngữ cảnh đã dùng bao nhiêu. Trang **Usage & cost** thống kê chi phí và token theo ngày, project, agent hoặc model. Settings → Usage & Resources có giới hạn chi phí cho mỗi phiên.

Chi tiết: [Agents](user-guide.md#agents), [Everyday use](user-guide.md#everyday-use) (mục Chat view), [Past conversations](user-guide.md#past-conversations), [Several accounts](user-guide.md#several-claude-code--codex-accounts), [Context, tokens and cost](user-guide.md#context-tokens-and-cost).

## Sub-session

Một agent có thể mở các phiên agent khác để chia việc. Khác với subagent chạy bên trong tiến trình của agent, **sub-session** là một phiên Kelpie thật: phiên đó hiện trên sidebar, bạn mở, gõ vào hoặc đóng được, và kết quả của nó được gửi về phiên cha.

**Cách dùng.** Trong một phiên Claude, Codex hoặc agent khác do Kelpie khởi động, bạn yêu cầu bằng lời thường, ví dụ: *"mở sub-session để sửa #1 #2 #3 bằng Codex"*. Agent mở một phiên con cho mỗi việc. Khi phiên con báo xong, bị chặn hoặc cần bạn, kết quả hiện thành tin nhắn mới trong phiên cha.

**Phê duyệt.** Theo mặc định, mỗi sub-session hỏi bạn trước bằng một thẻ ở góc dưới bên phải. Thẻ ghi tên phiên yêu cầu, agent, thư mục, việc và model. Bạn chọn *Allow once*, *Allow for this session*, *Always allow* hoặc *Deny*. `Ctrl+Shift+A` đưa yêu cầu đang mở lên trước.

**Giới hạn.** Sub-session sâu tối đa 3 cấp, mỗi phiên có tối đa 10 phiên con đang chạy, và toàn máy có một giới hạn tự động hoặc do bạn đặt. Sub-session không bao giờ có nhiều quyền hơn phiên đã mở nó. Settings → Agents → **Sub-sessions** có ba chế độ: Off, Ask (mặc định) và Allow.

Chi tiết: [Sub-sessions](user-guide.md#sub-sessions).

## Worktree và board

**Tạo worktree.** Bạn nhấp chuột phải vào một repository và chọn **New worktree…**. Hộp thoại gợi ý tên nhánh, nhánh gốc và thư mục (`<repo>-worktrees/<branch>`). Worktree hiện dưới repository với phiên và layout riêng. **Remove worktree…** liệt kê trước những gì sẽ mất, rồi mới dừng phiên và xoá worktree; nhánh luôn được giữ lại.

**Tệp `.kelpie.yaml`.** Repository có thể chứa tệp này để chuẩn bị worktree mới. Kelpie hiển thị nội dung tệp theo commit gốc và chỉ chạy khi bạn đánh dấu đồng ý:

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

**Agent mới trong worktree riêng.** Settings → Agents → *Worktree for new agent sessions* cho phép mỗi agent mới chạy trong worktree riêng, với tên nhánh tạo từ prompt đầu tiên.

**Board (`Ctrl+Shift+J`).** Board hiện mọi project và worktree thành thẻ trong các cột *Todo*, *In progress*, *Review* và *Done*, kèm các agent đang chạy. Bạn kéo thẻ giữa các cột, ghim thẻ quan trọng và nhấp thẻ để mở. Mỗi worktree có thể có một ghi chú về việc đang làm; bạn đặt ghi chú từ menu sidebar, hoặc agent tự đặt bằng `kelpied worktree set --comment "…" --status review`.

Chi tiết: [Worktrees and the workspace board](user-guide.md#worktrees-and-the-workspace-board), [New agent sessions in a worktree](user-guide.md#new-agent-sessions-in-a-worktree).

## Review, Git và Finish

**Panel Changes (`Ctrl+Shift+G`).** Panel này push, pull (chỉ fast-forward) và sync, hiện xung đột khi đang merge hoặc rebase, và có **Resolve with AI**. Nút ✦ soạn thông điệp commit từ diff. Bạn nhấp tên nhánh ở đầu panel để đổi nhánh; Kelpie chỉ đổi khi không có gì bị mất. Phần *History* hiện 100 commit gần nhất, và phần *Stashes* cho phép áp dụng hoặc xoá stash. Với GitHub, panel hiện pull request của nhánh và trạng thái check, và có **Create pull request…**. Với GitLab, panel hiện merge request và pipeline.

**Ghi chú review.** Trong diff, bạn nhấp dấu `+` ở lề để ghi chú một dòng. Ghi chú của mọi tệp gom vào một thanh phía trên diff. **Send to agent ▾** gửi tất cả ghi chú thành một prompt cho agent đang chạy hoặc một agent mới. `F7` và `Shift+F7` chuyển giữa các thay đổi.

**Review với agent khác.** **Review with…** nhờ một agent khác, mặc định là agent không viết thay đổi đó, review worktree. Agent review có thể để ghi chú trên diff bằng `kelpied review add <file>:<line> "<comment>"`.

**Hàng đợi review và Finish.** Cột *Review* của board là hàng đợi review. Thẻ worktree trong cột này hiện số tệp, số dòng thêm và bớt, số commit, check, chi phí và số lượt. **Approve → Finish** mở hộp thoại Finish: danh sách tệp chưa commit sẽ được lưu thành commit "WIP", cách gộp (squash, merge commit hoặc rebase), thông điệp và các check. Sau đó, Kelpie dừng các phiên của worktree, commit, gộp, chạy check, chỉ di chuyển nhánh đích nếu nhánh đó không đổi trong lúc chờ, và xoá worktree sau cùng. Finish không bao giờ dùng force. Check được đọc từ `.kelpie.yaml` trên nhánh đích, không bao giờ từ nhánh đang được gộp, và chỉ chạy sau khi bạn duyệt đúng danh sách đó.

**Lượt và hoàn tác.** Khi bật Settings → Agents → **Turn snapshots**, Kelpie chụp trạng thái thư mục sau mỗi lượt của agent. Tab **Turns** liệt kê các lượt, và nút hoàn tác cho bạn xem trước mọi tệp sẽ được khôi phục trước khi thay đổi bất cứ điều gì.

Chi tiết: [Source control and review](user-guide.md#source-control-and-review), [Review and Finish](user-guide.md#review-and-finish), [Review with another agent](user-guide.md#review-with-another-agent-and-best-of-several), [Turns and undo](user-guide.md#turns-and-undo).

## Kế hoạch, run và automation

**Tab Plan.** Tab Plan đọc kế hoạch và spec có sẵn trong repository: `plans/<tên>/plan.md` cùng các tệp `phase-*.md`, `specs/**`, `.kiro/specs/**` và `docs/plans/**`. Kelpie không có định dạng riêng; tab chỉ đọc checklist Markdown `- [ ]` và `- [x]`. Khi bạn tích một ô, tệp chỉ thay đổi đúng một ký tự. Nút tia sét ở cuối một task (**Run with…**) giao task đó cho một agent, chạy ngay tại thư mục, trong worktree riêng hoặc trên một máy chủ SSH đã liên kết. **New spec** tạo `requirements.md`, `design.md` và `tasks.md` cho một spec mới.

**Run (`Ctrl+Shift+R`).** Một run chia việc lớn thành các task. Mỗi task có prompt, agent, các task phải chờ, tuỳ chọn cần phê duyệt trước khi chạy và tuỳ chọn worktree riêng. *Plan tasks with AI* soạn sẵn danh sách task từ một mục tiêu để bạn xem lại. Daemon chạy các task theo thứ tự phụ thuộc. Bạn cũng có thể tạo run từ một tệp JSON bằng `kelpied run create run.json`.

**Thử nhiều agent.** *Try with several agents…* gửi một prompt tới 2–5 agent, mỗi agent trong worktree riêng. Tab **Compare** của run đặt các kết quả cạnh nhau: trạng thái, thời gian, chi phí, số tệp, số dòng thay đổi và check. **Use this one** mở Finish cho kết quả được chọn và dọn các kết quả còn lại.

**Automation.** Settings → Automations chạy một agent với một prompt theo lịch (mỗi giờ, mỗi ngày, ngày làm việc hoặc mỗi tuần), hoặc khi có sự kiện: tệp thay đổi, agent xong một lượt, có commit mới, check của pull request thất bại, hoặc một worktree được Finish. Automation có thể chạy một lệnh, nhưng lệnh đó phải được bạn duyệt và chạy không qua shell. *Dry run* cho biết automation sẽ làm gì mà không chạy thật.

**Quick command.** Settings → Quick Commands lưu các lệnh và prompt hay dùng. Bạn chạy chúng từ tìm kiếm (`>`), từ menu phiên mới hoặc bằng `Ctrl+1…9`.

**Issue tracker.** Settings → Integrations kết nối Linear, Jira và GitLab. Panel **Issues** liệt kê issue đang mở; bạn có thể giao issue cho agent, tạo worktree cho issue hoặc đổi trạng thái của nó.

Chi tiết: [Specs and tasks](user-guide.md#specs-and-tasks-the-plan-tab), [Orchestration runs](user-guide.md#orchestration-runs), [Automations](user-guide.md#automations), [Quick commands](user-guide.md#quick-commands), [Issue trackers](user-guide.md#issue-trackers-and-gitlab).

## Tệp, trình duyệt và tiến trình

**Tệp.** Panel bên phải có Files (lọc theo tên hoặc tìm theo nội dung), Changes và Sessions. Bạn nhấp một tệp để mở trong trình soạn thảo (`Ctrl+S` lưu; Kelpie phát hiện khi tệp bị đổi từ bên ngoài). Tệp CSV và TSV mở thành bảng sắp xếp và lọc được; PDF, notebook Jupyter, sơ đồ Mermaid và Markdown đều được hiển thị trực tiếp. `Alt+Z` bật hoặc tắt ngắt dòng. Menu của project mở thư mục trong VS Code, Cursor, Windsurf, Zed, Sublime Text hoặc IntelliJ IDEA nếu máy có cài.

**Composer (`Ctrl+Shift+I`).** Composer là ô soạn nhiều dòng trên phiên đang chọn. Bạn gõ `@` để nhắc tới tệp, thư mục, `@selection`, `@diff`, `@note`, `@terminal:<phiên>` hoặc `@issue:<mã>`. Bí mật trong nội dung được che trước khi gửi.

**Trình duyệt (`Ctrl+Shift+U`).** Pane trình duyệt có thanh địa chỉ, công cụ nhà phát triển và chụp màn hình trang. **Design mode** cho phép bạn nhấp các phần tử trên trang, thêm ghi chú và gửi cho agent trong một prompt.

**Processes.** Khi một agent trên màn hình khởi động một trang web (dev server, dashboard…), trang đó được mở trong một tab nền và Kelpie báo ngắn. Tab **Processes** của panel bên phải liệt kê những gì mỗi phiên đang chạy dưới dạng cây, kèm PID, bộ nhớ, CPU và cổng. Bạn nhấp một cổng để mở trang trong tab trình duyệt. **Kill** dừng một chương trình và những gì nó khởi động; **Kill children** dừng những gì một phiên đã khởi động mà vẫn giữ phiên. Cả hai đều hỏi trước và nói rõ cổng nào sẽ được giải phóng.

Chi tiết: [Files](user-guide.md#files), [Composer](user-guide.md#composer), [Browser panes](user-guide.md#browser-panes-and-design-mode), [Processes](user-guide.md#processes-ports-and-preview).

## Làm việc qua SSH

**Project trên máy chủ.** *Add Remote Project (SSH)…* chọn một máy chủ và một thư mục trên đó. Terminal và agent chạy trên máy chủ qua `ssh -t`, bên trong `tmux` nếu máy chủ có, nên phiên vẫn sống khi kết nối rớt. Cây tệp, trình soạn thảo, tìm kiếm và Git cũng hoạt động qua SSH. Kelpie dùng OpenSSH và cấu hình SSH của bạn.

**Trang SSH hosts (`Ctrl+Alt+H`).** Trang này quản lý máy chủ theo kiểu Termius:

- Mỗi máy chủ có tên, nhóm, thẻ, màu, ghi chú, địa chỉ, người dùng và cổng. Bạn đăng nhập bằng khoá, SSH agent hoặc mật khẩu được lưu trong kho thông tin xác thực của hệ điều hành.
- Mỗi máy chủ có thể có màu và theme terminal riêng, để môi trường production không bao giờ trông giống máy tính của bạn.
- Một cú nhấp mở *Terminal*, **Claude Code here** hoặc agent khác trên máy chủ.
- Tab *Files* duyệt, tải lên, tải xuống, đổi tên và xoá tệp, với hàng đợi truyền tệp.
- Bạn có thể chạy một lệnh trên nhiều máy chủ cùng lúc, quản lý khoá trong tab *Keychain*, xem *Known hosts* và tạo quy tắc *Port forwarding*.

**Pane SSH trong mọi project.** Một project trên máy có thể chứa terminal và agent chạy trên máy chủ, cạnh các pane cục bộ. Pane mới và hộp thoại *New session…* có dòng **Run on**: *This computer*, các máy chủ đã liên kết với project, hoặc *Other host…*. Pane trên máy khác có một sọc theo màu của máy chủ.

**Giữ phiên chạy trên máy chủ.** Bạn cài `tmux` trên máy chủ, ví dụ `sudo apt-get install -y tmux` trên Ubuntu. Khi đó, phiên vẫn chạy trên máy chủ khi mạng rớt, khi bạn đóng Kelpie hoặc khi máy tính của bạn ngủ.

Chi tiết: [Remote projects](user-guide.md#remote-projects-ssh), [SSH hosts](user-guide.md#ssh-hosts), [SSH panes in any project](user-guide.md#ssh-panes-in-any-project), [Keeping sessions running on a server](user-guide.md#keeping-sessions-running-on-a-server).

## Điện thoại

Truy cập từ điện thoại mặc định tắt. Settings → Mobile → *Mobile access* bật tính năng này. Điện thoại có thể kết nối theo ba cách: ứng dụng Kelpie tải từ relay của Kelpie (không cần Tailscale, là cách mặc định của thiết lập mới), ứng dụng do chính máy tính phục vụ qua Tailscale, hoặc trình duyệt trên cùng mạng.

**Ghép đôi bằng ứng dụng Kelpie.**

1. Bạn bật *Mobile access*. Với thiết lập mới, Kelpie bật relay và hỏi bạn xác nhận một lần.
2. Ở bước **Install**, bạn quét mã bằng camera điện thoại và cài Kelpie lên màn hình chính (Share → Add to Home Screen trên iPhone, nút Install trên Android).
3. Ở bước **Pair**, bạn mở ứng dụng trên điện thoại, nhấn *Scan the code*, rồi nhập sáu chữ số điện thoại hiện vào Kelpie. Mã ghép đôi chỉ dùng một lần và hết hạn sau hai phút.
4. Bạn chọn *Allow reading* hoặc *Allow reading and typing*, rồi xác nhận bằng Windows Hello, mật khẩu hoặc Touch ID.
5. Điện thoại yêu cầu bạn thiết lập khoá ứng dụng bằng Face ID, vân tay hoặc mã máy. Bạn nên làm ngay.

**Điện thoại làm được gì.** Điện thoại liệt kê mọi phiên theo nhóm *Needs you*, *Working*, *Idle* và *Stopped*, rung khi một phiên cần bạn và hiện màn hình của phiên. Nếu được phép gõ, điện thoại gửi câu trả lời, phím (Enter, Esc, Ctrl+C, mũi tên, y/n, 1–3) và tin nhắn chat. Mỗi yêu cầu phê duyệt được gắn nhãn rủi ro của Kelpie (*Low risk*, *Medium risk*, *Dangerous*, *Risk unknown*) kèm toàn bộ lệnh. Trên Windows, một điện thoại được cấp quyền điều khiển qua Tailscale hoặc HTTPS còn có thể tiếp quản một pane trình duyệt hoặc xem màn hình máy tính.

**Bảo mật.** Nội dung giữa điện thoại và máy tính được mã hoá đầu cuối, và không cần tài khoản Kelpie. Relay chỉ chuyển tiếp các khung đã mã hoá nhưng thấy siêu dữ liệu kết nối. Mã của ứng dụng điện thoại được tải từ relay; [phần Phone access](user-guide.md#phone-access) giải thích mô hình tin cậy và cách kiểm tra những gì relay phục vụ. Để cắt quyền một điện thoại, bạn dùng *Unpair*, hoặc *Turn off phone access and revoke all* để gỡ mọi điện thoại.

Chi tiết: [Phone access](user-guide.md#phone-access), [Tự host relay](relay-self-host.md).

## Thông báo

**Trên máy tính.** Thông báo của hệ điều hành chỉ dành cho những việc cần bạn: agent chờ trả lời, run chờ phê duyệt, lỗi và giới hạn chi phí. Settings → Notifications → **Also when a turn finishes** bật thêm thông báo khi một lượt kết thúc, kèm âm báo tuỳ chọn. Biểu tượng thẻ cạnh chuông tắt hoặc bật thông báo trên máy tính.

**Ngoài máy tính.** Settings → Notifications → *Off this computer* gửi cùng các sự kiện qua **ntfy**, **Telegram** hoặc **webhook**. Bạn chọn loại sự kiện, bỏ qua các lượt ngắn và có thể chỉ gửi khi bạn không ở trước máy. Token được lưu trong kho thông tin xác thực của hệ điều hành.

Chi tiết: [Activity and notifications](user-guide.md#activity-and-notifications), [On your phone](user-guide.md#on-your-phone-with-the-window-closed).

## Cá nhân hoá

**Giao diện.** Settings → Appearance chọn ngôn ngữ (English, Tiếng Việt hoặc theo hệ thống), theme tối hoặc sáng, mật độ, cỡ chữ giao diện, và việc hiện Kip cùng chuyển động của Kip. Settings → Terminal chọn font, cỡ chữ, số dòng cuộn lại, shell mặc định và bảng màu (Dracula, Nord, One Dark, Tokyo Night, Catppuccin, Gruvbox, Solarized, GitHub Light).

**Agent setup.** Settings → Agent setup gom vào một trang những gì agent được biết và được dùng: MCP server của Claude Code, Codex, Gemini, OpenCode và Cursor; các tệp hướng dẫn như `CLAUDE.md` và `AGENTS.md`; danh sách skill và lệnh; và registry Agent Client Protocol. Mọi thay đổi đều có bản xem trước, bản sao lưu và bước kiểm tra tệp sau khi ghi.

**Plugin.** Plugin là một thư mục có tệp `kelpie-plugin.json`, thêm agent, lệnh, quick command, theme terminal, layout và mục trên thanh trạng thái. Plugin không làm gì cho tới khi bạn nhấn **Review & enable**; màn hình review liệt kê mọi chương trình plugin có thể chạy. Định dạng: [plugins.md](plugins.md).

**Đọc chính tả.** Settings → Voice tải một mô hình nhận dạng giọng nói một lần: mô hình tiếng Việt nhanh (26 MB) hoặc Whisper cho khoảng 100 ngôn ngữ. `Ctrl+Shift+Space` bắt đầu và kết thúc việc nghe; chữ được chèn vào chỗ bạn đang gõ. Giọng nói được nhận dạng ngay trên máy và không được gửi đi đâu.

**Xuất và nhập cài đặt.** Settings → Session Daemon → **Export and import settings** chuyển cài đặt, quick command, layout, automation, máy chủ SSH, danh sách plugin và tham số agent sang máy khác trong một tệp JSON. Mật khẩu, token và thông tin đăng nhập không bao giờ được xuất; văn bản giống bí mật được che bằng `[redacted]`. Khi nhập, Kelpie liệt kê mọi thứ quyết định chương trình nào sẽ chạy và chờ bạn xác nhận.

Chi tiết: [Agent setup](user-guide.md#agent-setup), [Plugins](user-guide.md#plugins), [Voice dictation](user-guide.md#voice-dictation), [Export and import settings](user-guide.md#export-and-import-settings), [Settings](user-guide.md#settings).

## Phím tắt

Phím tắt của Kelpie dùng `Ctrl+Shift` trên Windows để các phím `Ctrl+chữ` vẫn tới được giao diện terminal của agent. Mọi phím tắt đều có thể đổi trong Settings → Shortcuts.

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
| Đóng pane | Ctrl+Shift+W | ⌘W |
| Phóng to / thu nhỏ pane | Ctrl+Shift+Enter | ⌘⇧↩ |
| Menu layout | Ctrl+Shift+L | ⌘⇧L |
| Run | Ctrl+Shift+R | ⌘⇧R |
| Mở trình duyệt | Ctrl+Shift+U | ⌘⇧U |
| Composer | Ctrl+Shift+I | ⌘⇧I |
| Đọc chính tả | Ctrl+Shift+Space | ⌘⇧Space |
| Terminal nổi | Ctrl+Shift+` | ⌘⌥A |
| Máy chủ SSH | Ctrl+Alt+H | ⌘⌥H |
| Yêu cầu sub-session đang mở | Ctrl+Shift+A | ⌘⇧A |
| Cài đặt | Ctrl+, | ⌘, |

Trong terminal, `Ctrl+C` sao chép khi có vùng chọn và gửi tín hiệu ngắt khi không có, `Ctrl+V` dán, `Shift+Enter` xuống dòng trong prompt của agent, và `Ctrl+click` mở liên kết. Bạn kéo tệp từ Explorer hoặc Finder vào terminal để dán đường dẫn. Bảng đầy đủ: [Keyboard shortcuts](user-guide.md#keyboard-shortcuts).

## Điều khiển bằng CLI

Agent và script có thể điều khiển Kelpie qua `kelpied`:

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

Bên trong một phiên Kelpie, `kelpied` nói thay cho phiên đó. Cài đặt, automation, agent tuỳ chỉnh và truy cập từ điện thoại không bao giờ điều khiển được từ bên trong một phiên; bạn chạy `kelpied` từ terminal bên ngoài Kelpie cho những việc đó.

Chi tiết: [Terminals](user-guide.md#terminals), [Automation](user-guide.md#automation).

## Khi gặp sự cố

- **Agent không hiện trong launcher.** Bạn kiểm tra rằng CLI chạy được trong terminal, rồi mở Settings → Agents để xem Kelpie có tìm thấy nó không. Trên macOS và Linux, Kelpie tìm agent qua login shell, nên `PATH` phải được đặt trong profile của shell.
- **Phiên hiện *interrupted* sau khi khởi động lại máy.** Đây là hành vi bình thường. Theo Settings → Agents → *Resume after a restart*, agent tiếp tục khi bạn mở phiên, tiếp tục tất cả cùng lúc, hoặc chờ bạn nhấn **Resume**.
- **Một tính năng mới chưa xuất hiện sau khi cập nhật.** Một số tính năng cần daemon mới. Settings → Session Daemon hoặc thông báo *Switch now* khởi động lại daemon; agent có khả năng resume sẽ tiếp tục cuộc hội thoại.
- **Điện thoại báo "Not paired here".** Máy tính không nhận ra điện thoại hoặc không trả lời. Điện thoại tự thử lại. Nếu bạn đã gỡ điện thoại có chủ đích, hãy nhấn *Forget this device* rồi ghép đôi lại.

Nếu bạn cần chi tiết hơn, [user guide tiếng Anh](user-guide.md) và tài liệu [kiến trúc](architecture.md) mô tả đầy đủ mọi hành vi.
