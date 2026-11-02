//! The page a view is drawn in: an empty root, Dioxus's DOM-edit
//! interpreter over `window.__aioWire`, and aio's own page scripts (keys,
//! focus, dialogs, typing, the render probe, the dev overlay). The desktop
//! window (`aio-shell`) and the UI tests (`aio::testing::TestUi`) load the
//! same body, so a test meets the page a person meets.

/// What `html` (a whole page) may run and load (pure): its own inline
/// scripts, each by its SHA-256, and the scripts the page builds itself
/// ([`RUN`]: `blob:` URLs, which only a running script can make), and
/// WebAssembly (`'wasm-unsafe-eval'`: wasm only, never JS, as aio's
/// `script-src`); no `eval`, no other inline script, no `<base>`, no form
/// leaving the page;
/// any image source; `connect`: what it may connect to (`'none'`); the
/// sandboxed islands' boot script ([`crate::ISLAND_SANDBOX`]) by its hash.
/// Injected markup (`onerror=`, a `<script>` in an article) runs nothing.
pub fn policy(html: &str, connect: &str) -> String {
    use base64::Engine;
    let hashes: String = inline_scripts(html)
        .chain([crate::ISLAND_SANDBOX])
        .map(|s| {
            let d = base64::engine::general_purpose::STANDARD.encode(crate::sha256::Sha256::digest(s));
            format!("'sha256-{d}' ")
        })
        .collect();
    format!(
        "default-src 'none'; script-src {hashes}blob: 'wasm-unsafe-eval'; style-src 'unsafe-inline'; img-src * data: blob: aio:; \
         media-src * data: blob:; font-src data:; connect-src {connect}; base-uri 'none'; form-action 'none'"
    )
}

/// The policy of the browser's page (`ax build --web`): as [`policy`],
/// but its scripts are files of its own (`'self'`, no inline one), its
/// wasm may compile (`'wasm-unsafe-eval'`: wasm only, never JS), and it
/// links to its own server and installs its worker and manifest.
pub const WEB: &str = "default-src 'none'; script-src 'self' blob: 'wasm-unsafe-eval' \
                       'sha256-ByaE8OhSuVlfioCefIU3HhqNEy72dttF7hFp6HiJz7o='; style-src 'self' 'unsafe-inline'; \
                       img-src * data: blob:; media-src * data: blob:; font-src 'self' data:; connect-src 'self'; \
                       manifest-src 'self'; worker-src 'self'; base-uri 'none'; form-action 'none'";

/// `html` (a whole page) with its [`policy`] first in its `<head>`, after
/// its charset (pure).
pub fn sealed(html: &str, connect: &str) -> String {
    let csp = policy(html, connect).replace('"', "&quot;");
    let at = ["<meta charset=\"utf-8\">", "<head>"].into_iter().find(|t| html.contains(t)).unwrap_or("<head>");
    html.replacen(at, &format!(r#"{at}<meta http-equiv="Content-Security-Policy" content="{csp}">"#), 1)
}

/// The text of each inline `<script>` (none with a `src`) in `html`.
fn inline_scripts(html: &str) -> impl Iterator<Item = &str> {
    html.split("<script").skip(1).filter_map(|rest| {
        let (tag, body) = rest.split_once('>')?;
        let text = body.split_once("</script>")?.0;
        (!tag.contains("src=")).then_some(text)
    })
}

/// The page runs code from the app (a Dioxus query: a focus, a rect, an
/// island's script) as a script of its own making, never through `eval`:
/// a `blob:` script, which the [`policy`] allows and injected markup can't
/// make. It runs as a task of its own, after the frame that asked, and
/// in the order asked (a script the page adds runs whenever it has loaded,
/// unless it is not `async`: an island's unmount could run after the mount
/// that replaced it);
/// `__aioRunning` counts the ones not run yet and `__aioRan` says one ran
/// (a test waits for them, and for the frame that shows what they did).
pub const RUN: &str = "window.__aioRunning = 0;
window.__aioRan = false;
window.__aioRun = (code) => {
  const u = URL.createObjectURL(new Blob([code], { type: 'text/javascript' }));
  const s = document.createElement('script');
  window.__aioRunning++;
  const done = () => { window.__aioRunning--; window.__aioRan = true; URL.revokeObjectURL(u); s.remove(); };
  s.async = false;
  s.src = u;
  s.onload = done;
  s.onerror = () => { done(); console.error('[aio] a script from the app did not run: the page policy refused it'); };
  document.head.appendChild(s);
};";

#[cfg(any(feature = "kit", feature = "web"))]
/// Dioxus's interpreter runs a query with `Function` (`unsafe-eval`).
const QUERY_EVAL: &str = "Function(\"Eval\", `\"use strict\";${event.data};`)();";

#[cfg(any(feature = "kit", feature = "web"))]
/// The page object a query's script talks through (`send`, `recv`); `aio`
/// is its name for apps (`aio::ui::eval`), the renderer's own stays too.
const QUERY_PAGE: &str = "let dioxus = {";

#[cfg(any(feature = "kit", feature = "web"))]
/// Where a query's script starts: its `recv` is put there in place of the
/// renderer's, whose `while (true)` held the page until a message came,
/// one the page could then never take in (a `recv` waiting longer than
/// 50 ms hung the window), and which skipped a falsy one (`0`, `false`).
const QUERY_START: &str = "let _request_id = ";
#[cfg(any(feature = "kit", feature = "web"))]
const RECV: &str =
    "aio.recv = () => new Promise((r) => { const t = () => _message_queue.length ? r(_message_queue.shift()) : setTimeout(t, 4); t(); }); ";

/// Dioxus's DOM-edit interpreter, its queries run by [`RUN`], each with
/// its page object also named `aio`.
#[cfg(any(feature = "kit", feature = "web"))]
fn interpreter() -> String {
    let glue = dioxus_liveview::interpreter_glue("aio://wire");
    assert!(glue.contains(QUERY_EVAL), "dioxus-liveview's query runner moved: see aio::page::QUERY_EVAL");
    glue.replace(
        QUERY_EVAL,
        &format!(
            "window.__aioRun(`\"use strict\";${{event.data.replace('{QUERY_PAGE}', 'let aio, dioxus = aio = {{').replace('{QUERY_START}', '{RECV}{QUERY_START}')}};`);"
        ),
    )
}

/// The page's `<body>` content: the root, `bridge` (a script defining
/// `window.__aioWire` where no preload does; may be empty), the render
/// probe, the interpreter and the page scripts, and in a dev build the
/// overlay. Put it in a page with [`sealed`].
#[cfg(any(feature = "kit", feature = "web"))]
pub fn body(bridge: &str) -> String {
    let bridge = if bridge.is_empty() { String::new() } else { format!("<script>{bridge}</script>") };
    format!(
        r#"<div id="main"></div>{bridge}<script>{}{RENDER}</script><script>{RUN}</script><script>{WIRE}</script>{}<script>{TYPING}</script><script>{SET_FOCUS}</script><script>{KEYS}</script><script>{REVEAL}</script><script>{WHEEL}</script><script>{FOCUS}</script><script>{DRAG}</script><script>{DIALOG}</script><script>{PREVENT}</script><script>{ENTER}</script><script>{FILE_INPUT}</script>{}"#,
        render_probe(&crate::vitals::get()),
        interpreter(),
        overlay(cfg!(debug_assertions))
    )
}

/// The page's `<body>` on air: the root, `bridge`, the render probe,
/// air's interpreter (`air::PAGE_JS`: it keeps typed text itself, and
/// talks to `window.__aioWire` with no socket) and the page scripts that
/// need no renderer's internals.
#[cfg(all(feature = "air", not(any(feature = "kit", feature = "web"))))]
pub fn body(bridge: &str) -> String {
    let bridge = if bridge.is_empty() { String::new() } else { format!("<script>{bridge}</script>") };
    format!(
        r#"<div id="main"></div>{bridge}<script>{}{RENDER}</script><script>{RUN}</script><script>{}</script><script>{KEYS}</script><script>{REVEAL}</script><script>{WHEEL}</script><script>{FOCUS}</script><script>{DRAG}</script><script>{DIALOG}</script><script>{PREVENT}</script><script>{ENTER}</script>{}"#,
        render_probe(&crate::vitals::get()),
        air::PAGE_JS,
        overlay(cfg!(debug_assertions))
    )
}

/// `MountedData::set_focus` reads a yes back from the page, and Dioxus's
/// interpreter (0.7.10) answers nothing: the focus moved, yet the view got
/// an error. It answers yes once it is done.
pub const SET_FOCUS: &str = "(() => {
  const it = window.interpreter, set = it.setFocus.bind(it);
  it.setFocus = (id, on) => (set(id, on), true);
})();";

/// The dev overlay's script tag in a dev build, else nothing (the web
/// build's page takes it too).
pub fn overlay(dev: bool) -> String {
    if dev { format!("<script>{OVERLAY}</script>") } else { String::new() }
}


/// The page talks through the preload, not a socket: this stands in for the
/// WebSocket the DOM-edit interpreter opens.
pub const WIRE: &str = "window.WebSocket = class {
  constructor() {
    this.binaryType = 'arraybuffer';
    window.__aioWire.listen((f) => {
      const t = performance.now();
      this.onmessage && this.onmessage({ data: f.buffer.slice(f.byteOffset, f.byteOffset + f.byteLength) });
      window.__aioPainted(t);
    });
    queueMicrotask(() => this.onopen && this.onopen());
  }
  send(m) { window.__aioWire.send(typeof m === 'string' ? new TextEncoder().encode(m) : new Uint8Array(m)); }
  close() {}
};";

/// The render probe (aio vitals' render meter): how long each update from
/// the app takes to reach the screen, graded by `[vitals] render` (`T`), and
/// as the loop and transport probes do, a step up to warning or frozen is
/// said in the app's log (with `HINT`) and the way back as recovered. A
/// hidden window paints nothing, so time spent hidden is never slow, and
/// it asks for no frame (its frames wait until it shows: one per update
/// would pile up for as long as it stays hidden). One frame is asked for at
/// a time, timed from the oldest update it paints (the slowest).
pub const RENDER: &str = "window.__aioPainted = (() => {
  let worst = 0, shown = 0, asked = false;
  const names = ['healthy', 'degraded', 'warning', 'frozen'];
  const grade = (ms) => ms >= T.frozen ? 3 : ms >= T.warning ? 2 : ms >= T.degraded ? 1 : 0;
  document.addEventListener('visibilitychange', () => { if (!document.hidden) shown = performance.now(); });
  return (t) => document.hidden || asked || (asked = true, requestAnimationFrame(() => {
    asked = false;
    const ms = Math.round(performance.now() - t), g = grade(ms);
    if (document.hidden || shown > t) return;
    if (g === 0) {
      if (worst >= 2) console.info('[vitals] render recovered: an update painted in ' + ms + ' ms');
      worst = 0;
    } else if (g > worst) {
      worst = g;
      if (g >= 2) console.warn('[vitals] render ' + names[g] + ': an update took ' + ms + ' ms to paint' + HINT);
    }
  }));
})();";

/// The render probe's settings: the `[vitals] render` tier, and its hint.
pub fn render_probe(v: &crate::vitals::Vitals) -> String {
    let hint = if v.hints {
        " · likely: a large DOM update or a script in the page held it (DevTools: Performance)"
    } else {
        ""
    };
    let t = v.render;
    format!(
        "const T = {}, HINT = {};",
        serde_json::json!({ "degraded": t.degraded, "warning": t.warning, "frozen": t.frozen }),
        serde_json::json!(hint)
    )
}

/// Keys pressed with nothing focused land on `<body>` (after a click on
/// nothing, a dialog closed onto nothing); they go to the app's root
/// element instead, down and up, so app-wide shortcuts work then too (aio
/// used a window listener).
pub const KEYS: &str = "for (const type of ['keydown', 'keyup']) document.addEventListener(type, (e) => {
  const root = document.querySelector('#main > :not(style)');
  if (e.target !== document.body || !root) return;
  const copy = new KeyboardEvent(type, e);
  if (!root.dispatchEvent(copy)) e.preventDefault();
});";

/// A view's `prevent_default()` reaches the page too late (the event was
/// sent to the app and the browser moved on: a liveview has no synchronous
/// answer), so the page prevents keys itself: inside an element with
/// `data-aio-prevent="ArrowUp ArrowDown PageUp PageDown Home End Tab"` (key
/// names as `KeyboardEvent.key` says them, any modifiers), those keys don't
/// scroll or move the focus; the view still hears them. A field typed in
/// keeps its own keys, and a modal dialog inside keeps its own Tab (this
/// runs after the dialog's own Tab handling).
pub const PREVENT: &str = "document.addEventListener('keydown', (e) => {
  const t = e.target;
  if (e.defaultPrevented || e.isComposing || !(t instanceof Element)) return;
  if (t.matches('input, textarea, select') || t.isContentEditable) return;
  const at = t.closest('[data-aio-prevent],[aria-modal=\"true\"]');
  if (at && at.hasAttribute('data-aio-prevent') && at.getAttribute('data-aio-prevent').split(' ').includes(e.key)) e.preventDefault();
});";

/// An element that gets `data-aio-reveal` (or a new value of it: the
/// cursor's index) is scrolled into view, nearest edge, as its frame is
/// applied: no script from the app per step, no frame late. Put it on the
/// selected row of a list the keyboard moves through.
pub const REVEAL: &str = "new MutationObserver((ms) => {
  let el = null;
  for (const m of ms) {
    if (m.type === 'attributes') { if (m.target.hasAttribute('data-aio-reveal')) el = m.target; continue; }
    for (const n of m.addedNodes) {
      if (n.nodeType !== 1) continue;
      el = n.hasAttribute('data-aio-reveal') ? n : n.querySelector('[data-aio-reveal]') || el;
    }
  }
  if (el && el.isConnected) el.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'auto' });
}).observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-aio-reveal'] });";

/// Enter in a form's field submits it as a person expects, though the view
/// decides a frame later (its round trip) what the form allows: a form with
/// no submit button, which HTML won't submit with two fields or more, is
/// submitted (aio's key handler: `requestSubmit`); one whose submit button
/// is still disabled is submitted as soon as the view's answer to what was
/// typed enables it (within half a second, unless typing goes on), or at
/// once with `data-enter-submits` on the form (its submit checks again).
pub const ENTER: &str = "document.addEventListener('keydown', (e) => {
  const t = e.target;
  if (e.key !== 'Enter' || e.defaultPrevented || e.isComposing || e.shiftKey || e.ctrlKey || e.altKey || e.metaKey) return;
  if (!(t instanceof HTMLInputElement) || !t.form || ['button', 'submit', 'reset', 'image', 'file'].includes(t.type)) return;
  const form = t.form;
  const btn = [...form.elements].find((el) => ['submit', 'image'].includes(el.type) && (el instanceof HTMLButtonElement || el instanceof HTMLInputElement));
  if (!btn) { e.preventDefault(); form.requestSubmit(); return; }
  if (!btn.disabled) return;
  e.preventDefault();
  if (form.hasAttribute('data-enter-submits')) { form.requestSubmit(); return; }
  const done = () => { mo.disconnect(); clearTimeout(late); form.removeEventListener('keydown', done); form.removeEventListener('input', done); };
  const mo = new MutationObserver(() => { if (!btn.disabled) { done(); if (btn.isConnected) form.requestSubmit(btn); } });
  mo.observe(btn, { attributes: true, attributeFilter: ['disabled'] });
  const late = setTimeout(done, 500);
  setTimeout(() => { form.addEventListener('keydown', done); form.addEventListener('input', done); });
});";

/// Ctrl+wheel is the app's (zoom), never a page scroll (aio prevented it too).
pub const WHEEL: &str =
    "document.addEventListener('wheel', (e) => { if (e.ctrlKey) e.preventDefault(); }, { passive: false });";

/// Chromium honors `autofocus` only at page load; views add fields later
/// (a rename box), so the newest `autofocus` element is focused when it
/// appears, and its text selected too with `data-select`.
pub const FOCUS: &str = "new MutationObserver((ms) => {
  let el = null;
  for (const m of ms) for (const n of m.addedNodes) {
    if (n.nodeType !== 1) continue;
    el = n.hasAttribute('autofocus') ? n : n.querySelector('[autofocus]') || el;
  }
  if (!el) return;
  el.focus();
  if (el.hasAttribute('data-select') && el.select) el.select();
}).observe(document.body, { childList: true, subtree: true });";

/// The web page's first view takes the place of the server's paint
/// (`aio::web::launch`) with what a person did to the paint meanwhile, as
/// aio's hydration kept it by keeping the paint's nodes: the page's and each
/// box's scroll, the focus and its selection, typed text, ticks and picks
/// (then said to the view as its `input` / `change`, so its state agrees),
/// open `<details>`, and where audio and video are (playing on). It goes element by element where both trees have
/// the same tags; a part the view draws otherwise is the view's (where aio
/// threw the whole paint away and drew the page again).
pub const ADOPT: &str = "window.__aioAdopt = (paint, view) => {
  const later = [], told = [];
  let focus = null;
  const walk = (a, b) => {
    if (a.tagName !== b.tagName) return;
    const [top, left] = [a.scrollTop, a.scrollLeft];
    if (top || left) later.push(() => { b.scrollTop = top; b.scrollLeft = left; });
    if (a instanceof HTMLDetailsElement) b.open = a.open;
    if (a instanceof HTMLMediaElement && (a.currentTime || !a.paused)) {
      const [at, playing] = [a.currentTime, !a.paused];
      later.push(() => { b.currentTime = at; if (playing) b.play().catch(() => {}); });
    }
    if (a instanceof HTMLInputElement && (a.type === 'checkbox' || a.type === 'radio')) {
      if (a.checked !== a.defaultChecked) { b.checked = a.checked; told.push([b, 'change']); }
    } else if ((a instanceof HTMLInputElement && a.type !== 'file') || a instanceof HTMLTextAreaElement) {
      if (a.value !== a.defaultValue) { b.value = a.value; told.push([b, 'input']); }
    } else if (a instanceof HTMLSelectElement && b.options.length === a.options.length) {
      const picked = [...a.options].map((o) => o.selected);
      if (picked.some((s, i) => s !== a.options[i].defaultSelected)) {
        picked.forEach((s, i) => { b.options[i].selected = s; });
        told.push([b, 'change']);
      }
    }
    if (document.activeElement === a) {
      let range = null;
      try { range = a.selectionStart === null ? null : [a.selectionStart, a.selectionEnd, a.selectionDirection]; } catch {}
      focus = [b, range];
    }
    if (a.children.length === b.children.length) for (let i = 0; i < a.children.length; i++) walk(a.children[i], b.children[i]);
  };
  walk(paint, view);
  const [x, y] = [scrollX, scrollY];
  paint.remove();
  view.id = 'main';
  view.removeAttribute('hidden');
  scrollTo(x, y);
  for (const f of later) f();
  if (focus) {
    focus[0].focus({ preventScroll: true });
    if (focus[1]) try { focus[0].setSelectionRange(...focus[1]); } catch {}
  }
  for (const [n, t] of told) n.dispatchEvent(new Event(t, { bubbles: true }));
};";

/// What a drag carries: a view's `ondragstart` runs after the browser has
/// moved on, so its `set_data` never reaches the drop (and Dioxus 0.7.10's
/// event copy ignores it). The page sets it instead, in the dragstart
/// itself, from the dragged element's `data-drag` (as `text/plain`): any
/// drop reads it, in the app, a test or another app.
pub const DRAG: &str = "document.addEventListener('dragstart', (e) => {
  const el = e.target instanceof Element && e.target.closest('[data-drag]');
  if (el && e.dataTransfer) e.dataTransfer.setData('text/plain', el.getAttribute('data-drag'));
}, true);";

/// A file input opens the system's own picker (Electron's, Android's).
/// Dioxus's interpreter (0.7.10) catches a click on one at the window for
/// its desktop host and asks a `__file_dialog` endpoint no page here has:
/// the click was refused and no picker ever opened. Stopped at the
/// document, after the view's own handlers on `#main`, it never gets there;
/// the picked files then reach the view as its `change` (the interpreter's
/// liveview path reads them).
pub const FILE_INPUT: &str = "document.addEventListener('click', (e) => {
  if (e.target instanceof HTMLInputElement && e.target.type === 'file') e.stopPropagation();
});";

/// A modal dialog (`[aria-modal=true]`, the kit's `Modal`) holds the
/// keyboard (aio's `onModalKey`): focus on the box itself moves to its first
/// control (or back to the one inside that just had it, an `autofocus`), Tab
/// and Shift+Tab wrap inside the top dialog, and once it is gone focus goes
/// back to what had it before it opened, unless something else took it.
pub const DIALOG: &str = "(() => {
  const TABBABLE = 'a[href],area[href],button:not([disabled]),input:not([disabled]):not([type=\"hidden\"]),' +
    'select:not([disabled]),textarea:not([disabled]),iframe,[contenteditable=\"\"],[contenteditable=\"true\"],[tabindex]';
  const open = [];
  const els = (box) => [...box.querySelectorAll(TABBABLE)].filter((el) => el.getAttribute('tabindex') !== '-1' && !el.closest('[hidden],[inert]'));
  const top = () => { for (let i = open.length - 1; i >= 0; i--) if (open[i].box.isConnected) return open[i].box; return null; };
  const add = (box) => {
    if (open.some((o) => o.box === box)) return;
    const had = document.activeElement;
    open.push({ box, back: had && had !== document.body ? had : null });
    box.addEventListener('focusin', (e) => {
      if (e.target !== box) return;
      const inside = e.relatedTarget && box.contains(e.relatedTarget) ? e.relatedTarget : els(box)[0];
      if (inside) inside.focus();
    });
  };
  new MutationObserver((ms) => {
    for (const m of ms) for (const n of m.addedNodes) {
      if (n.nodeType !== 1) continue;
      if (n.getAttribute('aria-modal') === 'true') add(n);
      for (const d of n.querySelectorAll('[aria-modal=\"true\"]')) add(d);
    }
    for (let i = open.length - 1; i >= 0; i--) {
      const { box, back } = open[i];
      if (box.isConnected) continue;
      open.splice(i, 1);
      queueMicrotask(() => {
        const a = document.activeElement;
        if (a && a !== document.body && a.isConnected) return;
        if (back && back.isConnected) back.focus();
      });
    }
  }).observe(document.body, { childList: true, subtree: true });
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Tab' || e.defaultPrevented || e.isComposing) return;
    const box = top();
    if (!box) return;
    const all = els(box), a = document.activeElement, inside = a && box.contains(a);
    if (!all.length) { e.preventDefault(); box.focus(); return; }
    const first = all[0], last = all[all.length - 1];
    if (e.shiftKey && (!inside || a === first || a === box)) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && (!inside || a === last)) { e.preventDefault(); first.focus(); }
  });
})();";

/// Typing is never undone. Each keystroke goes to the app, and its echo of
/// the field's value comes back later: by then the person may have typed on,
/// and the old echo would wipe those keys (a fast typist on a phone loses
/// letters). So a field keeps the values it sent until the app's value
/// matches it again; an echo of one of those is skipped. Any other value is
/// the app's own (a clear after sending, "Max") and is applied.
pub const TYPING: &str = "(() => {
  const sent = new WeakMap();
  document.addEventListener('input', (e) => {
    const t = e.target;
    if (typeof t.value !== 'string') return;
    if (!sent.has(t)) sent.set(t, []);
    sent.get(t).push(t.value);
  }, true);
  const it = window.interpreter, set = it.setAttributeInner.bind(it);
  it.setAttributeInner = (node, field, value, ns) => {
    const past = field === 'value' && !ns ? sent.get(node) : undefined;
    if (past) {
      if (value === node.value) return void sent.delete(node);
      if (past.includes(value)) return;
      sent.delete(node);
    }
    set(node, field, value, ns);
  };
})();";

/// Dev builds only: the page's errors on the page (aio dev-overlay), where
/// the person is looking. Plain DOM (the renderer may be what broke), one
/// row per problem with a count (a per-frame failure is one problem),
/// at most 20 rows, a small corner badge that never covers the page.
/// `window.__aioDiag(title, detail, show)` adds one (`show`: the list open);
/// `window.__aioDiagDrop(title)` takes those titled so away (a fixed build).
pub const OVERLAY: &str = "(() => {
  const rows = new Map();
  let root = null, list = null, badge = null, open = false, total = 0;
  const css = (el, s) => { el.style.cssText = s; return el; };
  const ensure = () => {
    if (root) return;
    root = css(document.createElement('div'), 'position:fixed;right:8px;bottom:8px;z-index:2147483647;max-width:min(560px,90vw);pointer-events:none;font:12px/1.4 ui-monospace,monospace');
    root.setAttribute('data-aio-dev', 'overlay');
    list = css(document.createElement('div'), 'display:none;max-height:40vh;overflow:auto;background:#1b1b1f;color:#f3f3f3;border-radius:6px;padding:6px 8px;margin-bottom:4px;pointer-events:auto');
    badge = css(document.createElement('button'), 'float:right;pointer-events:auto;background:#b3261e;color:#fff;border:0;border-radius:10px;padding:2px 9px;cursor:pointer');
    badge.onclick = () => { open = !open; draw(); };
    root.appendChild(list);
    root.appendChild(badge);
    document.body.appendChild(root);
  };
  const draw = () => {
    list.textContent = '';
    for (const r of rows.values()) {
      const el = css(document.createElement('div'), 'white-space:pre-wrap');
      el.textContent = (r.count > 1 ? '×' + r.count + ' ' : '') + r.title + (r.detail ? ' — ' + r.detail : '');
      list.appendChild(el);
    }
    badge.textContent = total + (total === 1 ? ' page error' : ' page errors');
    list.style.display = open ? 'block' : 'none';
  };
  const add = (title, detail, show) => {
    title = String(title || 'error'); detail = String(detail || '');
    if (show) open = true;
    const k = title + '\\n' + detail, r = rows.get(k);
    total++;
    if (r) r.count++;
    else {
      if (rows.size >= 20) rows.delete(rows.keys().next().value);
      rows.set(k, { title, detail, count: 1 });
    }
    ensure();
    draw();
  };
  window.addEventListener('error', (e) => add(e.message, e.filename ? e.filename + ':' + e.lineno : ''));
  window.addEventListener('unhandledrejection', (e) => add('unhandled rejection', e.reason && e.reason.stack || e.reason));
  window.__aioDiag = add;
  window.__aioDiagDrop = (title) => {
    for (const [k, r] of rows) if (r.title === title) { rows.delete(k); total -= r.count; }
    if (root && !rows.size) { root.remove(); root = null; open = false; } else if (root) draw();
  };
})();";

#[cfg(test)]
mod tests {
    use super::*;

    /// The interpreter still catches file inputs at the window, and the
    /// page stops the click before it gets there (on Android, measured:
    /// "Refused to connect to …/__file_dialog" and no picker before).
    #[cfg(any(feature = "kit", feature = "web"))]
    #[test]
    fn a_file_input_opens_the_system_picker_not_the_desktop_hosts() {
        let glue = interpreter();
        assert!(glue.contains("__file_dialog") && glue.contains("window.addEventListener(\"click\""));
        assert!(body("").contains(&format!("<script>{FILE_INPUT}</script>")));
        assert!(FILE_INPUT.starts_with("document.addEventListener('click'") && !FILE_INPUT.contains(", true)"));
    }

    #[test]
    fn the_page_runs_only_its_own_scripts_and_never_eval() {
        let page = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><script src=\"x.js\"></script></head><body>{}</body></html>",
            body("")
        );
        let page = sealed(&page, "'none'");
        let csp = policy(&page, "'none'");
        let scripts = csp.split("script-src ").nth(1).unwrap().split(';').next().unwrap();
        assert!(!csp.contains("'unsafe-eval'") && !scripts.contains("unsafe-inline") && !scripts.contains('*'), "{csp}");
        assert!(scripts.ends_with("blob: 'wasm-unsafe-eval'"), "wasm compiles, as in aio: {scripts}");
        assert!(csp.contains("base-uri 'none'") && csp.contains("form-action 'none'") && csp.contains("connect-src 'none'"));
        // Every inline script by its hash, the one with a src by none.
        let n = inline_scripts(&page).count();
        assert!(n >= 10, "{n}");
        assert_eq!(scripts.matches("'sha256-").count(), n + 1, "and the sandbox's: {scripts}");
        // The policy comes right after the charset, before any script.
        let at = page.find("Content-Security-Policy").unwrap();
        assert!(page.find("<meta charset").unwrap() < at && at < page.find("<script").unwrap());
        // The interpreter runs its queries as the page's own scripts, never through Function.
        assert!(!page.contains("Function(") && page.contains("window.__aioRun("), "no eval in the page");
    }

    #[test]
    fn the_sandbox_boot_script_is_allowed_by_its_hash_in_both_policies() {
        use base64::Engine;
        let d = base64::engine::general_purpose::STANDARD.encode(crate::sha256::Sha256::digest(crate::ISLAND_SANDBOX));
        let hash = format!("'sha256-{d}'");
        assert!(WEB.contains(&hash), "page::WEB names {hash}: update it with ISLAND_SANDBOX");
        assert!(policy("<p>", "'none'").contains(&hash));
    }

    #[test]
    fn a_hash_is_the_sha256_of_the_exact_script_text() {
        // `printf 'alert(1)' | openssl dgst -sha256 -binary | base64`
        let csp = policy("<p><script>alert(1)</script>", "'none'");
        assert!(csp.contains("script-src 'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI=' 'sha256-"), "{csp}");
    }
}
