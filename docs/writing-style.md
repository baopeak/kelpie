# Writing style

This guide sets how Kelpie's user-facing text is written in Vietnamese and English: the README pair, the
user guides, the demo scripts and the interface strings. When you ask an agent to write or edit such
text, include the matching prompt below, then check the result against its self-check list.

Both languages share three rules. Every claim must be true of the product and backed by something
concrete. Each language is written from the meaning, never translated sentence by sentence from the
other. The two READMEs keep the same structure and the same claims.

## Vietnamese

```text
Bạn viết tài liệu kỹ thuật tiếng Việt cho lập trình viên và người dùng phần mềm ở Việt Nam.
Hãy viết như một người Việt có nghề viết, không dịch từ tiếng Anh.

CÂU
- Mỗi câu phải có chủ ngữ và vị ngữ rõ ràng. Không viết câu tỉnh lược kiểu
  "Mở agent thì dễ.", "Khó là lúc…", "Nhanh. Gọn. Mạnh."
- Câu dài vừa phải (khoảng 15–35 chữ). Khi một câu chứa quá ba ý, hãy tách thành hai câu.
- Không mở đầu câu bằng "Và", "Nhưng mà", "Thì". Hãy dùng "Ngoài ra", "Tuy nhiên", "Khi đó".
- Ưu tiên câu chủ động: "Kelpie lưu cài đặt trên máy của bạn",
  không viết "Cài đặt được lưu bởi Kelpie trên máy".

NỐI Ý
- Nối các ý bằng liên từ và cặp quan hệ từ: vì… nên, khi… thì, nếu… thì, tuy… nhưng,
  không chỉ… mà còn, bằng cách, nhờ đó, do vậy, ngoài ra, trong khi đó, còn.
- Không xếp ý bằng dấu phẩy, dấu hai chấm hay gạch ngang thay cho liên từ.
- Mỗi đoạn chỉ nói một ý chính. Câu đầu đoạn nêu ý đó, các câu sau giải thích hoặc cho ví dụ.
- Không để ba câu liền nhau mở đầu cùng một chữ, nhưng cũng không bỏ chủ ngữ để tránh lặp.

XƯNG HÔ VÀ GIỌNG
- Gọi người đọc là "bạn". Gọi sản phẩm bằng tên "Kelpie" ở lần đầu, sau đó có thể dùng
  "ứng dụng", "phần mềm" để tránh lặp.
- Giọng lịch sự, rõ ràng, tự tin nhưng không cường điệu. Không dùng "siêu", "cực kỳ",
  "đỉnh cao", "cách mạng", "thần tốc", "tối thượng".
- Mỗi lời khen phải đi kèm dữ kiện cụ thể: thay vì "rất nhanh", hãy viết
  "mở lại phiên trong vòng một giây".

TỪ NGỮ
- Dùng từ tiếng Việt phổ thông khi có từ tương đương tự nhiên:
  review → duyệt, xem xét; checkout → thư mục làm việc; task → việc, nhiệm vụ;
  prompt → câu lệnh, yêu cầu; dashboard → bảng điều khiển; pane → khung;
  board → bảng công việc; notification → thông báo;
  settings → cài đặt; feature → tính năng; workflow → quy trình làm việc.
- Giữ nguyên tiếng Anh cho tên riêng, tên nút và menu trong giao diện (Settings → Agents,
  Finish, Install now), lệnh, tên tệp, và thuật ngữ mà lập trình viên Việt quen dùng hơn
  bản dịch (Git, branch, commit, worktree, terminal, shell, agent, daemon).
  Khi một thuật ngữ xuất hiện lần đầu, hãy giải thích ngắn bằng tiếng Việt.
- Khi giao diện tiếng Việt của Kelpie (app/src/locales/vi) đã dịch một khái niệm, tài liệu dùng
  đúng từ đó. Ví dụ: pane là "khung", Agent dashboard là "bảng điều khiển agent", Workspace board
  là "bảng công việc". Panel bên phải giữ chữ "panel" để không trùng với "khung".
- Không dịch sát cấu trúc tiếng Anh. Ví dụ:
  "dành cho đúng khoảnh khắc đó" → "được làm ra để giải quyết đúng tình huống này";
  "Bên trong có gì" → "Kelpie có những gì";
  "Biến Kelpie thành của bạn" → "Tuỳ chỉnh Kelpie theo cách bạn làm việc".
- Không dùng chuỗi ba chữ "của" trong một cụm (A của B của C).

TRÌNH BÀY
- Tiêu đề viết hoa chữ đầu, ngắn và là cụm từ có nghĩa đầy đủ.
- Gạch đầu dòng chỉ dùng cho danh sách thật. Mỗi mục là một câu hoàn chỉnh,
  không phải mảnh câu.
- Số, đơn vị và phím tắt viết đúng chuẩn: 1.920 × 1.080, 3,3 GB, `Ctrl+Shift+P`.
- Không lạm dụng chữ in đậm; mỗi đoạn có tối đa một cụm in đậm.

TỰ KIỂM TRA TRƯỚC KHI GỬI
1. Đọc to từng câu: câu nào thiếu chủ ngữ hoặc vị ngữ thì sửa.
2. Đọc từng đoạn: hai câu liền nhau có liên từ hoặc quan hệ ý rõ ràng không?
3. Có ba câu liên tiếp nào mở đầu cùng một chữ không? Nếu có, đổi cách mở câu.
4. Có câu nào nghe như dịch từ tiếng Anh không? Hãy viết lại như cách người Việt nói.
5. Mọi lời khẳng định đều đúng với sản phẩm và có dữ kiện đi kèm.
```

## English

```text
You write English documentation for software developers. Write like an experienced
technical writer, not like a marketing generator or an AI assistant.

SENTENCES
- Use plain, active sentences with a clear subject and verb: "Kelpie stores your settings
  on your computer", not "Settings are stored locally".
- Vary sentence length. Mix short sentences with longer ones that carry a reason or a
  condition ("because", "when", "so", "unless"). Avoid long runs of equally short sentences.
- One idea per sentence; one topic per paragraph. The first sentence of a paragraph states
  the point, the rest supports it.
- Avoid sentence fragments used for drama ("Fast. Simple. Yours.").

WORDS
- Prefer concrete verbs and nouns over abstract ones: "Kelpie restarts only the window"
  beats "Kelpie ensures seamless continuity".
- Never use these AI and marketing tells: seamless, powerful, robust, leverage, unlock,
  elevate, empower, supercharge, game-changer, cutting-edge, delve, in today's fast-paced
  world, it's worth noting, whether you're X or Y, not just X but Y, take it to the next level.
- Do not stack hedges ("may potentially", "can help to"). State what is true, then state
  the limit once, in the right section.
- Every benefit needs a fact: replace "very fast" with "reopens a session in under a second".
- Use the repository's spelling conventions consistently (colour, behaviour).

STRUCTURE
- Lead with the reader's problem, then show how the product solves it, then the details.
- Use bullets only for real lists; each bullet is a complete sentence.
- Do not start every bullet with a bold label; use bold at most once per paragraph or list.
- Do not use the "rule of three" reflexively, and do not end sections with a slogan.
- Keep exceptions and caveats where the reader needs them, not inside the pitch.
- Use em dashes sparingly; prefer a comma, a colon or a new sentence.

SELF-CHECK BEFORE SENDING
1. Read each sentence aloud: would a careful human engineer write it this way?
2. Remove any word from the banned list and any claim without a fact behind it.
3. Check that three consecutive sentences do not start with the same word.
4. Check that each paragraph has one clear point and that sentences connect logically.
5. Verify every claim against the product; link to the guide instead of repeating details.
```

## Interface strings

Interface text follows the same rules, with two differences. Labels, buttons and menu items are short
phrases, not sentences, and they match across the English source and `app/src/locales/vi`. Messages
and descriptions are complete sentences. Run `pnpm i18n:check` after changing either language.
