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
  const NO_BUBBLE = new Set(['focus', 'blur', 'mouseenter', 'mouseleave', 'pointerenter', 'pointerleave', 'load', 'error', 'scroll'])
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
    const parts = [['u8', 1], ['u16', ids.length], ...ids.map((i) => ['u32', i]), s(e.type), s(e.key), s(e.code), ['u8', e.location || 0]]
    const flags = !!e.shiftKey | (!!e.ctrlKey << 1) | (!!e.altKey << 2) | (!!e.metaKey << 3) | (!!e.repeat << 4) |
      (!!e.isComposing << 5) | ((t.checked === true) << 6) | (!!e.isPrimary << 7)
    parts.push(['u8', flags], ['u8', typeof e.button === 'number' ? e.button : 255], ['u16', e.buttons || 0])
    for (const v of [e.clientX, e.clientY, e.pageX, e.pageY, e.offsetX, e.offsetY, e.screenX, e.screenY, e.deltaX, e.deltaY, e.deltaZ]) parts.push(f(v))
    parts.push(['u8', e.deltaMode || 0], s(typeof t.value === 'string' ? t.value : ''))
    for (const v of [sc.scrollTop, sc.scrollLeft, sc.scrollWidth, sc.scrollHeight, sc.clientWidth, sc.clientHeight]) parts.push(f(v))
    parts.push(['u32', e.pointerId || 0], s(e.pointerType), ['u16', form.length])
    for (const [k, v] of form) parts.push(s(k), s(typeof v === 'string' ? v : v.name))
    window.__aioWire.send(frame(parts))
  }
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
  const run = (id, js) => {
    const inbox = [], waiting = []
    const aio = {
      send: (v) => reply(id, 0, JSON.stringify(v === undefined ? null : v)),
      recv: () => new Promise((r) => (inbox.length ? r(inbox.shift()) : waiting.push(r))),
      node: (n) => nodes.get(n),
    }
    evals.set(id, (v) => (waiting.length ? waiting.shift()(v) : inbox.push(v)))
    window.__airEval = window.__airEval || new Map()
    window.__airEval.set(id, [aio, (ok, v) => {
      evals.delete(id), window.__airEval.delete(id)
      ok ? reply(id, 1, JSON.stringify(v === undefined ? null : v)) : reply(id, 2, String((v && v.message) || v))
    }])
    // A script of the page's own making where the page's policy allows no eval (aio::page RUN).
    const code = `(() => { const [aio, done] = window.__airEval.get(${id}); (async () => {\n${js}\n})().then((v) => done(true, v), (e) => done(false, e)); })();`
    if (window.__aioRun) return window.__aioRun(code)
    const u = URL.createObjectURL(new Blob([code], { type: 'text/javascript' })), el = document.createElement('script')
    el.src = u, el.onload = () => (URL.revokeObjectURL(u), el.remove())
    document.head.appendChild(el)
  }
  const forget = (n) => {
    if (n.__airId !== undefined) nodes.delete(n.__airId)
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

  const apply = (fr) => {
    const t0 = performance.now()
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
    const build = (marks, path, svg) => {
      const k = u8()
      let n, mark = path.length === 1
      if (k === 1) {
        const tag = str(), inSvg = svg || tag === 'svg'
        n = inSvg ? document.createElementNS(SVG, tag) : document.createElement(tag)
        for (let a = u8(); a > 0; a--) n.setAttribute(str(), str())
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
        case 4: node().setAttribute(str(), str()); break
        case 5: node().removeAttribute(str()); break
        case 6: { const n = node(); prop(n, str(), str()); break }
        case 7: { const n = node(); n[str()] = u8() !== 0; break }
        case 8: {
          const n = node(), name = str()
          ;(n.__airOn ||= new Set()).add(name)
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
        default: throw new Error('air: unknown edit at ' + (p - 1))
      }
    }
    if (window.__aioPainted) window.__aioPainted(t0)
  }
  window.__airApply = apply
  window.__aioWire.listen(apply)
})()
