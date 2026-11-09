// air's side in the page: applies the app's DOM edits (a binary frame per
// flush, `air::dom`) and sends events back (`air::Raw::encode`). Nodes the
// app names are kept by id; a template is built once with the DOM's own
// calls and cloned per use (cloneNode keeps its empty text holes), its
// roots in a fragment until placed. A view is a range of siblings, first
// to last. One listener per event type on the document (AIR
// `_ensureDelegation`); an event goes to the app with every node on its
// path that listens for it, innermost first: the app's handlers bubble it.
(() => {
  const nodes = new Map([[0, document.getElementById('main')]])
  const tpls = new Map()
  const evals = new Map()
  const dec = new TextDecoder(), enc = new TextEncoder()
  const NO_BUBBLE = new Set(['focus', 'blur', 'mouseenter', 'mouseleave', 'pointerenter', 'pointerleave', 'load', 'error', 'scroll', 'resize', 'visible'])
  // `onresize` and `onvisible` of an element: observed, told as an event of
  // its own (the box, or 1/0 for in view, in its client point).
  const tell = (n, type, x, y) => {
    const e = new Event(type)
    Object.defineProperties(e, { clientX: { value: x }, clientY: { value: y } })
    n.dispatchEvent(e)
  }
  let resizes, visibles
  const observe = (n, name) => {
    if (name === 'resize') {
      resizes ||= new ResizeObserver((es) => { for (const r of es) { const b = r.borderBoxSize && r.borderBoxSize[0]; tell(r.target, 'resize', b ? b.inlineSize : r.contentRect.width, b ? b.blockSize : r.contentRect.height) } })
      resizes.observe(n)
    } else if (name === 'visible') {
      visibles ||= new IntersectionObserver((es) => { for (const r of es) tell(r.target, 'visible', r.isIntersecting ? 1 : 0, 0) })
      visibles.observe(n)
    }
  }
  const SVG = 'http://www.w3.org/2000/svg'
  const delegated = new Set()
  // Typing is never undone: a field keeps the values it sent until the
  // app's value matches one again; an older echo is skipped (aio::page TYPING).
  const sent = new WeakMap()
  document.addEventListener('input', (e) => {
    const t = e.target
    if (typeof t.value !== 'string') return
    if (!sent.has(t)) sent.set(t, [])
    sent.get(t).push(t.value)
  }, true)

  const frame = (parts) => {
    let n = 0
    for (const [k, v] of parts) n += k === 's' ? 4 + v.length : k === 'f' ? 8 : k === 'u32' ? 4 : k === 'u16' ? 2 : 1
    const b = new Uint8Array(n), d = new DataView(b.buffer)
    let p = 0
    for (const [k, v] of parts) {
      if (k === 's') d.setUint32(p, v.length, true), b.set(v, p + 4), (p += 4 + v.length)
      else if (k === 'f') d.setFloat64(p, v, true), (p += 8)
      else if (k === 'u32') d.setUint32(p, v >>> 0, true), (p += 4)
      else if (k === 'u16') d.setUint16(p, v, true), (p += 2)
      else b[p++] = v
    }
    return b
  }
  const s = (v) => ['s', enc.encode(v == null ? '' : String(v))]
  const f = (v) => ['f', +v || 0]

  const send = (ids, e) => {
    const t = e.target || {}, form = e.type === 'submit' && t instanceof HTMLFormElement ? [...new FormData(t)] : []
    const sc = t instanceof Element ? t : document.scrollingElement || {}
    // A key is asked (air::ASKED) where the wire can wait for its answer:
    // its edits are in before the frame is drawn.
    const ask = e.type === 'keydown' && !reading && window.__aioWire.ask
    const parts = [['u8', ask ? 3 : 1], ['u16', ids.length], ...ids.map((i) => ['u32', i]), s(e.type), s(e.key), s(e.code), ['u8', e.location || 0]]
    const flags = !!e.shiftKey | (!!e.ctrlKey << 1) | (!!e.altKey << 2) | (!!e.metaKey << 3) | (!!e.repeat << 4) |
      (!!e.isComposing << 5) | ((t.checked === true) << 6) | (!!e.isPrimary << 7)
    parts.push(['u8', flags], ['u8', typeof e.button === 'number' ? e.button : 255], ['u16', e.buttons || 0])
    for (const v of [e.clientX, e.clientY, e.pageX, e.pageY, e.offsetX, e.offsetY, e.screenX, e.screenY, e.deltaX, e.deltaY, e.deltaZ]) parts.push(f(v))
    parts.push(['u8', e.deltaMode || 0], s(e.dataTransfer ? e.dataTransfer.getData('text/plain') : typeof t.value === 'string' ? t.value : ''))
    for (const v of [sc.scrollTop, sc.scrollLeft, sc.scrollWidth, sc.scrollHeight, sc.clientWidth, sc.clientHeight]) parts.push(f(v))
    parts.push(['u32', e.pointerId || 0], s(e.pointerType), ['u16', form.length])
    for (const [k, v] of form) parts.push(s(k), s(typeof v === 'string' ? v : v.name))
    // The picked files, read here (a view can't reach the page's `File`s):
    // a file input's on its `input`/`change`, a form's on `submit`.
    const picked = e.type === 'submit' ? form.filter(([, v]) => typeof v !== 'string' && v.name)
      : (e.type === 'input' || e.type === 'change') && t instanceof HTMLInputElement && t.type === 'file' && t.files ? [...t.files].map((v) => ['', v]) : []
    const out = () => window.__aioWire.send(frame(parts))
    if (!picked.length && !reading) return parts.push(['u16', 0]), ask ? window.__aioWire.ask(frame(parts)) : out()
    // Reading is async: the events after it wait, so the views get them in order.
    reading++
    queue = queue.then(async () => {
      const got = []
      for (const [k, v] of picked) try { got.push([k, v, new Uint8Array(await v.arrayBuffer())]) } catch {}
      parts.push(['u16', got.length])
      for (const [k, v, b] of got) parts.push(s(k), s(v.name), s(v.type), f(v.lastModified), ['s', b])
      out()
    }).catch(() => {}).finally(() => reading--)
  }
  let reading = 0, queue = Promise.resolve()
  // How many events wait for their files (a test waits for none).
  window.__airReading = () => reading
  const handle = (e) => {
    const ids = []
    for (const n of e.composedPath()) {
      if (n === document || n === window) break
      if (n.__airOn && n.__airOn.has(e.type)) ids.push(n.__airId)
      if (NO_BUBBLE.has(e.type) && n === e.target) break
    }
    if (!ids.length) return
    // AIR's SPA defaults: a handled submit never navigates, nor a handled link.
    if (e.type === 'submit' || (e.type === 'click' && e.target.closest && e.target.closest('a[href]'))) e.preventDefault()
    send(ids, e)
  }
  const reply = (id, kind, v) => window.__aioWire.send(frame([['u8', 2], ['u32', id], ['u8', kind], s(v)]))
  let execs = 0
  const run = (id, js) => {
    const inbox = [], waiting = []
    const aio = {
      send: (v) => reply(id, 0, JSON.stringify(v === undefined ? null : v)),
      recv: () => new Promise((r) => (inbox.length ? r(inbox.shift()) : waiting.push(r))),
      node: (n) => nodes.get(n),
    }
    // An `exec` (id 0) has no answer: a slot of its own, so the several of
    // one batch (their scripts load later, one by one) each find theirs.
    const slot = id || `x${++execs}`
    if (id) evals.set(id, (v) => (waiting.length ? waiting.shift()(v) : inbox.push(v)))
    window.__airEval = window.__airEval || new Map()
    window.__airEval.set(slot, [aio, (ok, v) => {
      evals.delete(id), window.__airEval.delete(slot)
      ok ? reply(id, 1, JSON.stringify(v === undefined ? null : v)) : reply(id, 2, String((v && v.message) || v))
    }])
    // A script of the page's own making where the page's policy allows no eval (aio::page RUN).
    const code = `(() => { const [aio, done] = window.__airEval.get(${JSON.stringify(slot)}); (async () => {\n${js}\n})().then((v) => done(true, v), (e) => done(false, e)); })();`
    if (window.__aioRun) return window.__aioRun(code)
    const u = URL.createObjectURL(new Blob([code], { type: 'text/javascript' })), el = document.createElement('script')
    el.src = u, el.onload = () => (URL.revokeObjectURL(u), el.remove())
    document.head.appendChild(el)
  }
  const forget = (n) => {
    if (n.__airId !== undefined) nodes.delete(n.__airId)
    if (n.__airOn) resizes?.unobserve(n), visibles?.unobserve(n)
    for (let c = n.firstChild; c; c = c.nextSibling) forget(c)
  }
  // The siblings of a view, first to last.
  const span = (first, last) => {
    const a = []
    for (let n = first; n; n = n.nextSibling) {
      a.push(n)
      if (n === last) break
    }
    return a
  }
  const prop = (n, k, v) => {
    const past = k === 'value' ? sent.get(n) : undefined
    if (past) {
      if (v === n.value) return void sent.delete(n)
      if (past.includes(v)) return
      sent.delete(n)
    }
    n[k] = v
  }

  // A page the server painted (`air::ssr`, `data-aio-painted` on #main):
  // the first frame is built apart, then its nodes' ids, listeners and
  // values go to the page's own nodes, which stay (no flash; scroll, focus
  // and typed text kept, what was typed told to the views). A node of
  // another kind takes the built one in its place; an attribute or a text
  // that differs is written.
  let painted = nodes.get(0).hasAttribute('data-aio-painted')
  // The page's children of `p` as air built them: `<!--a-->` marks a text
  // node HTML would have merged or dropped (`air::ssr::TEXT_MARK`).
  const built = (p) => {
    const out = []
    for (let c = p.firstChild; c; c = c.nextSibling) {
      if (c.nodeType !== 8 || c.data !== 'a') { out.push(c); continue }
      const t = c.nextSibling && c.nextSibling.nodeType === 3 ? c.nextSibling : document.createTextNode('')
      c.replaceWith(t)
      out.push((c = t))
    }
    return out
  }
  const alike = (v, r) => v.nodeType === r.nodeType && (v.nodeType !== 1 || (v.namespaceURI === r.namespaceURI && v.tagName === r.tagName))
  const adopt = (v, r, told) => {
    told.n++
    v.__airReal = r
    if (v.__airId !== undefined) (r.__airId = v.__airId), nodes.set(v.__airId, r)
    if (v.__airOn) {
      r.__airOn = v.__airOn
      resizes?.unobserve(v), visibles?.unobserve(v)
      for (const k of v.__airOn) observe(r, k)
    }
    if (v.nodeType !== 1) return void (r.nodeValue !== v.nodeValue && (r.nodeValue = v.nodeValue))
    const typed = 'defaultValue' in r && r.value !== r.defaultValue
    const ticked = (r.type === 'checkbox' || r.type === 'radio') && r.checked !== r.defaultChecked
    for (const a of [...r.attributes]) if (!v.hasAttribute(a.name)) r.removeAttribute(a.name)
    for (const a of v.attributes) if (r.getAttribute(a.name) !== a.value) r.setAttribute(a.name, a.value)
    if (typed) told.push([r, 'input'])
    else if ('value' in v && r.value !== v.value) r.value = v.value
    if (ticked) told.push([r, 'change'])
    else if ('checked' in v && r.checked !== v.checked) r.checked = v.checked
    if (r.tagName !== 'TEXTAREA') kids(v, r, told)
  }
  const kids = (v, r, told) => {
    const vs = [...v.childNodes], rs = built(r)
    vs.forEach((c, i) => {
      if (rs[i] && alike(c, rs[i])) adopt(c, rs[i], told)
      else (rs[i] ? rs[i].replaceWith(c) : r.appendChild(c)), told.missed++
    })
    for (const c of rs.slice(vs.length)) c.remove(), told.missed++
  }

  const apply = (fr) => {
    const t0 = performance.now()
    const main = painted && nodes.get(0)
    if (main) (painted = false), nodes.set(0, document.createElement('div'))
    const b = fr instanceof Uint8Array ? fr : new Uint8Array(fr)
    const v = new DataView(b.buffer, b.byteOffset, b.byteLength)
    let p = 0
    const u8 = () => b[p++]
    const u16 = () => ((p += 2), v.getUint16(p - 2, true))
    const u32 = () => ((p += 4), v.getUint32(p - 4, true))
    const str = () => {
      const n = u32()
      return (p += n), n ? dec.decode(b.subarray(p - n, p)) : ''
    }
    const node = () => {
      const id = u32(), n = nodes.get(id)
      if (!n) throw new Error('air: no node ' + id)
      return n
    }
    const range = () => span(node(), node())
    // Elements whose animation changed: their box, measured below.
    const moving = new Set()
    const animating = (e, was) => e.style.animation !== was && e.style.animationName && moving.add(e)
    // `style:<property>`: a CSS property (rsx!'s style attributes), else an
    // attribute. A `style` string keeps the properties it doesn't set:
    // `position: "fixed", style: "inset: 0"` is both.
    const attr = (n, k, v) => {
      if (k.startsWith('style:')) return n.style.setProperty(k.slice(6), v)
      if (k !== 'style') return n.setAttribute(k, v)
      const s = n.style, had = [...s].map((p) => [p, s.getPropertyValue(p), s.getPropertyPriority(p)])
      n.setAttribute(k, v)
      for (const [p, x, pr] of had) if (!s.getPropertyValue(p)) s.setProperty(p, x, pr)
    }
    const build = (marks, path, svg) => {
      const k = u8()
      let n, mark = path.length === 1
      if (k === 1 || k === 5) {
        const tag = str(), inSvg = svg || k === 5 || tag === 'svg'
        n = inSvg ? document.createElementNS(SVG, tag) : document.createElement(tag)
        for (let a = u8(); a > 0; a--) attr(n, str(), str())
        const children = u16()
        if (u8()) mark = true
        if (mark) marks.push(path.slice())
        for (let i = 0; i < children; i++) path.push(i), n.appendChild(build(marks, path, inSvg)), path.pop()
        return n
      }
      if (k === 2) n = document.createTextNode(str())
      else if (k === 3) (n = document.createTextNode('')), (mark = true)
      else (n = document.createComment('')), (mark = true)
      if (mark) marks.push(path.slice())
      return n
    }
    while (p < b.length) {
      switch (u8()) {
        case 1: {
          const id = u32(), roots = u32()
          u32()
          const t = { roots: [], marks: [] }
          for (let r = 0; r < roots; r++) t.roots.push(build(t.marks, [r], false))
          tpls.set(id, t)
          break
        }
        case 2: {
          const t = tpls.get(u32()), base = u32(), frag = document.createDocumentFragment()
          const cl = t.roots.map((r) => frag.appendChild(r.cloneNode(true)))
          for (let j = 0; j < t.marks.length; j++) {
            const path = t.marks[j]
            let n = cl[path[0]]
            for (let k = 1; k < path.length; k++) n = n.childNodes[path[k]]
            n.__airId = base + j
            nodes.set(base + j, n)
          }
          break
        }
        case 3: node().nodeValue = str(); break
        case 4: { const n = node(), was = n.style?.animation; attr(n, str(), str()); if (n.style) animating(n, was); break }
        case 5: { const n = node(), k = str(); k.startsWith('style:') ? n.style.removeProperty(k.slice(6)) : n.removeAttribute(k); break }
        case 6: { const n = node(); prop(n, str(), str()); break }
        case 7: { const n = node(); n[str()] = u8() !== 0; break }
        case 8: {
          const n = node(), name = str()
          ;(n.__airOn ||= new Set()).add(name)
          observe(n, name)
          if (!delegated.has(name)) delegated.add(name), document.addEventListener(name, handle, NO_BUBBLE.has(name))
          break
        }
        case 9: {
          const at = node(), list = range()
          if (list.length === 1) at.parentNode.insertBefore(list[0], at)
          else {
            const frag = document.createDocumentFragment()
            frag.append(...list)
            at.parentNode.insertBefore(frag, at)
          }
          break
        }
        case 10: { const parent = node(); parent.append(...range()); break }
        case 11: for (const n of range()) forget(n), n.remove(); break
        case 12: { const id = u32(), n = document.createTextNode(str()); n.__airId = id; nodes.set(id, n); break }
        case 13: { const frag = document.createDocumentFragment(); frag.append(...range()); break }
        case 14: { const id = u32(); run(id, str()); break }
        case 15: { const id = u32(), m = str(), to = evals.get(id); if (to) to(JSON.parse(m)); break }
        case 16: {
          const list = range(), k = str(), v = str(), e = list.find((n) => n.nodeType === 1)
          if (e) {
            const was = e.style.animation
            e.style.setProperty(k, v)
            animating(e, was)
          }
          break
        }
        default: throw new Error('air: unknown edit at ' + (p - 1))
      }
    }
    if (main) {
      const told = Object.assign([], { n: 0, missed: 0 })
      kids(nodes.get(0), main, told)
      nodes.set(0, main)
      // A motion measures the page's node that took the built one's place.
      for (const e of [...moving]) if (e.__airReal) moving.delete(e), moving.add(e.__airReal)
      main.removeAttribute('data-aio-painted')
      for (const [n, e] of told) n.dispatchEvent(new Event(e, { bubbles: true }))
      // What hydration kept and what it put in place (`air::ssr`).
      window.__airHydrated = { kept: told.n, replaced: told.missed }
      if (told.missed) console.warn('[air] ' + told.missed + ' nodes of the server paint differed from the view and were replaced')
    }
    // A motion may measure what it moves (aio's TransitionFn got the node):
    // `--aio-w` and `--aio-h`, the element's box without the animation, set
    // before its first frame, all of them in one layout.
    if (moving.size) {
      const els = [...moving].filter((e) => e.isConnected), held = els.map((e) => e.style.animation)
      for (const e of els) e.style.animation = 'none'
      const box = els.map((e) => (e.offsetWidth === undefined ? e.getBoundingClientRect() : { width: e.offsetWidth, height: e.offsetHeight }))
      els.forEach((e, i) => {
        e.style.animation = held[i]
        e.style.setProperty('--aio-w', box[i].width + 'px')
        e.style.setProperty('--aio-h', box[i].height + 'px')
      })
    }
    if (window.__aioPainted) window.__aioPainted(t0)
  }
  window.__airApply = apply
  // A named node, for the page's readers (a component's first and last node).
  window.__airNode = (id) => nodes.get(id)
  window.__aioWire.listen(apply)
})()
