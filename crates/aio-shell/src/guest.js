// What a page that is not the app's may do: a <Browser>'s guest
// (aio::kit::Browser) and a child window (aio_shell::open_window). Pure:
// main.js wires it, and tests run it in Node (guest.rs).

// The name a page gives a declared preload: Electron drops any other scheme
// from a <webview preload> before main sees it, and it names no file.
exports.PRELOAD = 'file:///aio-guest-preload/'

// A guest's preload `want`: null when none was asked, { name } when it is
// one the app declared (an exact match, so nothing a page writes can climb
// out of the folder they are in), else { why }.
exports.preload = (want, declared) => {
  if (!want) return null
  const name = String(want).startsWith(exports.PRELOAD) ? String(want).slice(exports.PRELOAD.length) : ''
  if (name && declared.includes(name)) return { name }
  return { why: `${JSON.stringify(String(want).slice(0, 200))} is not a declared guest preload (declared: ${declared.join(', ') || 'none'})` }
}

// A guest's host keys (KeyboardEvent.key names): 1 to 16 of at most 32
// characters, as a Set; anything else is null, never half applied.
exports.hostKeys = (keys) => Array.isArray(keys) && keys.length > 0 && keys.length <= 16 &&
  keys.every((k) => typeof k === 'string' && k.length > 0 && k.length <= 32)
  ? new Set(keys)
  : null

// A guest's or child's pop-up: an http(s) link goes to the system browser
// only after a real click or key in that page (main-process input, which a
// page cannot make) in the last 5 s, one per 2 s; `st` is that page's
// { input, opened } (ms). { open: href } or { why }.
exports.popup = (url, now, st) => {
  let u
  try { u = new URL(url) } catch { return { why: 'not a URL' } }
  if (u.protocol !== 'http:' && u.protocol !== 'https:') return { why: `only http(s) links open in the system browser, got ${u.protocol.slice(0, 32)}` }
  if (now - st.input > 5000) return { why: 'no click or key in that page in the last 5 s: a page may not open the system browser by itself' }
  if (now - st.opened < 2000) return { why: 'more than one link in 2 s' }
  return { open: u.href }
}

// The origin of `url` ('' when it is none).
const origin = (url) => { try { return new URL(url).origin } catch { return '' } }
exports.origin = origin

// May a child window go to `url`? Never to the app's scheme, a file or
// javascript:; with `origins` listed, only http(s) to those and the one it
// landed on (`ok`, a Set of origins; its first load's redirects are its own
// until `landed`). null, or why not.
exports.childNav = (url, origins, ok, landed) => {
  let u = null
  try { u = new URL(url) } catch {}
  if (!u) return 'not a URL'
  if (['aio:', 'file:', 'javascript:'].includes(u.protocol)) return `a child window never goes to ${u.protocol}`
  if (!origins) return null
  if (u.protocol !== 'http:' && u.protocol !== 'https:') return 'a child window opened with origins goes to http(s) only'
  return !landed || ok.has(u.origin) ? null : `${u.origin} is not one of its origins`
}

// The window's own keys (aio electron.shortcuts): Ctrl+R, Ctrl+Shift+R and
// Ctrl+F5 reload, Ctrl+P prints, unless `keys` gives them to the page; F12
// and Ctrl+Shift+I toggle DevTools in a dev build or one whose app asked
// (`devtools`: Window { devtools }), and a dev build's Ctrl+Shift+Delete
// clears the cache and reloads. Plain F5 stays the app's. The action, or null.
exports.shortcut = (input, keys, dev, devtools) => {
  if (input.type !== 'keyDown') return null
  const ctrl = input.control || input.meta
  const k = String(input.key).toLowerCase()
  if (!keys.reload && ctrl && (k === 'r' || k === 'f5')) return 'reload'
  if (!keys.print && ctrl && !input.shift && k === 'p') return 'print'
  if ((dev || devtools) && (k === 'f12' || (ctrl && input.shift && k === 'i'))) return 'devtools'
  if (dev && ctrl && input.shift && k === 'delete') return 'clear'
  return null
}

// A refusal's line, said once per kind and key (`said`: a Map of Sets): a
// kind stops after `max` different ones and says so once, the other kinds
// go on, so a page can neither flood the log nor talk it into silence.
// The line to say, or null.
exports.refusal = (said, kind, key, line, max = 32) => {
  if (!said.has(kind)) said.set(kind, new Set())
  const seen = said.get(kind)
  if (seen.has(key) || seen.size > max) return null
  seen.add(key)
  return seen.size > max ? `further ${kind} lines are not said` : line
}

// The line for permission `p` denied to `from` (an origin, never a URL: a
// key in it stays out of the log), naming what would grant it. `who`: the
// app's own page, a child window on its own origin, or another page;
// `grantable`: the names a page may be given (guest.rs PERMISSIONS).
exports.denied = (p, from, who, grantable) => {
  const at = `permission "${p}" DENIED to ${who === 'app' ? "the app's own page" : from || 'an embedded page'}`
  if (p === 'notifications') return `${at}: notifications come from the app (aio_shell::desktop::notify), not a page`
  if (!grantable.includes(p)) return `${at}: no page may hold it`
  if (who === 'app') return `${at}: Window { permissions: &["${p}"] } grants it`
  if (who === 'child') return `${at}: Child { permissions: vec!["${p}".into()] } grants it to the window's own origin`
  return p === 'fullscreen' ? `${at}: Web { fullscreen: true } grants it` : `${at}: a page that is not the app's gets none`
}

// A child window's own grant (`grant`: { origin, perms }, Child {
// permissions }): `p` asked from its own origin only, never a frame of
// another site in it.
exports.childAllows = (grant, p, from) => !!grant && grant.perms.includes(p) && from === grant.origin

// A page's beforeunload veto: an exit the app decided leaves anyway; else
// the person is asked Leave / Stay (`confirm`), or the page keeps its say.
exports.unload = (leaving, confirm) => (leaving ? 'leave' : confirm ? 'ask' : 'stay')
