// The page's elements as a person meets them: roles, names, auto names,
// handles and paths, on the real DOM, its computed styles and live values.
// `ax surface` and `ax trigger` run it in the app's window, and
// `aio::testing::TestUi` in its test page, so a test's paths work on the
// running app. `(this)(op, arg)` answers `{ ok }` or `{ error }`.
(function (op, arg) {
  const IMPLIED = (el) => {
    const tag = el.localName, ty = el.getAttribute("type") || "";
    if (tag === "button" || (tag === "input" && ["button", "submit", "reset"].includes(ty))) return "button";
    if (tag === "a" && el.hasAttribute("href")) return "link";
    if (tag === "input" && ty === "checkbox") return "checkbox";
    if (tag === "input" && ty === "radio") return "radio";
    if (tag === "input" && ty === "range") return "slider";
    if (tag === "input" || tag === "textarea") return "textbox";
    if (tag === "select") return "combobox";
    if (/^h[1-6]$/.test(tag)) return "heading";
    if (tag === "img") return "img";
    if (tag === "dialog") return "dialog";
    return "generic";
  };
  const words = (s) => s.split(/\s+/).filter(Boolean).join(" ");
  const info = (el) => {
    const implied = IMPLIED(el);
    const role = el.getAttribute("role") || implied;
    const aria = el.getAttribute("aria-label");
    const forLabel = () => {
      const id = el.getAttribute("id");
      if (id === null) return null;
      const l = document.querySelector(`label[for="${CSS.escape(id)}"]`);
      return l ? l.textContent : null;
    };
    const wrapped = () => {
      const l = el.parentElement && el.parentElement.closest("label");
      return l ? l.textContent : null;
    };
    const own = () => {
      const fields = ["textbox", "combobox"].includes(implied);
      const t = el.textContent;
      return !fields && t.trim() ? t : null;
    };
    const name = (aria !== null && aria.trim() ? aria : null) ?? forLabel() ?? wrapped() ?? own() ??
      el.getAttribute("placeholder") ?? el.getAttribute("alt") ?? "";
    return { tag: el.localName, role, name: words(name) };
  };
  // Why a person can't see it (`display: none` on it or around it, the
  // `hidden` attribute is one; `visibility`), or null. A component shows
  // when one of its elements does.
  const hidden = (el) => {
    if (el.scope) return el.els.some((e) => !hidden(e)) ? null : "none of what it drew shows";
    for (let e = el; e; e = e.parentElement) {
      if (getComputedStyle(e).display === "none") return e === el ? "display: none" : `display: none on an enclosing <${e.localName}>`;
    }
    const v = getComputedStyle(el).visibility;
    return v === "visible" ? null : `visibility: ${v}`;
  };
  const under = (root) => Array.from(root.querySelectorAll("*"));
  const pascal = (s) => {
    const ws = s.split(/[^\p{Alphabetic}\p{N}\p{M}]+/u).filter(Boolean).slice(0, 5);
    const joined = ws.map((w) => { const [f, ...r] = Array.from(w); return f.toUpperCase() + r.join(""); }).join("");
    return Array.from(joined).slice(0, 32).join("");
  };
  const autoName = (i) => {
    let word;
    if (i.role === "generic") {
      word = { tr: "Row", li: "Item", form: "Form" }[i.tag];
      if (!word) return null;
    } else if (i.role === "textbox") word = "Input";
    else if (i.role === "combobox") word = "Select";
    else word = pascal(i.role);
    const label = Array.from(i.name).length <= 60 ? pascal(i.name) : "";
    return label.endsWith(word) ? label : label + word;
  };
  const named = (root) => {
    const seen = new Map(), out = [];
    for (const el of self ? [root, ...under(root)] : under(root)) {
      if (hidden(el)) continue;
      const i = info(el);
      if (i.role === "generic") {
        // A row, item or form is named by its own text, not its controls'.
        i.name = Array.from(el.childNodes).filter((c) => c.nodeType === 3).map((c) => c.data).join("");
      }
      const base = autoName(i);
      if (base === null) continue;
      const n = (seen.get(base) || 0) + 1;
      seen.set(base, n);
      out.push([n === 1 ? base : base + n, el]);
    }
    return out;
  };
  const top = document.documentElement;
  const handle = (t) => `[data-t="${CSS.escape(t)}"]`;
  // `text=Skip this version`: the showing elements whose visible text is
  // that (spaces folded), each the control around it when it is a
  // control's whole text (the <button>, not its <span>).
  const TEXT = "text=";
  const byText = (want) => {
    const w = words(want);
    const all = under(top).filter((el) => words(el.textContent).includes(w) && !hidden(el) && words(el.innerText) === w);
    const lift = (el) => {
      for (let e = el; e && all.includes(e); e = e.parentElement) if (info(e).role !== "generic") return e;
      return el;
    };
    return [...new Set(all.filter((el) => !all.some((o) => o !== el && el.contains(o))).map(lift))];
  };
  // `css=#cmd-0`: the elements a CSS selector matches (in `outer/css=…`,
  // inside `outer`; the selector is the rest, slashes and all); none for
  // one that is no selector.
  const CSS_ = "css=";
  const cssAt = (t) => t.startsWith(CSS_) ? 0 : t.indexOf("/" + CSS_) + 1;
  const bySelector = (t) => {
    const at = cssAt(t);
    const root = at ? find(t.slice(0, at - 1)) : top;
    if (!root) return [];
    try { return Array.from(root.querySelectorAll(t.slice(at + CSS_.length))); } catch { return []; }
  };
  // air's components (`arg.tree`, from `TestUi`: each one's name, key, the
  // page's ids of its first and last node, its parent's place in the list)
  // as scopes a path names: `comp=Row[42]` (by key), `comp=Row` (the one
  // Row there), `comp=List/comp=Row[42]/DeleteButton` (inside one).
  const COMP = "comp=";
  const tree = arg.tree || [];
  const span = (c) => {
    if (c.first == null || !window.__airNode) return [];
    const a = window.__airNode(c.first), b = window.__airNode(c.last), out = [];
    for (let n = a; n && b; n = n.nextSibling) { out.push(n); if (n === b) return out; }
    return [];
  };
  const scope = (nodes) => {
    const els = nodes.filter((n) => n.nodeType === 1);
    if (els.length === 1 && nodes.every((n) => n === els[0] || n.nodeType === 8 || !n.data.trim())) return els[0];
    return {
      scope: true, els, localName: "", getAttribute: () => null, hasAttribute: () => false, matches: () => false,
      contains: (el) => nodes.some((n) => n === el || n.contains(el)),
      querySelectorAll: (sel) => els.flatMap((e) => [...(e.matches(sel) ? [e] : []), ...e.querySelectorAll(sel)]),
      get textContent() { return nodes.map((n) => n.textContent).join(""); },
      get innerText() { return nodes.map((n) => n.nodeType === 1 ? n.innerText : n.nodeType === 3 ? n.data : "").join(""); },
    };
  };
  const label = (c) => c.key == null ? c.name : `${c.name}[${c.key}]`;
  const comps = (seg, root) => {
    const m = /^comp=([^[\]]+)(?:\[(.*)\])?$/.exec(seg);
    if (!m) return [];
    return tree.filter((c) => c.name === m[1] && (m[2] === undefined || c.key === m[2])).map(span)
      .filter((ns) => ns.length && (root === top || ns.some((n) => root.contains(n)))).map(scope);
  };
  const find = (t) => {
    const cutAt = t.lastIndexOf("/"), last = t.slice(cutAt + 1);
    if (last.startsWith(COMP) && !t.startsWith(TEXT) && !(t.startsWith(CSS_) || cssAt(t) > 0)) {
      const root = cutAt >= 0 ? find(t.slice(0, cutAt)) : top;
      const hits = root ? comps(last, root) : [];
      return hits.length === 1 ? hits[0] : null;
    }
    if (t.startsWith(TEXT)) {
      const hits = byText(t.slice(TEXT.length));
      return hits.length === 1 ? hits[0] : null;
    }
    if (t.startsWith(CSS_) || cssAt(t) > 0) {
      const hits = bySelector(t);
      return hits.length === 1 ? hits[0] : null;
    }
    const exact = document.querySelector(handle(t));
    let root = top, inner = t;
    const cut = t.lastIndexOf("/");
    if (cut >= 0 && !exact) {
      root = find(t.slice(0, cut));
      if (!root) return null;
      inner = t.slice(cut + 1);
    }
    const inside = (el) => el !== root && root.contains(el);
    if (exact && inside(exact)) return exact;
    const byHandle = Array.from(document.querySelectorAll(handle(inner))).find(inside);
    if (byHandle) return byHandle;
    const byName = named(root).find(([n]) => n === inner);
    return byName ? byName[1] : null;
  };
  const handles = () =>
    [...new Set(Array.from(document.querySelectorAll("[data-t]")).map((e) => e.getAttribute("data-t")))].sort();
  const names = () => named(top).map(([n]) => n);
  const missing = (t) => {
    const there = `the handles there are: ${handles().join(", ")}; by name: ${names().join(", ")}`;
    const seg = t.slice(t.lastIndexOf("/") + 1);
    if (seg.startsWith(COMP) && !t.startsWith(TEXT) && cssAt(t) === 0 && !t.startsWith(CSS_)) {
      if (!window.__airNode) return { error: `${t}: components are named in the page on air only` };
      const cut = t.lastIndexOf("/"), root = cut >= 0 ? find(t.slice(0, cut)) : top;
      if (!root) return missing(t.slice(0, cut));
      const n = comps(seg, root).length;
      const all = [...new Set(tree.map(label))].join(", ") || "none";
      return { error: n ? `${n} components match ${t}: name one by its key (comp=Name[key])` : `no component ${seg.slice(COMP.length)} there; the components are: ${all}` };
    }
    if (t.startsWith(CSS_) || cssAt(t) > 0) {
      const sel = t.slice(cssAt(t) + CSS_.length);
      try { document.querySelector(sel); } catch { return { error: `${JSON.stringify(sel)} is no CSS selector (in ${t})` }; }
      const n = bySelector(t).length;
      if (!n) return { error: `no element matches ${t}; ${there}` };
      return { error: `${n} elements match ${t}: narrow the selector to one (:nth-child(2), an id)` };
    }
    // A selector given as a handle: say how to give it.
    const looks = /^[#.[]|[\s>:]/.test(t) ? ` (a CSS selector? css=${t})` : "";
    if (!t.startsWith(TEXT)) return { error: `no element with data-t="${t}"${looks}; ${there}` };
    const hits = byText(t.slice(TEXT.length)), all = named(top);
    if (!hits.length) return { error: `no showing element has the text ${JSON.stringify(t.slice(TEXT.length))}; ${there}` };
    const which = hits.map((el) => el.getAttribute("data-t") || (all.find(([, e]) => e === el) || [`<${el.localName}>`])[0]);
    return { error: `${hits.length} elements show the text ${JSON.stringify(t.slice(TEXT.length))} (${which.join(", ")}): name one of them instead` };
  };
  // A string as Rust's `{:?}` writes it (TestUi's lines are written so).
  const debug = (s) => '"' + s.replace(/[\\"\n\r\t\0]|[\p{Cc}\p{Cf}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Grapheme_Extend}]/gu, (c) =>
    ({ "\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t", "\0": "\\0" })[c] ??
      `\\u{${c.codePointAt(0).toString(16)}}`) + '"';
  const cutText = (s, max) => {
    const cs = Array.from(s);
    return cs.length > max ? cs.slice(0, max).join("") + "…" : s;
  };
  const disabled = (el) => el.hasAttribute("disabled") || el.matches(":disabled");
  // `self`: the root is a path the caller named, so its own line comes first;
  // `depth`: levels below the first shown at most; `full`: names uncut.
  const surface = (root, rects, self, depth = Infinity, full = false) => {
    const lines = [], depths = new Map();
    for (const el of self ? [root, ...under(root)] : under(root)) {
      if (hidden(el)) continue;
      const i = info(el), t = el.getAttribute("data-t");
      if (t === null && i.role === "generic") continue;
      let d = 0;
      for (let a = el.parentElement; a && a !== root.parentElement; a = a.parentElement) {
        if (depths.has(a)) { d = depths.get(a) + 1; break; }
      }
      depths.set(el, d);
      if (d > depth) continue;
      let line = `${"  ".repeat(d + 1)}${i.role} ${debug(full ? i.name : cutText(i.name, 40))}`;
      if (t !== null) line += ` [${t}]`;
      if (el.localName === "input" && ["checkbox", "radio"].includes(el.type)) line += el.checked ? " checked" : " unchecked";
      if (i.role === "textbox" || i.role === "combobox") line += ` = ${debug(el.value ?? "")}`;
      for (const [a, w] of [["disabled", "disabled"], ["aria-pressed", "pressed"], ["aria-expanded", "expanded"]]) {
        const v = el.getAttribute(a);
        if (v !== null && v !== "false") line += ` ${w}`;
      }
      if (rects) {
        const r = el.getBoundingClientRect();
        line += ` @${Math.round(r.x)},${Math.round(r.y)} ${Math.round(r.width)}x${Math.round(r.height)}`;
      }
      lines.push(line);
    }
    return lines.join("\n");
  };
  // An element for a person to use: there, showing, and (to change it) enabled.
  const target = (t, verb) => {
    const el = find(t);
    if (!el) return [null, missing(t)];
    const why = hidden(el);
    if (why) return [null, { error: `data-t="${t}" can't be used: it is hidden (${why})` }];
    if (verb && disabled(el)) return [null, { error: `can't ${verb} data-t="${t}": it is disabled` }];
    return [el, null];
  };
  const box = (el) => {
    el.scrollIntoView({ block: "center", inline: "center" });
    const r = el.getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2, w: r.width, h: r.height };
  };
  const fire = (el, ...types) => types.forEach((ty) => el.dispatchEvent(new Event(ty, { bubbles: true })));
  const checkable = (el) =>
    (el.localName === "input" && ["checkbox", "radio"].includes(el.type)) ||
    ["checkbox", "switch", "radio", "menuitemcheckbox"].includes(el.getAttribute("role"));
  const isChecked = (el) => el.localName === "input" ? el.checked : el.getAttribute("aria-checked") === "true";

  if (op === "surface") {
    const root = arg.path ? find(arg.path) : top;
    if (!root) return missing(arg.path);
    const why = arg.path && hidden(root);
    if (why) return { error: `data-t="${arg.path}" is there but hidden (${why}): nothing of it shows` };
    const depth = arg.depth ?? Infinity, full = !!arg.full;
    // A component: each instance (handle `c`, or `c-…`) under the root, with what is under it.
    if (arg.component) {
      const c = arg.component, of = (el) => { const t = el.getAttribute("data-t"); return t === c || t.startsWith(c + "-"); };
      const all = [...root.querySelectorAll("[data-t]")].filter((el) => of(el) && !hidden(el));
      const outer = all.filter((el) => !all.some((o) => o !== el && o.contains(el)));
      if (!outer.length) return { error: `no showing element is handled ${c} or ${c}-…; the handles there are: ${handles().join(", ")}` };
      return { ok: outer.map((el) => surface(el, arg.rects, true, depth, full)).join("\n") };
    }
    return { ok: surface(root, arg.rects, !!arg.path, depth, full) };
  }
  // The surface as a tree of components (aio's `UISurfaceNode`): each with
  // its key, its path, the elements it drew itself (not a child
  // component's) and its text; the app's own at the top.
  if (op === "tree") {
    const nameOf = new Map(named(top).map(([n, el]) => [el, n]));
    const spans = tree.map(span);
    const depth = tree.map((c) => { let d = 0; for (let p = c.parent; p != null; p = tree[p].parent) d++; return d; });
    const nodes = tree.map((c, i) => ({ component: c.name, key: c.key, path: "", elements: [], children: [], text: "", i }));
    const app = { component: "app", key: null, path: "app", elements: [], children: [], text: words(document.getElementById("main")?.innerText ?? "") };
    nodes.forEach((n, i) => {
      const up = tree[i].parent == null ? app : nodes[tree[i].parent];
      up.children.push(n);
      n.text = cutText(words(scope(spans[i]).innerText ?? ""), 200);
    });
    const pathOf = (n, at) => { n.path = `${at}/${label(tree[n.i])}`; n.children.forEach((c) => pathOf(c, n.path)); };
    app.children.forEach((c) => pathOf(c, "app"));
    for (const el of under(document.getElementById("main") || top)) {
      if (hidden(el)) continue;
      const i = info(el), t = el.getAttribute("data-t");
      if (t === null && i.role === "generic") continue;
      let at = -1;
      spans.forEach((ns, k) => { if (ns.some((n) => n === el || n.contains(el)) && (at < 0 || depth[k] > depth[at])) at = k; });
      const field = i.role === "textbox" || i.role === "combobox";
      (at < 0 ? app : nodes[at]).elements.push({
        name: t ?? nameOf.get(el) ?? "", tag: i.tag, role: i.role, label: i.name, handle: t,
        text: cutText(words(el.innerText ?? ""), 80), value: field ? el.value ?? "" : null,
        checked: checkable(el) ? isChecked(el) : null, disabled: disabled(el),
      });
    }
    const out = (n) => ({ component: n.component, key: n.key, path: n.path, elements: n.elements, children: n.children.map(out), text: n.text });
    return { ok: out(app) };
  }
  if (op === "names" && arg.path) {
    const root = find(arg.path);
    return root ? { ok: named(root).map(([n]) => n) } : missing(arg.path);
  }
  if (op === "names") return { ok: names() };
  if (op === "handles") return { ok: handles() };
  if (op === "count") return { ok: document.querySelectorAll(arg.selector).length };
  if (op === "focused") {
    const a = document.activeElement;
    return { ok: a && a !== document.body ? a.getAttribute("data-t") : null };
  }
  // What the app mounted, then what it put on <body> itself (a tooltip,
  // a portal; not the page's scripts or the dev overlay), without Dioxus's
  // ids, each field as it is now (Dioxus sets values and checks as
  // properties, which HTML leaves out).
  if (op === "html") {
    const main = document.getElementById("main");
    const copied = (node) => {
      const copy = node.cloneNode(true), fields = "input, textarea, option";
      const live = node.querySelectorAll(fields);
      copy.querySelectorAll(fields).forEach((c, i) => {
        const el = live[i];
        if (el.localName === "option") el.selected ? c.setAttribute("selected", "") : c.removeAttribute("selected");
        else if (el.type === "checkbox" || el.type === "radio") el.checked ? c.setAttribute("checked", "") : c.removeAttribute("checked");
        else if (el.localName === "textarea") c.textContent = el.value;
        else if (el.type !== "password" && el.type !== "file") c.setAttribute("value", el.value);
      });
      return copy;
    };
    const own = (n) => n !== main && !(n.nodeType === 1 && (["script", "style", "template"].includes(n.localName) || n.hasAttribute("data-aio-dev")));
    const added = Array.from(document.body.childNodes).filter(own).map((n) => n.nodeType === 1 ? copied(n).outerHTML : n.nodeType === 3 ? n.data.trim() : "");
    const html = (main ? copied(main).innerHTML : "") + added.join("");
    return { ok: html.replace(/ data-dioxus-id="[^"]*"/g, "") };
  }
  if (op === "present") {
    const shown = (el) => el && !hidden(el);
    const byHandle = Array.from(document.querySelectorAll(handle(arg.path))).some(shown);
    return { ok: byHandle || (!arg.handle && shown(find(arg.path))) };
  }
  // Each showing element with text of its own: its colour and the
  // backgrounds behind it up to the first opaque one (null: an image there),
  // as sRGB 0..1 with alpha; a subtree marked data-aio-contrast="skip" is left out.
  if (op === "contrast") {
    const c = document.createElement("canvas");
    c.width = c.height = 1;
    const g = c.getContext("2d", { willReadFrequently: true });
    const rgba = (s) => {
      g.clearRect(0, 0, 1, 1);
      g.fillStyle = "#000";
      g.fillStyle = s;
      g.fillRect(0, 0, 1, 1);
      return Array.from(g.getImageData(0, 0, 1, 1).data).map((v) => v / 255);
    };
    const out = [];
    for (const el of under(document.body)) {
      const own = Array.from(el.childNodes).some((n) => n.nodeType === 3 && n.data.trim());
      if (!own || el.closest('[data-aio-contrast="skip"]') || hidden(el)) continue;
      let layers = [];
      for (let e = el; e && layers; e = e.parentElement) {
        const s = getComputedStyle(e);
        if (s.backgroundImage !== "none") layers = null;
        else {
          const bg = rgba(s.backgroundColor);
          if (bg[3] > 0) layers.push(bg);
          if (bg[3] >= 0.999) break;
        }
      }
      const s = getComputedStyle(el), cls = (el.getAttribute("class") || "").trim();
      out.push({
        place: cls ? `<${el.localName} class="${cls}">` : `<${el.localName}>`,
        text: el.textContent.trim(), fg: rgba(s.color), layers,
        size: parseFloat(s.fontSize), weight: parseFloat(s.fontWeight),
      });
    }
    return { ok: out };
  }
  // Reading needs no showing element: what is there, and its state.
  if (["el", "attr", "style", "rect"].includes(op)) {
    const el = find(arg.path);
    if (!el) return missing(arg.path);
    if (op === "attr") return { ok: el.getAttribute(arg.name) };
    if (op === "style") return { ok: getComputedStyle(el).getPropertyValue(arg.name).trim() };
    if (op === "rect") { const r = el.getBoundingClientRect(); return { ok: { x: r.x, y: r.y, width: r.width, height: r.height } }; }
    const native = el.localName === "input" && ["checkbox", "radio"].includes(el.type);
    return {
      ok: {
        ...info(el), text: el.textContent, value: typeof el.value === "string" ? el.value : "",
        disabled: disabled(el), readonly: el.hasAttribute("readonly"), required: el.hasAttribute("required"),
        focused: document.activeElement === el, checked: native ? el.checked : null,
      },
    };
  }
  // The window: nothing focused, so a key goes to the document.
  if (op === "focus" && arg.path === "window") {
    if (document.activeElement) document.activeElement.blur();
    return { ok: null };
  }
  // A click lands where its press and release meet (their nearest common
  // element): one that moved between them (a late layout shift) misses.
  if (op === "pressed") {
    const p = window.__aioPress || {};
    window.__aioPress = null;
    if (!p.down || !p.up) return { ok: null };
    let meet = p.down;
    while (meet && !meet.contains(p.up)) meet = meet.parentElement;
    if (meet && p.el.contains(meet)) return { ok: null };
    const at = (n) => {
      const who = n.closest("[data-t]")?.getAttribute("data-t");
      return `<${n.localName}>${who ? ` in data-t="${who}"` : ""}`;
    };
    return {
      error: `the click on data-t="${arg.path}" missed it: pressed on ${at(p.down)}, released on ${at(p.up)}, so the click went to ${meet ? at(meet) : "nothing"}; the page moved between press and release (a box that changes size late, such as an image without a fixed size)`,
    };
  }
  const [el, refused] = target(arg.path, arg.verb);
  if (refused) return refused;
  switch (op) {
    case "box": {
      const b = box(el);
      if (b.w === 0 || b.h === 0) return { error: `data-t="${arg.path}" has no box: nothing to ${arg.verb || "reach"}` };
      return { ok: b };
    }
    case "usable": return { ok: null };
    // What the next press and release land on, for `pressed`.
    case "watch": {
      window.__aioPress = { el, down: null, up: null };
      if (!window.__aioPressOn) {
        window.__aioPressOn = true;
        addEventListener("mousedown", (e) => { if (window.__aioPress) window.__aioPress.down = e.target; }, true);
        addEventListener("mouseup", (e) => { if (window.__aioPress) window.__aioPress.up = e.target; }, true);
      }
      return { ok: null };
    }
    case "focus": {
      // As the keyboard gives it: `:focus-visible` rings it.
      el.focus({ focusVisible: true });
      if (document.activeElement !== el && !el.contains(document.activeElement)) {
        return { error: `data-t="${arg.path}" can't take focus: no control, link or tabindex` };
      }
      return { ok: null };
    }
    case "blur": el.blur(); return { ok: null };
    case "edit": {
      // Focused, its text all selected (replace) or the caret at its end (append).
      el.focus();
      if (typeof el.setSelectionRange === "function" && typeof el.value === "string") {
        const n = el.value.length;
        // An email or number field has no selection to set (its caret stays).
        try { el.setSelectionRange(arg.all ? 0 : n, n); } catch {}
      }
      return { ok: null };
    }
    case "select": {
      if (el.localName !== "select") return { error: `data-t="${arg.path}" is no <select>: type into it instead` };
      const opts = Array.from(el.options);
      const o = opts.find((o) => o.value === arg.value);
      if (!o) {
        return { error: `data-t="${arg.path}" has no option ${JSON.stringify(arg.value)}; it has: ${opts.map((o) => JSON.stringify(o.value)).join(", ")}` };
      }
      if (o.disabled) return { error: `option ${JSON.stringify(arg.value)} of data-t="${arg.path}" is disabled` };
      el.value = arg.value;
      fire(el, "input", "change");
      return { ok: null };
    }
    case "checked": {
      if (!checkable(el)) return { error: `data-t="${arg.path}" has no checked state: click it instead` };
      return { ok: isChecked(el) };
    }
    case "scroll": {
      el.scrollTo({ top: arg.top, left: arg.left, behavior: "instant" });
      const w = el.scrollWidth - el.clientWidth, h = el.scrollHeight - el.clientHeight;
      return { ok: { top: el.scrollTop, left: el.scrollLeft, w, h } };
    }
    // The point at fractions (fx, fy) of the part of its box a person
    // sees (brought into the window only as far as needed; what a box with
    // `overflow` cuts off is not there to click).
    case "point": {
      const shows = () => {
        const r = el.getBoundingClientRect();
        let v = [Math.max(r.left, 0), Math.max(r.top, 0), Math.min(r.right, innerWidth), Math.min(r.bottom, innerHeight)];
        for (let a = el.parentElement; a; a = a.parentElement) {
          const s = getComputedStyle(a);
          if (s.overflowX === "visible" && s.overflowY === "visible") continue;
          const c = a.getBoundingClientRect();
          v = [Math.max(v[0], c.left), Math.max(v[1], c.top), Math.min(v[2], c.right), Math.min(v[3], c.bottom)];
        }
        return [r, v];
      };
      let [r, v] = shows();
      if (r.width === 0 || r.height === 0) return { error: `data-t="${arg.path}" has no box: nothing to ${arg.doing}` };
      // Scrolled in only when none of it shows (a scroll can close what a hover opened).
      if (v[2] <= v[0] || v[3] <= v[1]) {
        el.scrollIntoView({ block: "nearest", inline: "nearest" });
        [r, v] = shows();
      }
      const [x0, y0, x1, y1] = v;
      if (x1 <= x0 || y1 <= y0) return { error: `data-t="${arg.path}" is cut off from view: nothing to ${arg.doing}` };
      const [x, y] = [x0 + (x1 - x0) * arg.fx, y0 + (y1 - y0) * arg.fy];
      // What a person's pointer meets there: another element on top gets it.
      const top = document.elementFromPoint(x, y);
      if (top && top !== el && !el.contains(top)) {
        const who = top.closest("[data-t]")?.getAttribute("data-t");
        return { error: `data-t="${arg.path}" is covered at that point by <${top.localName}>${who ? ` in data-t="${who}"` : ""}: nothing to ${arg.doing}` };
      }
      return { ok: { x, y } };
    }
    // HTML drag and drop onto `onto`: one data transfer from dragstart to dragend.
    case "drag": {
      const [to, no] = target(arg.onto, null);
      if (no) return no;
      const dt = new DataTransfer();
      const at = (e) => { const r = e.getBoundingClientRect(); return { clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 }; };
      const send = (e, type) => e.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt, ...at(e) }));
      send(el, "dragstart");
      ["dragenter", "dragover", "drop"].forEach((ty) => send(to, ty));
      send(el, "dragend");
      return { ok: null };
    }
  }
  return { error: `unknown op ${op}` };
})
