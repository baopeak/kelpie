// Kelpie on a phone: pair once (QR code from Kelpie → Settings → Mobile), then see every session,
// what needs you, read a session's screen and — if the device may type — answer it.
// Sessions are addressed by id; the daemon pushes the list and a followed session's screen as
// row diffs (protocol v2), nothing is polled.
// The device token lives in this browser only; Kelpie can revoke it at any time.
(() => {
  "use strict";
  const view = document.getElementById("view");
  const title = document.getElementById("title");
  const back = document.getElementById("back");
  const conn = document.getElementById("conn");
  const TOKEN = "kelpie-token";

  // The phone's own language: Vietnamese or English.
  const VI = {
    "Back": "Quay lại",
    "Connection": "Kết nối",
    "Pair with Kelpie": "Ghép với Kelpie",
    "Waiting for approval on your computer…": "Đang chờ duyệt trên máy tính…",
    "Pair this device": "Ghép thiết bị này",
    "Not paired": "Chưa ghép",
    "Name this device, then approve it in Kelpie on your computer.": "Đặt tên cho thiết bị này, rồi duyệt nó trong Kelpie trên máy tính.",
    "In Kelpie on your computer open Settings → Mobile → Pair a phone, and scan the QR code with this device.": "Trong Kelpie trên máy tính, mở Cài đặt → Di động → Ghép đôi điện thoại, rồi quét mã QR bằng thiết bị này.",
    "This device is no longer paired. Pair it again from Kelpie → Settings → Mobile.": "Thiết bị này không còn được ghép. Hãy ghép lại từ Kelpie → Cài đặt → Di động.",
    "Phone": "Điện thoại",
    "Android phone": "Điện thoại Android",
    "Browser": "Trình duyệt",
    "Needs you": "Cần bạn",
    "Working": "Đang làm",
    "Idle": "Rảnh",
    "Stopped": "Đã dừng",
    "new": "mới",
    "No sessions are running.": "Không có phiên nào đang chạy.",
    "This is a chat-view session: open it on your computer.": "Đây là phiên ở chế độ chat: hãy mở nó trên máy tính.",
    "This agent is waiting for you.": "Agent này đang chờ bạn.",
    "Type a reply…": "Nhập câu trả lời…",
    "Send": "Gửi",
    "Not sent: something else happened on the session first. Try again.": "Chưa gửi: có việc khác xảy ra trên phiên trước. Hãy thử lại.",
    "This session has stopped.": "Phiên này đã dừng.",
    "Chat-view sessions are answered on the computer.": "Phiên chế độ chat được trả lời trên máy tính.",
    "This device can read but not type (Kelpie → Settings → Mobile).": "Thiết bị này chỉ được xem, không được gõ (Kelpie → Cài đặt → Di động).",
  };
  const vi = (navigator.language || "").toLowerCase().startsWith("vi");
  const t = (s) => (vi && VI[s]) || s;
  document.documentElement.lang = vi ? "vi" : "en";
  const store = {
    get: (k) => { try { return localStorage.getItem(k); } catch { return null; } },
    set: (k, v) => { try { localStorage.setItem(k, v); } catch { /* private mode */ } },
    del: (k) => { try { localStorage.removeItem(k); } catch { /* private mode */ } },
  };
  // Phones paired before the rename to Kelpie keep their token under the old key.
  if (!store.get(TOKEN) && store.get("erka-token")) {
    store.set(TOKEN, store.get("erka-token"));
    store.del("erka-token");
  }
  const el = (tag, attrs = {}, ...kids) => {
    const n = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === "class") n.className = v;
      else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
      else if (v !== undefined && v !== null && v !== false) n.setAttribute(k, v === true ? "" : v);
    }
    for (const k of kids.flat()) n.append(k instanceof Node ? k : document.createTextNode(String(k)));
    return n;
  };

  let ws = null;
  let device = null;
  let sessions = [];
  let projects = {};
  let open = null; // id of the session on screen
  let epoch = null; // which run of the daemon the ids and frame numbers below belong to
  let lines = []; // the open session's screen, as the daemon last sent it
  let rev = null; // the number of the last frame applied to `lines`

  // ---------- pairing ----------
  const hashParams = new URLSearchParams(location.hash.slice(1));
  const pairCode = hashParams.get("p");
  // A link from a notification (#s=<session id>): opens that session once connected. It carries no
  // code or token, so it works only on a device that is already paired.
  let wantSession = hashParams.get("s");
  if (pairCode || wantSession) history.replaceState(null, "", location.pathname);

  function showPair(code, error) {
    title.textContent = t("Pair with Kelpie");
    back.hidden = true;
    const name = el("input", { class: "field", value: guessName(), maxlength: "40" });
    const status = el("div", { class: "muted" });
    const go = el("button", { class: "btn primary", onclick: async () => {
      go.disabled = true;
      status.textContent = t("Waiting for approval on your computer…");
      try {
        const r = await fetch("/pair", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, name: name.value.trim() || t("Phone") }) });
        const body = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(body.error || `HTTP ${r.status}`);
        store.set(TOKEN, body.token);
        connect();
      } catch (e) {
        status.textContent = String(e.message || e);
        go.disabled = false;
      }
    } }, t("Pair this device"));
    view.replaceChildren(el("div", { class: "card" },
      el("h2", {}, code ? t("Pair this device") : t("Not paired")),
      code
        ? el("div", {}, el("p", { class: "muted" }, t("Name this device, then approve it in Kelpie on your computer.")), name, el("p"), go, el("p"), status)
        : el("p", { class: "muted" }, error || t("In Kelpie on your computer open Settings → Mobile → Pair a phone, and scan the QR code with this device.")),
    ));
  }

  function guessName() {
    const ua = navigator.userAgent;
    if (/iPhone/.test(ua)) return "iPhone";
    if (/iPad/.test(ua)) return "iPad";
    if (/Android/.test(ua)) return t("Android phone");
    return t("Browser");
  }

  // ---------- connection ----------
  function connect() {
    const token = store.get(TOKEN);
    if (!token) return showPair(pairCode);
    ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`);
    ws.onopen = () => ws.send(JSON.stringify({ type: "auth", token }));
    ws.onmessage = (e) => onMessage(JSON.parse(e.data));
    ws.onclose = (e) => {
      conn.className = "dot";
      if (e.code === 4401) {
        store.del(TOKEN);
        return showPair(null, t("This device is no longer paired. Pair it again from Kelpie → Settings → Mobile."));
      }
      // Another tab or window of this device took the connection over: wait until this one is looked at.
      if (e.code === 4409) return;
      // Still paired, but this connection has to be made again (its permission changed): keep the token.
      if (e.code === 4408) return setTimeout(connect, 300);
      setTimeout(connect, 2500);
    };
  }
  const send = (msg) => ws && ws.readyState === 1 && ws.send(JSON.stringify(msg));

  function onMessage(m) {
    if (m.type === "hello") {
      device = m.device;
      conn.className = "dot on";
      // A daemon that restarted has other positions and numbers: start from nothing.
      if (epoch !== m.daemon_epoch) {
        lines = [];
        rev = null;
      }
      epoch = m.daemon_epoch;
      sessions = m.sessions;
      projects = m.projects || {};
      render();
      const wanted = wantSession && sessions.find((s) => s.id === wantSession);
      wantSession = null;
      if (wanted) openSession(wanted.id);
      else if (open !== null) follow();
    } else if (m.type === "sessions") {
      const before = new Map(sessions.map((s) => [s.id, s.status]));
      sessions = m.sessions;
      projects = m.projects || projects;
      // A buzz when something starts needing you.
      if (sessions.some((s) => s.status === "waiting" && before.get(s.id) !== "waiting") && navigator.vibrate) navigator.vibrate(200);
      render();
    } else if (m.type === "screen" && m.id === open) {
      applyFrame(m);
    } else if (m.type === "ack" && m.entered === false) {
      // Someone else typed meanwhile, or a prompt came up: the text is on the prompt line, not sent.
      alert(t("Not sent: something else happened on the session first. Try again."));
    } else if (m.type === "error") {
      alert(m.message);
    }
  }

  // Asks the daemon to follow the open session, carrying on from the frame we hold.
  function follow() {
    const s = sessions.find((x) => x.id === open);
    if (s && !s.chat) send({ type: "sub", id: open, height: 200, rev });
  }

  // A frame says which one it follows (`base`): scroll `shift` lines, then replace the rows it
  // names. One that follows a frame we do not hold is not applied; asking again gets the whole screen.
  function applyFrame(m) {
    if (m.base !== null && m.base !== rev) {
      rev = null;
      return follow();
    }
    const next = (m.base === null ? [] : lines).slice(m.shift);
    next.length = m.height;
    for (let i = 0; i < next.length; i++) if (next[i] === undefined) next[i] = "";
    for (const [i, text] of m.rows) if (i < next.length) next[i] = text;
    lines = next;
    rev = m.rev;
    showScreen();
  }

  function showScreen() {
    const pre = document.querySelector(".screen");
    if (!pre) return;
    const atEnd = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 30;
    pre.textContent = lines.join("\n");
    if (atEnd) pre.scrollTop = pre.scrollHeight;
  }

  // ---------- views ----------
  const label = (s) => s.name || (s.title && s.title.length < 48 ? s.title.replace(/^[^\w(]+\s*/u, "") : "") || s.kind;
  const where = (s) => projects[s.project_id] || (s.cwd || "").split(/[\\/]/).filter(Boolean).pop() || "";
  const groupOf = (s) => (!s.alive ? "stopped" : s.status === "waiting" ? "needs" : s.status === "working" ? "working" : "idle");
  const GROUPS = [["needs", "Needs you"], ["working", "Working"], ["idle", "Idle"], ["stopped", "Stopped"]];

  function render() {
    if (open !== null) return renderSession();
    title.textContent = "Kelpie";
    back.hidden = true;
    const kids = [];
    for (const [g, name] of GROUPS) {
      const list = sessions.filter((s) => groupOf(s) === g).sort((a, b) => b.last_activity - a.last_activity);
      if (!list.length) continue;
      kids.push(el("div", { class: "group" }, `${t(name)} · ${list.length}`));
      for (const s of list) {
        kids.push(el("div", { class: "row", "data-id": s.id, onclick: () => openSession(s.id) },
          el("span", { class: `dot ${s.alive ? s.status : "exited"}` }),
          el("div", { class: "main" }, el("div", { class: "name" }, label(s)), el("div", { class: "sub" }, `${s.kind} · ${where(s)}`)),
          s.unread ? el("span", { class: "badge" }, t("new")) : "",
        ));
      }
    }
    if (!kids.length) kids.push(el("div", { class: "card" }, el("p", { class: "muted" }, t("No sessions are running."))));
    view.replaceChildren(...kids);
  }

  function openSession(id) {
    // What was shown of another session means nothing for this one.
    if (open !== id) {
      lines = [];
      rev = null;
    }
    open = id;
    renderSession();
    follow();
  }

  function closeSession() {
    if (open !== null) send({ type: "unsub", id: open });
    open = null;
    render();
  }

  const KEYS = [["Enter", "enter"], ["Esc", "esc"], ["Ctrl+C", "ctrl-c"], ["↑", "up"], ["↓", "down"], ["Tab", "tab"], ["y", "y"], ["n", "n"], ["1", "1"], ["2", "2"], ["3", "3"]];

  function renderSession() {
    const s = sessions.find((x) => x.id === open);
    if (!s) return closeSession();
    title.textContent = label(s);
    back.hidden = false;
    const existing = document.querySelector(".session");
    if (existing && existing.dataset.id === s.id) {
      existing.querySelector(".needs").hidden = s.status !== "waiting";
      return;
    }
    const pre = el("pre", { class: "screen" }, s.chat ? t("This is a chat-view session: open it on your computer.") : lines.join("\n") || "…");
    const needs = el("div", { class: "needs" }, t("This agent is waiting for you."));
    needs.hidden = s.status !== "waiting";
    let footer;
    if (device && device.can_write && s.alive && !s.chat) {
      const text = el("textarea", { class: "field", rows: "1", placeholder: t("Type a reply…") });
      const submit = () => {
        if (!text.value) return;
        send({ type: "input", id: s.id, text: text.value, enter: true, epoch });
        text.value = "";
      };
      // The Enter that confirms a composition (Telex, VNI, Japanese…) is not a send.
      text.addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing && e.keyCode !== 229) { e.preventDefault(); submit(); } });
      footer = el("div", { class: "composer" },
        el("div", { class: "keys" }, KEYS.map(([l, k]) => el("button", { onclick: () => send({ type: "key", id: s.id, key: k, epoch }) }, l))),
        el("div", { class: "send" }, text, el("button", { class: "btn primary", onclick: submit }, t("Send"))),
      );
    } else {
      footer = el("div", { class: "readonly" }, !s.alive ? t("This session has stopped.") : s.chat ? t("Chat-view sessions are answered on the computer.") : t("This device can read but not type (Kelpie → Settings → Mobile)."));
    }
    view.replaceChildren(el("div", { class: "session", "data-id": s.id }, needs, pre, footer));
  }

  // A pairing link opened in a tab that already has the app changes only the fragment.
  window.addEventListener("hashchange", () => {
    const params = new URLSearchParams(location.hash.slice(1));
    const want = params.get("s");
    if (want) {
      history.replaceState(null, "", location.pathname);
      const s = sessions.find((x) => x.id === want);
      if (s) openSession(s.id);
      return;
    }
    const code = params.get("p");
    if (!code) return;
    history.replaceState(null, "", location.pathname);
    if (ws) {
      ws.onclose = null;
      ws.close();
      ws = null;
    }
    conn.className = "dot"; // not connected until the new pairing is approved
    open = null;
    showPair(code);
  });
  back.setAttribute("aria-label", t("Back"));
  conn.title = t("Connection");
  back.addEventListener("click", closeSession);
  window.addEventListener("popstate", closeSession);
  document.addEventListener("visibilitychange", () => { if (!document.hidden && (!ws || ws.readyState > 1)) connect(); });

  if (pairCode) showPair(pairCode);
  else connect();
})();
