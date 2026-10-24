// Kelpie on a phone: pair once (QR code from Kelpie → Settings → Mobile), then see every session,
// what needs you, read a session's screen and — if the device may type — answer it.
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
  let open = null; // handle of the session on screen
  let lastScreen = "";
  let readTimer = 0;

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
      setTimeout(connect, 2500);
    };
  }
  const send = (msg) => ws && ws.readyState === 1 && ws.send(JSON.stringify(msg));

  function onMessage(m) {
    if (m.type === "hello") {
      device = m.device;
      conn.className = "dot on";
      sessions = m.sessions;
      projects = m.projects || {};
      render();
      const wanted = wantSession && sessions.find((s) => s.id === wantSession);
      wantSession = null;
      if (wanted) openSession(wanted.handle);
    } else if (m.type === "sessions") {
      const before = new Map(sessions.map((s) => [s.handle, s.status]));
      sessions = m.sessions;
      projects = m.projects || projects;
      // A buzz when something starts needing you.
      if (sessions.some((s) => s.status === "waiting" && before.get(s.handle) !== "waiting") && navigator.vibrate) navigator.vibrate(200);
      render();
    } else if (m.type === "screen" && m.handle === open) {
      if (m.text !== lastScreen) {
        lastScreen = m.text;
        const pre = document.querySelector(".screen");
        if (pre) {
          const atEnd = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 30;
          pre.textContent = m.text;
          if (atEnd) pre.scrollTop = pre.scrollHeight;
        }
      }
    } else if (m.type === "error") {
      alert(m.message);
    }
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
        kids.push(el("div", { class: "row", "data-handle": s.handle, onclick: () => openSession(s.handle) },
          el("span", { class: `dot ${s.alive ? s.status : "exited"}` }),
          el("div", { class: "main" }, el("div", { class: "name" }, label(s)), el("div", { class: "sub" }, `${s.kind} · ${where(s)}`)),
          s.unread ? el("span", { class: "badge" }, t("new")) : "",
        ));
      }
    }
    if (!kids.length) kids.push(el("div", { class: "card" }, el("p", { class: "muted" }, t("No sessions are running."))));
    view.replaceChildren(...kids);
  }

  function openSession(handle) {
    open = handle;
    lastScreen = "";
    renderSession();
    clearInterval(readTimer);
    const read = () => send({ type: "read", handle: open, lines: 200 });
    read();
    readTimer = setInterval(read, 1200);
  }

  function closeSession() {
    open = null;
    clearInterval(readTimer);
    render();
  }

  const KEYS = [["Enter", "enter"], ["Esc", "esc"], ["Ctrl+C", "ctrl-c"], ["↑", "up"], ["↓", "down"], ["Tab", "tab"], ["y", "y"], ["n", "n"], ["1", "1"], ["2", "2"], ["3", "3"]];

  function renderSession() {
    const s = sessions.find((x) => x.handle === open);
    if (!s) return closeSession();
    title.textContent = label(s);
    back.hidden = false;
    const existing = document.querySelector(".session");
    if (existing && existing.dataset.handle === String(s.handle)) {
      existing.querySelector(".needs").hidden = s.status !== "waiting";
      return;
    }
    const pre = el("pre", { class: "screen" }, lastScreen || (s.chat ? t("This is a chat-view session: open it on your computer.") : "…"));
    const needs = el("div", { class: "needs" }, t("This agent is waiting for you."));
    needs.hidden = s.status !== "waiting";
    let footer;
    if (device && device.can_write && s.alive && !s.chat) {
      const text = el("textarea", { class: "field", rows: "1", placeholder: t("Type a reply…") });
      const submit = () => {
        if (!text.value) return;
        send({ type: "input", handle: s.handle, text: text.value, enter: true });
        text.value = "";
      };
      text.addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } });
      footer = el("div", { class: "composer" },
        el("div", { class: "keys" }, KEYS.map(([l, k]) => el("button", { onclick: () => send({ type: "key", handle: s.handle, key: k }) }, l))),
        el("div", { class: "send" }, text, el("button", { class: "btn primary", onclick: submit }, t("Send"))),
      );
    } else {
      footer = el("div", { class: "readonly" }, !s.alive ? t("This session has stopped.") : s.chat ? t("Chat-view sessions are answered on the computer.") : t("This device can read but not type (Kelpie → Settings → Mobile)."));
    }
    view.replaceChildren(el("div", { class: "session", "data-handle": String(s.handle) }, needs, pre, footer));
  }

  // A pairing link opened in a tab that already has the app changes only the fragment.
  window.addEventListener("hashchange", () => {
    const params = new URLSearchParams(location.hash.slice(1));
    const want = params.get("s");
    if (want) {
      history.replaceState(null, "", location.pathname);
      const s = sessions.find((x) => x.id === want);
      if (s) openSession(s.handle);
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
    clearInterval(readTimer);
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
