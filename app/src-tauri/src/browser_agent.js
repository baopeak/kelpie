// Kelpie's page engine for agents (`browser/agent.rs`), injected into every page (main frame) before the page's own scripts. Until Kelpie
// calls it, it only records page errors and times the person's own input (trusted pointer, key and wheel events), so that agents step
// back while the person uses the page; in a pane opened for an agent it also keeps the console (the last 300 messages) from the start.
// Kelpie calls `window.__kelpieAgent.<fn>(nonce, args)`; a call without the pane's nonce does nothing, and each function answers at once
// with a JSON string, `{"ok": …}` or `{"error": "…"}`. The global is frozen and the main platform functions it uses are taken at
// document start, so a page can spoof only its own content, not the address, the document or the answer channel. Values of password,
// one-time-code, card and other secret fields never leave the page.
(() => {
  if (window.top !== window || window.__kelpieAgent) return;
  const NONCE = "__KELPIE_AGENT_NONCE__", AGENT_PANE = __KELPIE_AGENT_PANE__;
  const W = window, D = document, O = Object, call = Function.prototype.call;
  const fn = (p, k) => call.bind(p[k]);
  const get = (p, k) => call.bind(O.getOwnPropertyDescriptor(p, k).get);
  const set = (p, k) => call.bind(O.getOwnPropertyDescriptor(p, k).set);
  const EP = Element.prototype, NP = Node.prototype, HP = HTMLElement.prototype, IP = HTMLInputElement.prototype, TP = HTMLTextAreaElement.prototype;
  const attr = fn(EP, "getAttribute"), box = fn(EP, "getBoundingClientRect"), scrollIn = fn(EP, "scrollIntoView"), matches = fn(EP, "matches");
  const checkVis = EP.checkVisibility && fn(EP, "checkVisibility");
  const first = get(NP, "firstChild"), next = get(NP, "nextSibling"), up = get(NP, "parentNode"), kind = get(NP, "nodeType"), live = get(NP, "isConnected"), rootOf = fn(NP, "getRootNode");
  const tagOf = get(EP, "localName"), shadowOf = get(EP, "shadowRoot"), data = get(CharacterData.prototype, "data"), innerText = get(HP, "innerText"), editable = get(HP, "isContentEditable");
  const styleOf = W.getComputedStyle.bind(W), fire = fn(EventTarget.prototype, "dispatchEvent"), on = fn(EventTarget.prototype, "addEventListener");
  const focusEl = fn(HP, "focus"), exec = fn(Document.prototype, "execCommand"), submitForm = HTMLFormElement.prototype.requestSubmit && fn(HTMLFormElement.prototype, "requestSubmit");
  const val = { input: [get(IP, "value"), set(IP, "value")], textarea: [get(TP, "value"), set(TP, "value")] };
  const json = JSON.stringify, now = Date.now, Dt = Date, U = URL, Wm = WeakMap, Mp = Map, Wr = WeakRef, Ws = WeakSet, min = Math.min;
  const Ev = Event, ME = MouseEvent, PE = W.PointerEvent || MouseEvent, KE = KeyboardEvent, IE = W.InputEvent || Event, MO = MutationObserver;
  const later = W.setTimeout.bind(W), unlater = W.clearTimeout.bind(W), hist = W.history, back = fn(History.prototype, "back"), forward = fn(History.prototype, "forward");
  const ok = (v) => json({ ok: v }), fail = (e) => json({ error: String((e && e.message) || e) });
  const norm = (s) => String(s || "").replace(/\s+/g, " ").trim(), cut = (s, n) => (s.length > n ? s.slice(0, n) + "…" : s);
  const el1 = (x) => { try { return !!x && kind(x) === 1; } catch { return false; } };
  const pe = (x) => { const p = up(x); return el1(p) ? p : null; };

  // ---- page errors and the person's input from document start; the console and dialogs once an agent uses the page ----
  const ring = []; let seq = now() * 1000, lost = 0, hooked = false;
  const str = (a) => { try { return typeof a === "string" ? a : a instanceof Error ? String(a.stack || a).split("\n")[0] : a && typeof a === "object" ? json(a) || String(a) : String(a); } catch { return "[object]"; } };
  const file = (u) => { try { const x = new U(u, location.href); return x.pathname.split("/").pop() || x.host; } catch { return ""; } };
  const where = (stack) => { const m = /\(?(\S+?):(\d+):\d+\)?$/.exec(String(stack || "").split("\n")[2] || ""); return m ? file(m[1]) + ":" + m[2] : ""; };
  const log = (level, text, src) => { ring.push({ n: ++seq, t: now(), level, text: String(text).slice(0, 1000), src: src || "" }); if (ring.length > 300) { ring.shift(); lost++; } };
  const dlg = { mode: "", until: 0, seen: [] };
  // Wrapping the console makes DevTools show the wrapper as every message's source: done in a pane the person opened only once an agent uses it.
  const hook = () => {
    if (hooked) return;
    hooked = true;
    const C = W.console;
    for (const k of ["log", "info", "warn", "error", "debug"]) {
      const orig = C[k];
      if (typeof orig === "function") C[k] = function (...a) { try { log(k === "warn" ? "warning" : k, a.map(str).join(" "), k === "warn" || k === "error" ? where(new Error().stack) : ""); } catch {} return orig.apply(this, a); };
    }
    // Dialogs are answered only while an agent's action runs (`act`), never once the person touches the page.
    for (const k of ["alert", "confirm", "prompt"]) {
      const orig = W[k];
      W[k] = function (msg, def) {
        if (!dlg.mode || now() > dlg.until) return orig.apply(this, arguments);
        const yes = dlg.mode === "accept";
        if (dlg.seen.length < 10) dlg.seen.push({ type: k, message: cut(norm(msg), 300), answer: k === "alert" ? "closed" : yes ? "accepted" : "dismissed" });
        return k === "alert" ? undefined : k === "confirm" ? yes : yes ? (def === undefined ? "" : String(def)) : null;
      };
    }
  };
  if (AGENT_PANE) hook();
  on(W, "error", (e) => { const t = e.target; if (el1(t)) log("error", `Failed to load ${tagOf(t)} ${scrub(t.currentSrc || t.src || t.href || "")}`); else log("error", e.message || str(e.error), e.filename ? file(e.filename) + ":" + e.lineno : ""); }, true);
  on(W, "unhandledrejection", (e) => log("error", "Unhandled rejection: " + str(e.reason)), true);
  let busyTill = 0, person = 0;
  for (const t of ["pointerdown", "keydown", "wheel"]) on(W, t, (e) => { if (e.isTrusted && now() > busyTill) { person = now(); dlg.mode = ""; } }, { capture: true, passive: true });

  // ---- what an element is ----
  const SECRET = /(passw|passcode|secret|token|api[-_ ]?key|otp|one[-_ ]?time|mfa|2fa|cvv|cvc|card|cc-|ccnum|(^|[^a-z])pin([^a-z]|$)|ssn|security.?code|session|csrf|xsrf|auth|signature|sig$|credential|cookie)/i;
  const SECRET_VALUE = /^(?:[A-Za-z0-9+/_-]{32,}={0,2}|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*)$/;
  function scrub(u) {
    try {
      const x = new U(u, location.href);
      for (const [k, v] of [...x.searchParams]) if (SECRET.test(k) || SECRET_VALUE.test(v)) x.searchParams.set(k, "[redacted]");
      if (/access_token|id_token/.test(x.hash)) x.hash = "#[redacted]";
      return x.origin === location.origin ? x.pathname + x.search + x.hash : x.href;
    } catch { return ""; }
  }
  const SKIP = new Set(["script", "style", "template", "noscript", "head", "meta", "link", "title"]);
  const ACT = new Set(["button", "link", "textbox", "searchbox", "checkbox", "radio", "combobox", "listbox", "slider", "spinbutton", "switch", "menuitem", "menuitemcheckbox", "menuitemradio", "tab", "option", "treeitem"]);
  const COMPACT = new Set([...ACT, "heading", "alert", "alertdialog", "dialog", "status", "iframe"]);
  const CONTENT = new Set(["button", "link", "heading", "tab", "menuitem", "menuitemcheckbox", "menuitemradio", "option", "checkbox", "radio", "switch", "treeitem", "cell", "columnheader", "rowheader", "tooltip", "alert", "status"]);
  const INPUT = { button: "button", submit: "button", reset: "button", image: "button", file: "button", color: "button", checkbox: "checkbox", radio: "radio", range: "slider", number: "spinbutton", search: "searchbox", hidden: "" };
  const TAGS = { button: "button", summary: "button", textarea: "textbox", dialog: "dialog", nav: "navigation", main: "main", aside: "complementary", form: "form", ul: "list", ol: "list", menu: "list", li: "listitem", table: "table", tr: "row", td: "cell", th: "columnheader", p: "paragraph", img: "img", iframe: "iframe", progress: "progressbar", meter: "meter", hr: "separator", fieldset: "group", details: "group", option: "option", output: "status", article: "article", figure: "figure", blockquote: "blockquote", h1: "heading", h2: "heading", h3: "heading", h4: "heading", h5: "heading", h6: "heading" };
  const BLOCK = /^(address|article|aside|blockquote|br|dd|div|dl|dt|fieldset|figcaption|figure|footer|form|h[1-6]|header|hr|li|main|nav|ol|p|pre|section|table|tbody|td|th|thead|tfoot|tr|ul)$/;
  const FOCUSABLE = "a[href],button,input,select,textarea,summary,iframe,[tabindex],[contenteditable]:not([contenteditable=false])";
  const rendered = (x) => (checkVis ? checkVis(x) : styleOf(x).display !== "none") || styleOf(x).display === "contents";
  const shown = (x) => (checkVis ? checkVis(x, { visibilityProperty: true, checkVisibilityCSS: true }) : styleOf(x).visibility === "visible");
  const ignored = (x) => attr(x, "aria-hidden") === "true" || attr(x, "inert") !== null;
  const typeOf = (x) => (attr(x, "type") || "").toLowerCase();
  const isEditable = (x) => { try { return !!editable(x); } catch { return false; } };
  const disabled = (x) => x.disabled === true || attr(x, "aria-disabled") === "true";
  function roleOf(x, t) {
    const r = (attr(x, "role") || "").trim().split(/\s+/)[0];
    if (r) return r === "presentation" || r === "none" || r === "generic" ? "" : r;
    if (t === "a" || t === "area") return attr(x, "href") !== null ? "link" : "";
    if (t === "input") { const y = INPUT[typeOf(x)]; return typeof y === "string" ? y : x.list ? "combobox" : "textbox"; }
    if (t === "select") return x.multiple || x.size > 1 ? "listbox" : "combobox";
    if (t === "header" || t === "footer") return x.closest("article,aside,main,nav,section") ? "" : t === "header" ? "banner" : "contentinfo";
    if (t === "section") return attr(x, "aria-label") || attr(x, "aria-labelledby") ? "region" : "";
    if (t === "img") return attr(x, "alt") === "" ? "" : "img";
    if (t === "option") return x.closest("select,datalist") ? "" : "option";
    if (typeof TAGS[t] === "string") return TAGS[t];
    return el1(x) && x.namespaceURI === "http://www.w3.org/1999/xhtml" && isEditable(x) && !(pe(x) && isEditable(pe(x))) ? "textbox" : "";
  }
  const caseOf = (s, p) => { const t = el1(p) ? styleOf(p).textTransform : ""; return t === "uppercase" ? s.toUpperCase() : t === "lowercase" ? s.toLowerCase() : t === "capitalize" ? s.replace(/(^|\s)(\S)/g, (m, a, b) => a + b.toUpperCase()) : s; };
  const svgName = (x) => { const a = attr(x, "aria-label"); if (a) return a; for (let c = first(x); c; c = next(c)) if (el1(c) && tagOf(c) === "title") return c.textContent; return ""; };
  // Text as rendered: open shadow roots and slots followed, hidden parts left out, image alts and SVG titles in.
  function textIn(el) {
    let o = "", n = 0;
    const walk = (x) => {
      if (++n > 3000 || o.length > 1000) return;
      const k = kind(x);
      if (k === 3) { o += caseOf(data(x), up(x)); return; }
      if (k === 11) { for (let c = first(x); c; c = next(c)) walk(c); return; }
      if (k !== 1) return;
      const t = tagOf(x);
      if (x !== el && (SKIP.has(t) || ignored(x) || !rendered(x))) return;
      if (t === "img" || t === "area" || (t === "input" && typeOf(x) === "image")) { o += ` ${attr(x, "alt") || attr(x, "aria-label") || attr(x, "title") || ""} `; return; }
      if (t === "svg") { o += ` ${svgName(x)} `; return; }
      if (t === "input" || t === "select" || t === "textarea") return;
      if (x !== el && attr(x, "aria-label")) { o += ` ${attr(x, "aria-label")} `; return; }
      if (t === "slot") { const a = x.assignedNodes({ flatten: true }); if (a.length) { for (const c of a) walk(c); return; } }
      const b = BLOCK.test(t);
      if (b) o += " ";
      for (let c = first(shadowOf(x) || x); c; c = next(c)) walk(c);
      if (b) o += " ";
    };
    walk(el);
    return norm(o);
  }
  const labels = (x) => (x.labels && x.labels.length ? norm([...x.labels].map(textIn).join(" ")) : "");
  const child = (x, tag) => { for (let c = first(x); c; c = next(c)) if (el1(c) && tagOf(c) === tag) return c; return null; };
  function nameOf(x, role, t) {
    const lb = attr(x, "aria-labelledby");
    if (lb) {
      const r = rootOf(x), d = x.ownerDocument;
      const s = norm(lb.split(/\s+/).map((id) => { const y = (r.getElementById && r.getElementById(id)) || d.getElementById(id); return y ? textIn(y) || y.textContent : ""; }).join(" "));
      if (s) return s;
    }
    const al = norm(attr(x, "aria-label"));
    if (al) return al;
    if (t === "input" || t === "textarea" || t === "select") {
      const y = typeOf(x);
      if (t === "input" && /^(button|submit|reset)$/.test(y)) return norm(val.input[0](x)) || (y === "submit" ? "Submit" : y === "reset" ? "Reset" : "");
      if (y === "image") return norm(attr(x, "alt")) || "Submit";
      return labels(x) || norm(attr(x, "title")) || norm(attr(x, "placeholder"));
    }
    if (t === "img" || t === "area") return norm(attr(x, "alt")) || norm(attr(x, "title"));
    if (t === "svg") return norm(svgName(x));
    const cap = { fieldset: "legend", table: "caption", figure: "figcaption" }[t];
    if (cap && child(x, cap)) return textIn(child(x, cap));
    if (t === "iframe") return norm(attr(x, "title") || attr(x, "name"));
    if (CONTENT.has(role)) { const s = textIn(x); if (s) return s; }
    return labels(x) || norm(attr(x, "title")) || (role === "textbox" ? norm(attr(x, "aria-placeholder") || attr(x, "data-placeholder")) : "");
  }
  // A field whose value is never shown: a password, or a field whose name, id, label or autocomplete says secret, or whose value looks like a key.
  function secretOf(x, t, name) {
    if (t !== "input" && t !== "textarea") return "";
    const ac = (attr(x, "autocomplete") || "").toLowerCase();
    if (typeOf(x) === "password" || /(current|new)-password/.test(ac)) return "password";
    const v = val[t][0](x);
    return [ac, attr(x, "name"), attr(x, "id"), name].some((s) => s && SECRET.test(s)) || (v && SECRET_VALUE.test(v)) ? "secret" : "";
  }
  function valueOf(x, t, role) {
    if (t === "input" || t === "textarea") return /^(button|checkbox|radio)$/.test(role) ? "" : val[t][0](x);
    if (t === "select") return [...(x.selectedOptions || [])].map((o) => norm(o.label)).join(", ");
    if (role === "textbox" && isEditable(x)) return norm(x.textContent);
    if (/^(slider|spinbutton|progressbar|meter|scrollbar)$/.test(role)) return String(attr(x, "aria-valuetext") || attr(x, "aria-valuenow") || (x.value != null ? x.value : ""));
    return "";
  }
  function flagsOf(x, t, role, focused) {
    const f = [], a = (k) => attr(x, k), y = t === "input" ? typeOf(x) : "";
    if (y === "checkbox" || y === "radio") { if (x.indeterminate) f.push("mixed"); else if (x.checked) f.push("checked"); }
    else if (a("aria-checked") === "true") f.push("checked");
    else if (a("aria-checked") === "mixed") f.push("mixed");
    if (a("aria-pressed") === "true") f.push("pressed");
    const ex = a("aria-expanded"), p = pe(x);
    if (ex) f.push(ex === "true" ? "expanded" : "collapsed");
    else if (t === "summary" && p && tagOf(p) === "details") f.push(p.open ? "expanded" : "collapsed");
    if (a("aria-selected") === "true") f.push("selected");
    if (disabled(x)) f.push("disabled");
    if (x.required === true || a("aria-required") === "true") f.push("required");
    if ((t === "input" || t === "textarea") && x.readOnly) f.push("readonly");
    if (a("aria-invalid") === "true") f.push("invalid");
    if (role === "heading") f.push("level=" + (a("aria-level") || (/^h\d$/.test(t) ? t[1] : "2")));
    if (y === "file") f.push("file");
    if (t === "iframe" && !frameDoc(x)) f.push("not readable");
    if (x === focused) f.push("focused");
    return f;
  }
  const frameDoc = (x) => { try { const d = x.contentDocument; return d && d.documentElement ? d : null; } catch { return null; } };
  function active() {
    let a = D.activeElement;
    for (;;) {
      const s = a && a.shadowRoot && a.shadowRoot.activeElement;
      if (s) { a = s; continue; }
      const d = a && tagOf(a) === "iframe" && frameDoc(a);
      if (d && d.activeElement && d.activeElement !== d.body) { a = d.activeElement; continue; }
      return a;
    }
  }

  // ---- ids: per document, never another document's ----
  const DOC = now().toString(36) + Math.random().toString(36).slice(2);
  const ids = new Wm(), els = new Mp(); let lastId = 0;
  const idOf = (x) => {
    let i = ids.get(x);
    if (!i) {
      ids.set(x, (i = ++lastId));
      els.set(i, new Wr(x));
      if (els.size % 20000 === 0) for (const [k, r] of els) if (!r.deref()) els.delete(k);
    }
    return i;
  };
  const lookup = (i) => { const r = els.get(i), x = r && r.deref(); return x && live(x) ? x : null; };
  const describe = (x) => { const t = tagOf(x), role = roleOf(x, t) || t; return { id: idOf(x), role, name: cut(nameOf(x, role, t), 120) }; };
  const GONE = "stale ref: that element is no longer on the page; take a new snapshot";

  function snapshot(a) {
    const full = a.mode === "full", nodes = [], focused = active();
    let budget = (a.max || 8000) * 1.2 + 1000, more = 0;
    // Once the budget is spent, elements are only counted: no names or values are worked out for them.
    const room = () => budget >= 0 || (more++, false);
    const emit = (n) => {
      budget -= 24 + n.depth * 2 + n.role.length + n.name.length + (n.value || "").length + (n.href || "").length + (n.flags ? n.flags.join("").length + 3 * n.flags.length : 0);
      nodes.push(n);
    };
    const nodeOf = (x, t, role, depth) => {
      const n = { id: idOf(x), depth, role, name: cut(nameOf(x, role, t), 200) }, f = flagsOf(x, t, role, focused), s = secretOf(x, t, n.name);
      if (s) f.push(s);
      else { const v = valueOf(x, t, role); if (v) n.value = cut(v, 100); }
      if (f.length) n.flags = f;
      if (role === "link" && attr(x, "href")) n.href = cut(scrub(attr(x, "href")), 200);
      return n;
    };
    const visit = (x, d, named) => {
      const k = kind(x);
      if (k === 3) {
        if (full && !named) { const s = norm(data(x)), p = up(x); if (s && (!el1(p) || shown(p)) && room()) emit({ id: 0, depth: d, role: "text", name: cut(s, 200) }); }
        return;
      }
      if (k === 11) { for (let c = first(x); c; c = next(c)) visit(c, d, named); return; }
      if (k !== 1) return;
      const t = tagOf(x);
      if (SKIP.has(t) || ignored(x) || !rendered(x)) return;
      const role = roleOf(x, t);
      let e = d;
      if (role && (full || COMPACT.has(role)) && shown(x)) {
        const r = box(x);
        if (r.width || r.height || ACT.has(role)) {
          if (room()) emit(nodeOf(x, t, role, d));
          e = d + 1;
          if (CONTENT.has(role)) named = true;
        }
      }
      if (t === "select") {
        if (full) for (const o of x.options) if (room()) emit({ id: idOf(o), depth: e, role: "option", name: cut(norm(o.label), 200), ...(o.selected ? { flags: ["selected"] } : {}) });
        return;
      }
      if (t === "svg" || t === "input" || t === "textarea") return;
      if (t === "iframe" || t === "frame") { const fd = frameDoc(x); if (fd) visit(fd.documentElement, e, named); return; }
      if (t === "slot") { const as = x.assignedNodes({ flatten: true }); if (as.length) { for (const c of as) visit(c, e, named); return; } }
      for (let c = first(shadowOf(x) || x); c; c = next(c)) visit(c, e, named);
    };
    if (a.root) { const r = lookup(a.root); if (!r) return fail(GONE); visit(r, 0, false); }
    else visit(D.documentElement, 0, false);
    return ok({ doc: DOC, url: location.href, title: D.title, nodes, more });
  }

  // ---- acting: synthetic events, as a person's would arrive ----
  let mo = null, quietSince = 0, moStop = 0;
  const watch = () => {
    quietSince = now();
    if (!mo) { mo = new MO(() => { quietSince = now(); }); mo.observe(D, { subtree: true, childList: true, attributes: true, characterData: true }); }
    unlater(moStop);
    moStop = later(() => { if (mo) mo.disconnect(); mo = null; }, 4000);
  };
  const mods = (a) => { const m = a.modifiers || []; return { ctrlKey: m.includes("Control"), shiftKey: m.includes("Shift"), altKey: m.includes("Alt"), metaKey: m.includes("Meta") }; };
  const mouse = (x, type, p, button, m, detail) => fire(x, new (type.startsWith("pointer") ? PE : ME)(type, { bubbles: true, cancelable: true, composed: true, clientX: p.x, clientY: p.y, button, buttons: type.endsWith("down") ? (button === 2 ? 2 : 1) : 0, detail, pointerId: 1, pointerType: "mouse", isPrimary: true, view: W, ...m }));
  const input = (x, inputType, d) => fire(x, inputType ? new IE("input", { bubbles: true, composed: true, inputType, data: d == null ? null : d }) : new Ev("input", { bubbles: true, composed: true }));
  const change = (x) => fire(x, new Ev("change", { bubbles: true }));
  const focusIn = (x) => {
    while (x && !(el1(x) && matches(x, FOCUSABLE))) x = up(x) || x.host;
    if (!x) return;
    try { focusEl(x, { preventScroll: true }); } catch { try { x.focus(); } catch {} }
  };
  const within = (hit, x) => {
    for (let y = hit; y; y = up(y) || y.host) if (y === x) return true;
    const lab = hit.closest && hit.closest("label");
    return !!lab && lab.control === x;
  };
  const textual = (x) => { const t = tagOf(x); return t === "textarea" || (t === "input" && !/^(checkbox|radio|button|submit|reset|image|file|hidden|range|color)$/.test(typeOf(x))); };
  const CODES = { Enter: 13, Tab: 9, Escape: 27, Backspace: 8, Delete: 46, ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Home: 36, End: 35, PageUp: 33, PageDown: 34, " ": 32 };
  const key = (x, type, k, m) => {
    const kc = CODES[k] || (/^F\d+$/.test(k) ? 111 + +k.slice(1) : k.toUpperCase().charCodeAt(0));
    const code = k === " " ? "Space" : k.length > 1 ? k : /[a-z]/i.test(k) ? "Key" + k.toUpperCase() : /\d/.test(k) ? "Digit" + k : "";
    return fire(x, new KE(type, { key: k, code, keyCode: kc, which: kc, charCode: type === "keypress" ? (k === "Enter" ? 13 : k.charCodeAt(0)) : 0, bubbles: true, cancelable: true, composed: true, view: W, ...m }));
  };
  const scroller = (x) => {
    for (let y = x; el1(y); y = pe(y)) { const s = styleOf(y); if (/(auto|scroll)/.test(s.overflowY) && y.scrollHeight > y.clientHeight) return y; }
    return (x.ownerDocument || D).scrollingElement || D.documentElement;
  };
  function tab(x, back) {
    const d = x.ownerDocument || D;
    const all = [...d.querySelectorAll(FOCUSABLE)].filter((y) => y.tabIndex >= 0 && !y.disabled && rendered(y) && shown(y));
    const order = [...all.filter((y) => y.tabIndex > 0).sort((p, q) => p.tabIndex - q.tabIndex), ...all.filter((y) => y.tabIndex === 0)];
    if (!order.length) return;
    const i = order.indexOf(x), to = order[i < 0 ? (back ? order.length - 1 : 0) : (i + (back ? -1 : 1) + order.length) % order.length];
    focusIn(to);
    if (textual(to)) try { to.select(); } catch {}
  }
  // What the browser itself does with a key in a text field: insert, delete, move the caret, or submit the form.
  function edit(x, k) {
    const t = tagOf(x), [g, s] = val[t === "textarea" ? "textarea" : "input"], v = g(x);
    let a = x.selectionStart, b = x.selectionEnd;
    if (a == null) a = b = v.length;
    const caret = (p) => { try { x.setSelectionRange(p, p); } catch {} };
    if (k === "Enter" && t !== "textarea") { if (x.form) (submitForm ? submitForm(x.form) : x.form.submit()); return; }
    if (k === "Backspace" || k === "Delete") {
      if (a === b) { if (k === "Backspace" && a > 0) a--; else if (k === "Delete" && b < v.length) b++; }
      if (a === b) return;
      s(x, v.slice(0, a) + v.slice(b)); caret(a);
      input(x, k === "Backspace" ? "deleteContentBackward" : "deleteContentForward");
      return;
    }
    if (/^(ArrowLeft|ArrowRight|Home|End)$/.test(k)) { caret(k === "Home" ? 0 : k === "End" ? v.length : Math.max(0, min(v.length, a + (k === "ArrowLeft" ? -1 : 1)))); return; }
    const ins = k === "Enter" ? "\n" : k.length === 1 ? k : "";
    if (!ins) return;
    s(x, v.slice(0, a) + ins + v.slice(b)); caret(a + 1);
    input(x, ins === "\n" ? "insertLineBreak" : "insertText", ins);
  }
  function defaults(x, k, m) {
    const t = tagOf(x), d = x.ownerDocument || D;
    if (m.ctrlKey || m.metaKey || m.altKey) { if ((m.ctrlKey || m.metaKey) && k.toLowerCase() === "a" && textual(x)) x.select(); return; }
    if (k === "Tab") return tab(x, m.shiftKey);
    if (k === "Escape") { const dl = [...d.querySelectorAll("dialog[open]")].pop(); if (dl && fire(dl, new Ev("cancel", { cancelable: true }))) dl.close(); return; }
    if (textual(x)) return edit(x, k);
    if (isEditable(x)) { const c = k.length === 1 ? ["insertText", k] : { Enter: ["insertParagraph"], Backspace: ["delete"], Delete: ["forwardDelete"] }[k]; if (c) exec(d, c[0], false, c[1]); return; }
    if (t === "select" && /^Arrow(Up|Down)$/.test(k)) { const i = x.selectedIndex + (k === "ArrowDown" ? 1 : -1); if (i >= 0 && i < x.options.length) { x.selectedIndex = i; input(x); change(x); } return; }
    if ((k === "Enter" && matches(x, "a[href],button,summary,[role=button],[role=link],[role=menuitem],[role=tab]")) || (k === " " && matches(x, "button,summary,input,[role=button],[role=checkbox],[role=switch],[role=radio],[role=tab]"))) {
      fire(x, new ME("click", { bubbles: true, cancelable: true, composed: true, view: W }));
      return;
    }
    const sc = scroller(x), page = W.innerHeight * 0.9;
    if (k === "Home") sc.scrollTop = 0;
    else if (k === "End") sc.scrollTop = sc.scrollHeight;
    else if (k === "ArrowLeft" || k === "ArrowRight") sc.scrollLeft += k === "ArrowLeft" ? -40 : 40;
    else { const by = { ArrowDown: 40, ArrowUp: -40, PageDown: page, PageUp: -page, " ": page }[k]; if (by) sc.scrollTop += m.shiftKey && k === " " ? -by : by; }
  }
  const pressKey = (x, k, m) => {
    const go = key(x, "keydown", k, m);
    if (go && (k.length === 1 || k === "Enter") && !m.ctrlKey && !m.metaKey && !m.altKey) key(x, "keypress", k, m);
    if (go) defaults(x, k, m);
    key(x, "keyup", k, m);
  };
  const OPS = {
    click(x, a) {
      if (disabled(x)) return fail("the element is disabled");
      if (tagOf(x) === "input" && typeOf(x) === "file") return fail("file inputs cannot be used by agents");
      scrollIn(x, { block: "center", inline: "center", behavior: "instant" });
      const r = box(x);
      if (!r.width && !r.height) return fail("the element has no size on the page (hidden or collapsed); take a new snapshot");
      const p = { x: r.left + r.width / 2, y: r.top + r.height / 2 }, root = rootOf(x);
      const hit = root.elementFromPoint ? root.elementFromPoint(p.x, p.y) : null;
      if (hit && !within(hit, x)) { const h = describe(hit); return fail(`the element is covered at its centre by ${h.role}${h.name ? ` "${h.name}"` : ""}; close or move what covers it, or click that instead`); }
      const target = hit || x, m = mods(a), right = a.button === "right";
      const press = (detail) => {
        mouse(target, "pointerdown", p, right ? 2 : 0, m, detail);
        if (mouse(target, "mousedown", p, right ? 2 : 0, m, detail)) focusIn(target);
        mouse(target, "pointerup", p, right ? 2 : 0, m, detail);
        mouse(target, "mouseup", p, right ? 2 : 0, m, detail);
      };
      press(1);
      if (right) mouse(target, "contextmenu", p, 2, m, 1);
      else {
        mouse(target, "click", p, 0, m, 1);
        if (a.double) { press(2); mouse(target, "click", p, 0, m, 2); mouse(target, "dblclick", p, 0, m, 2); }
      }
      return ok(describe(x));
    },
    type(x, a) {
      const t = tagOf(x), y = typeOf(x), text = String(a.text == null ? "" : a.text);
      if (disabled(x)) return fail("the element is disabled");
      if (t === "select") {
        const want = norm(text).toLowerCase(), opts = [...x.options], lab = (o) => norm(o.label).toLowerCase();
        const o = opts.find((o) => lab(o) === want) || opts.find((o) => o.value === text) || opts.find((o) => want && lab(o).includes(want));
        if (!o) return fail(`no option of this list matches; its options are ${opts.slice(0, 10).map((o) => json(cut(norm(o.label), 30))).join(", ")}${opts.length > 10 ? ", …" : ""}`);
        if (o.disabled) return fail("the matching option is disabled");
        focusIn(x); o.selected = true; input(x); change(x);
        return ok({ ...describe(x), picked: true });
      }
      if (t === "input" || t === "textarea") {
        if (t === "input" && /^(checkbox|radio|button|submit|reset|image|file|hidden)$/.test(y)) return fail(y === "file" ? "file inputs cannot be filled by agents" : `this is a ${roleOf(x, t)}: click it instead of typing`);
        if (x.readOnly) return fail("the field is read-only");
        focusIn(x);
        const [g, s] = val[t], old = g(x), v = a.clear === false ? old + text : text;
        s(x, v);
        // A field that refuses the text (a number or date field) is put back as it was, with no events.
        if (v && !g(x)) { s(x, old); return fail(`the field did not take the text (it is a ${y || "text"} field)`); }
        input(x, "insertText", text); change(x);
      } else if (isEditable(x)) {
        focusIn(x);
        const d = x.ownerDocument, sel = d.getSelection(), rg = d.createRange();
        rg.selectNodeContents(x);
        if (a.clear === false) rg.collapse(false);
        sel.removeAllRanges(); sel.addRange(rg);
        if (!(text ? exec(d, "insertText", false, text) : exec(d, "delete"))) { x.textContent = a.clear === false ? x.textContent + text : text; input(x, "insertText", text); }
      } else return fail("this element does not take text: type into a textbox, a searchbox, a combobox or an editable area");
      if (a.submit) pressKey(x, "Enter", {});
      return ok(describe(x));
    },
    press(x, a) {
      const m = mods(a), k = a.key === "Space" ? " " : String(a.key || "");
      if (x) { if (disabled(x)) return fail("the element is disabled"); focusIn(x); }
      const target = x || active() || D.body;
      pressKey(target, k, m);
      return ok(el1(target) && target !== D.body ? describe(target) : {});
    },
  };
  function act(a) {
    if (a.doc !== DOC) return fail("stale ref: the page changed; take a new snapshot");
    const x = a.id ? lookup(a.id) : null;
    if (a.id && !x) return fail(GONE);
    if (!x && a.op !== "press") return fail("missing ref");
    if (person && now() - person < (+a.quiet || 0)) return fail("The user is using this pane; agents wait 10 s after the user's own input. Try again shortly.");
    // Dialogs the action opens are answered for 3 s at most (Kelpie ends it sooner, and so does the person's input).
    dlg.mode = a.dialog === "accept" ? "accept" : "dismiss";
    dlg.until = now() + 3000;
    watch();
    return OPS[a.op] ? OPS[a.op](x, a) : fail("unknown action");
  }

  // ---- looking ----
  function check(c) {
    const r = { url: location.href, title: D.title, ready: D.readyState, met: false };
    const body = () => norm(D.body ? innerText(D.body) : "");
    if (c.text != null) r.met = body().includes(norm(c.text));
    else if (c.text_gone != null) r.met = !body().includes(norm(c.text_gone));
    else if (c.selector != null) {
      let all;
      try { all = D.querySelectorAll(c.selector); } catch { return fail("`selector` is not a valid CSS selector"); }
      r.met = [...all].slice(0, 500).some((x) => rendered(x) && shown(x));
    } else if (c.url_contains != null) r.met = location.href.includes(c.url_contains);
    else r.met = D.readyState === "complete";
    return ok(r);
  }
  function readConsole(a) {
    const since = +a.since || 0, lv = a.level || "all", max = min(+a.max || 50, 300);
    const pick = ring.filter((e) => e.n > since && (lv === "all" || e.level === "error" || (lv === "warning" && e.level === "warning")));
    const list = pick.slice(0, max), more = pick.length - list.length;
    return ok({
      entries: list.map((e) => ({ level: e.level, time: new Dt(e.t).toTimeString().slice(0, 8), text: e.text, src: e.src })),
      next: more ? list[list.length - 1].n : Math.max(since, seq),
      more,
      dropped: ring.length && since < ring[0].n - 1 ? lost : 0,
      sinceAgent: !AGENT_PANE,
    });
  }
  const info = (a) => ok({ doc: DOC, url: location.href, title: D.title, ready: D.readyState, person: person ? now() - person : -1, quiet: mo ? now() - quietSince : -1, dialogs: a.take ? dlg.seen.splice(0) : [] });
  // The value of an agent's expression as JSON (Kelpie runs the expression itself and hands the value, or what it threw, here).
  function value(v, thrown) {
    try {
      if (thrown) return fail(`the expression threw: ${str(v)}`);
      if (v && typeof v.then === "function") return fail("the value is a promise: give an expression whose value is ready at once");
      if (v === undefined) return ok("undefined");
      const seen = new Ws();
      const s = json(v, (k, x) => (typeof x === "bigint" ? `${x}n` : typeof x === "function" ? "[function]" : typeof x === "symbol" ? String(x) : x && typeof x === "object" ? (typeof x.nodeType === "number" && typeof x.nodeName === "string" ? `[${String(x.nodeName).toLowerCase()}]` : seen.has(x) ? "[circular]" : (seen.add(x), x)) : x));
      return ok(s === undefined ? "undefined" : s);
    } catch (e) { return fail(e); }
  }
  // Every entry point does nothing without the nonce Kelpie put in this script; the first call starts the console and dialog hooks.
  const wrap = (f) => (k, a) => {
    if (k !== NONCE) return null;
    try { hook(); return f(a || {}); } catch (e) { return fail(e); }
  };
  O.defineProperty(W, "__kelpieAgent", {
    value: O.freeze({
      snapshot: wrap(snapshot), act: wrap(act), check: wrap(check), console: wrap(readConsole), info: wrap(info),
      value: (k, v, thrown) => (k === NONCE ? value(v, thrown) : null),
      go: wrap((a) => { (a.delta < 0 ? back : forward)(hist); return ok(true); }),
      // The window around Kelpie's own trusted input in which input does not count as the person's.
      busy: wrap((a) => { busyTill = now() + min(+a.ms || 0, 10000); return ok(true); }),
      dialogs: wrap(() => { dlg.mode = ""; dlg.until = 0; return ok(true); }),
    }),
    configurable: false, enumerable: false, writable: false,
  });
})();
