// The kit's Canvas painter, an Island: props `p` = { d: the draw-list, v:
// the view box [w, h] or null (CSS pixels), e: 1 to send pointer events, s: 1
// to send the box's size }. It paints the list into a <canvas> at the device's
// pixel ratio, at most once a frame, again when the list, the box's size, the
// pixel ratio or the page's colours change. Ops are in canvas.rs (`Draw`).
const b64 = Uint8Array.fromBase64 ? (s) => Uint8Array.fromBase64(s) : (s) => {
  const t = atob(s), u = new Uint8Array(t.length);
  for (let i = 0; i < t.length; i++) u[i] = t.charCodeAt(i);
  return u;
};
// Images by address, shared by the page's canvases.
const imgs = new Map();
const img = (src) => {
  let i = imgs.get(src);
  if (!i) i = new Image(), i.src = src, imgs.set(src, i);
  return i;
};
// Pixels made in the app (`['g', w, h, data, palette | null, ...]`) as a
// bitmap: RGBA, or one byte per pixel through a palette of RGBA entries.
const bitmap = (o) => {
  const w = o[1], h = o[2];
  if (!(w > 0 && h > 0 && w * h <= 1 << 26)) return null;
  const src = b64(o[3]), px = new ImageData(w, h);
  if (o[4]) {
    const lut = new Uint32Array(b64(o[4]).buffer), out = new Uint32Array(px.data.buffer), n = Math.min(src.length, w * h);
    for (let i = 0; i < n; i++) out[i] = lut[src[i]] || 0;
  } else px.data.set(src.subarray(0, w * h * 4));
  const c = document.createElement('canvas');
  c.width = w, c.height = h;
  c.getContext('2d').putImageData(px, 0, 0);
  return c;
};
return (el, p, emit) => {
  const c = document.createElement('canvas'), box = document.createElement('div'), probe = document.createElement('i');
  // The canvas covers the box, so its pixels never size it; a box nothing
  // sizes is 150 px tall, as a <canvas> is. The canvas's colour is the
  // box's: a theme change runs its transition.
  box.style.cssText = 'position:relative;width:100%;height:100%';
  c.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;transition:color 1ms';
  probe.hidden = true;
  box.append(c);
  el.append(box, probe);
  if (!el.getBoundingClientRect().height) el.style.height = '150px';
  const g = c.getContext('2d');
  let W = 0, H = 0, q = 0, gone = false, depth = 0, bits = new Map(), tint = new Map(), mv = null;
  // A colour the canvas can't read itself (`var(--aio-x)`, `color-mix`) as the page has it now.
  const color = (s) => {
    if (!/var\(|light-dark|color-mix/.test(s)) return s;
    let v = tint.get(s);
    if (v === undefined) probe.style.color = s, v = getComputedStyle(probe).color, tint.set(s, v);
    return v;
  };
  const line = (a, close) => {
    const l = new Path2D();
    for (let i = 0; i + 1 < a.length; i += 2) i ? l.lineTo(a[i], a[i + 1]) : l.moveTo(a[i], a[i + 1]);
    if (close) l.closePath();
    return l;
  };
  const paint = () => {
    q = 0;
    const t0 = performance.now(), r = devicePixelRatio || 1, w = Math.round(W * r), h = Math.round(H * r);
    if (c.width !== w || c.height !== h) c.width = w, c.height = h;
    for (; depth > 0; depth--) g.restore();
    const cs = getComputedStyle(el), v = p.v;
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.clearRect(0, 0, w, h);
    g.setTransform(r * (v ? W / v[0] : 1), 0, 0, r * (v ? H / v[1] : 1), 0, 0);
    g.fillStyle = g.strokeStyle = cs.color;
    g.lineWidth = 1, g.globalAlpha = 1, g.lineJoin = 'round', g.lineCap = 'butt';
    g.font = cs.fontSize + ' ' + cs.fontFamily, g.textAlign = 'start', g.textBaseline = 'middle';
    g.setLineDash([]), g.imageSmoothingEnabled = true;
    g.beginPath();
    const now = new Map();
    for (const o of p.d) switch (o[0]) {
      case 'F': g.fillStyle = color(o[1]); break;
      case 'S': g.strokeStyle = color(o[1]), g.lineWidth = o[2]; break;
      case 'A': g.globalAlpha = o[1]; break;
      case 'T': g.font = o[1]; break;
      case 'L': g.textAlign = o[1]; break;
      case 'D': g.setLineDash(o[1]); break;
      case 'q': g.imageSmoothingEnabled = o[1]; break;
      case 'r': g.fillRect(o[1], o[2], o[3], o[4]); break;
      case 'R': g.strokeRect(o[1], o[2], o[3], o[4]); break;
      case 'l': g.stroke(line(o.slice(1))); break;
      case 'p': g.stroke(line(o[1])); break;
      case 'm': g.moveTo(o[1], o[2]); break;
      case 't': g.lineTo(o[1], o[2]); break;
      case 'b': g.bezierCurveTo(o[1], o[2], o[3], o[4], o[5], o[6]); break;
      case 'a': g.arc(o[1], o[2], o[3], o[4], o[5]); break;
      case 'z': g.closePath(); break;
      case 'f': g.fill(), g.beginPath(); break;
      case 's': g.stroke(), g.beginPath(); break;
      case 'x': g.fillText(o[3], o[1], o[2]); break;
      case 'i': {
        const i = img(o[1]);
        if (i.complete && i.naturalWidth) g.drawImage(i, o[2], o[3], o[4], o[5]);
        else if (!i.complete) i.addEventListener('load', later, { once: true });
        break;
      }
      case 'g': {
        let b = bits.get(o[3]) || now.get(o[3]);
        if (!b || b.l !== o[4]) b = { l: o[4], c: bitmap(o) };
        now.set(o[3], b);
        if (b.c) g.drawImage(b.c, o[5], o[6], o[7], o[8]);
        break;
      }
      case '(': g.save(), depth++; break;
      case ')': if (depth) g.restore(), depth--; break;
      case 'o': g.translate(o[1], o[2]); break;
      case 'k': g.scale(o[1], o[2]); break;
      case 'w': g.rotate(o[1]); break;
    }
    bits = now;
    // The last paint's time, for profiling (`canvas.aioPaintMs`).
    c.aioPaintMs = performance.now() - t0;
  };
  const later = () => { if (!gone) q ||= requestAnimationFrame(paint); };
  const sized = new ResizeObserver(([e]) => {
    W = e.contentRect.width, H = e.contentRect.height;
    if (p.s) emit({ t: 's', w: W, h: H });
    later();
  });
  sized.observe(el);
  // A move per frame at most; any other event sends the move before it first.
  const at = (e) => {
    const b = c.getBoundingClientRect(), v = p.v, x = e.clientX - b.left, y = e.clientY - b.top;
    return v ? [x * v[0] / (b.width || 1), y * v[1] / (b.height || 1)] : [x, y];
  };
  const send = (k, e) => { const [x, y] = at(e); emit({ t: 'p', k, x, y, b: e.buttons }); };
  const flush = () => { if (mv) { const m = mv; mv = null; send('move', m); } };
  const hear = (k) => (e) => {
    if (!p.e) return;
    if (k === 'move') { if (!mv) requestAnimationFrame(flush); mv = e; return; }
    flush();
    if (k === 'down') c.setPointerCapture(e.pointerId);
    send(k, e);
  };
  const on = [['pointerdown', 'down'], ['pointermove', 'move'], ['pointerup', 'up'], ['pointerleave', 'leave'], ['click', 'click']]
    .map(([n, k]) => [n, hear(k)]);
  for (const [n, f] of on) c.addEventListener(n, f);
  c.addEventListener('transitionrun', () => { tint = new Map(); later(); });
  let dpr = matchMedia('(resolution: ' + devicePixelRatio + 'dppx)');
  const zoomed = () => { dpr.removeEventListener('change', zoomed); dpr = matchMedia('(resolution: ' + devicePixelRatio + 'dppx)'); dpr.addEventListener('change', zoomed); later(); };
  dpr.addEventListener('change', zoomed);
  return {
    update: (n) => { p = n; later(); },
    unmount: () => {
      gone = true, cancelAnimationFrame(q), sized.disconnect(), dpr.removeEventListener('change', zoomed);
      box.remove(), probe.remove();
    },
  };
};
