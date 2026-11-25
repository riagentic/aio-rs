// CodeArea's script, on its textarea `el`, once (a view drawn again keeps
// the node). Tab puts spaces to the next stop (`data-tab` apart), or with
// lines selected indents each one; Shift+Tab outdents. In the page, so
// typing never waits on the app, and through the browser's `insertText`,
// so Ctrl+Z takes it back. Escape, then Tab, leaves the field: no
// keyboard trap. While an IME composes, `data-ime` shows the field's own
// text (the sheet hides the copy under it, which lags the composition).
if (el.__aioCode) return;
el.__aioCode = true;
let free = false;
const put = (a, z, t) => {
  el.setSelectionRange(a, z);
  if (document.execCommand('insertText', false, t)) return;
  el.setRangeText(t, a, z, 'end');
  el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: t }));
};
el.addEventListener('keydown', (e) => {
  const tab = e.key === 'Tab' && !free && !e.ctrlKey && !e.altKey && !e.metaKey && !e.isComposing && !el.readOnly && !el.disabled;
  free = e.key === 'Escape';
  if (!tab) return;
  e.preventDefault();
  const n = Math.max(1, +el.dataset.tab || 4), v = el.value, a = el.selectionStart, z = el.selectionEnd;
  const from = v.lastIndexOf('\n', a - 1) + 1;
  if (!e.shiftKey && !v.slice(a, z).includes('\n')) return put(a, z, ' '.repeat(n - ((a - from) % n)));
  // The lines the selection is on (one that ends where a line starts leaves that line out).
  const nl = v.indexOf('\n', z > a && v[z - 1] === '\n' ? z - 1 : z), to = nl < 0 ? v.length : nl;
  const lead = new RegExp('^(\t| {1,' + n + '})');
  const moves = [];
  let at = from;
  const text = v.slice(from, to).split('\n').map((l) => {
    const d = e.shiftKey ? -((l.match(lead) || [''])[0].length) : l ? n : 0;
    if (d) moves.push([at, d]);
    at += l.length + 1;
    return d > 0 ? ' '.repeat(d) + l : l.slice(-d);
  }).join('\n');
  if (!moves.length) return;
  // Where an offset goes: past each line start before it, by that line's change
  // (`keep`: one right at an indented line's start stays before the indent).
  const map = (p, keep) => moves.reduce((q, [s, d]) => (p > s || (p === s && !keep && d > 0) ? q + (d > 0 ? d : -Math.min(-d, p - s)) : q), p);
  const back = el.selectionDirection === 'backward', na = map(a, true), nz = a === z ? na : map(z, false);
  put(from, to, text);
  el.setSelectionRange(na, nz, back ? 'backward' : 'forward');
});
el.addEventListener('compositionstart', () => el.setAttribute('data-ime', ''));
el.addEventListener('compositionend', () => el.removeAttribute('data-ime'));
