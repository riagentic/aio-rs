// aio shell for the shared Electron runtime: one window on the app's page.
// Node runs only this file. The page gets no Node, no navigation and no
// network: it loads from aio://app and talks to the app through the preload,
// which this file relays to the app's private socket (length-prefixed frames).
const { app, BrowserWindow, Menu, Notification, Tray, dialog, ipcMain, nativeImage, nativeTheme, net: enet, protocol, screen, session, shell, webContents } = require('electron')
const net = require('node:net')
const path = require('node:path')
const fs = require('node:fs')
const { pathToFileURL } = require('node:url')

const arg = (k) => (process.argv.find((a) => a.startsWith(`--aio-${k}=`)) || '').slice(k.length + 7)
const dir = __dirname

// aio: Cloud Print's mDNS lookup holds the print dialog until an Avahi timeout.
app.commandLine.appendSwitch('disable-features', 'CloudPrintEnable')
// Linux runs the network service unsandboxed anyway: in the browser process
// it is one process and about 15 MB less per app (measured).
if (process.platform === 'linux') app.commandLine.appendSwitch('enable-features', 'NetworkServiceInProcess2')
// nested or GPU-less displays get the software renderer.
if (arg('gpu') === 'software') app.disableHardwareAcceleration()
// Windows files notifications and taskbar entries under this id.
if (process.platform === 'win32' && arg('id')) app.setAppUserModelId(arg('id'))
// The tray the app declared (desktop.rs), or null.
const TRAY = require(path.join(dir, 'tray.json'))
// What the app allows beyond its own page (guest.rs): <Browser>, child
// windows, guest preloads, permissions, the unload dialog, its keys.
const SHELL = require(path.join(dir, 'shell.json'))
const G = require(path.join(dir, 'guest.js'))
// The splash ([build.splash], splash.rs), or null: up first, until the window has painted.
const SPLASH = require(path.join(dir, 'splash.json'))
let splash = null
let splashAt = 0
function openSplash() {
  if (!SPLASH) return
  const [light, dark] = SPLASH.bg
  splash = new BrowserWindow({
    width: SPLASH.width, height: SPLASH.height, center: true, show: false, frame: false,
    resizable: false, movable: false, minimizable: false, maximizable: false, fullscreenable: false,
    skipTaskbar: true, title: arg('title'),
    // A rounded or bordered splash ([build.splash] radius, border): clear around its box.
    transparent: !!SPLASH.clear, backgroundColor: SPLASH.clear ? '#00000000' : nativeTheme.shouldUseDarkColors ? dark : light,
    webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false, javascript: false },
  })
  splash.webContents.on('will-navigate', (e) => e.preventDefault())
  splash.once('ready-to-show', () => {
    if (!splash) return
    splashAt = Date.now()
    splash.show()
    console.error(`[aio-shell] splash: up ${Math.round(process.uptime() * 1000)} ms after the runtime started`)
  })
  splash.loadFile(path.join(dir, 'splash.html'))
  setTimeout(() => endSplash(), SPLASH.max)
}
// The window may show: once the splash has been up its minimum. `then`
// shows it; the splash goes with it (or at its cap, then).
function endSplash(then) {
  const go = () => {
    if (then) then()
    if (!splash || splash.isDestroyed()) return (splash = null)
    if (splashAt) console.error(`[aio-shell] splash: gone after ${Date.now() - splashAt} ms${then ? ', the window shown' : ' (its cap)'}`)
    splashAt = 0
    // The last window closing would end the app before its own opened:
    // hidden until the window shows. Faded out first ([build.splash]
    // fade_ms; where the OS fades windows: Windows, macOS).
    const s = splash
    const away = () => {
      if (s.isDestroyed()) return
      if (BrowserWindow.getAllWindows().length > 1) {
        s.destroy()
        if (splash === s) splash = null
      } else s.hide()
    }
    const steps = Math.round((SPLASH.fade || 0) / 16)
    if (steps < 2 || !s.isVisible()) return away()
    let i = 0
    const t = setInterval(() => {
      if (s.isDestroyed() || ++i >= steps) { clearInterval(t); return away() }
      s.setOpacity(1 - i / steps)
    }, 16)
  }
  const left = splash && splashAt ? SPLASH.min - (Date.now() - splashAt) : 0
  if (left > 0) setTimeout(go, left)
  else go()
}
// A real quit (Quit, the app stopping) passes close-to-tray.
let quitting = false
app.on('before-quit', () => { quitting = true })
// An exit the app decided (it stops, its socket closed) is not the page's
// to refuse; the person's own close keeps the page's beforeunload say:
// told once (aio), or asked Leave / Stay (Window { confirm_unload }).
let leaving = false
const leave = () => { leaving = true; app.quit() }
let vetoSaid = false
app.on('web-contents-created', (_e, wc) => wc.on('will-prevent-unload', (e) => {
  const act = G.unload(leaving, SHELL.confirmUnload)
  if (act === 'leave') return e.preventDefault()
  if (act === 'stay') {
    if (!vetoSaid) console.error('[aio-shell] the page vetoed leaving (a beforeunload handler): the window stays open and no dialog is shown. Window { confirm_unload: true } asks Leave / Stay. (Said once.)')
    vetoSaid = true
    return
  }
  const owner = BrowserWindow.fromWebContents(wc.hostWebContents || wc)
  const o = { type: 'question', buttons: ['Leave', 'Stay'], defaultId: 0, cancelId: 1, title: 'Leave this page?', message: 'Changes you made may not be saved.' }
  try {
    if ((owner ? dialog.showMessageBoxSync(owner, o) : dialog.showMessageBoxSync(o)) === 0) e.preventDefault()
  } catch (err) {
    console.error(`[aio-shell] the Leave / Stay dialog did not open (${err.message}): the page stays`)
  }
}))

protocol.registerSchemesAsPrivileged([{ scheme: 'aio', privileges: { standard: true, secure: true } }])

// Frames from the app socket, split by their 4-byte big-endian length.
function frames(onFrame) {
  let buf = Buffer.alloc(0)
  return (chunk) => {
    buf = Buffer.concat([buf, chunk])
    while (buf.length >= 4) {
      const n = buf.readUInt32BE(0)
      if (buf.length < 4 + n) break
      onFrame(buf.subarray(4, 4 + n))
      buf = buf.subarray(4 + n)
    }
  }
}

app.whenReady().then(() => {
  openSplash()
  // No menu (Electron's default carries DevTools and reload), except on
  // macOS: there the standard keys are menu items (no Edit menu, no Cmd+C
  // in a field). The smallest menu that makes them work, all system roles.
  Menu.setApplicationMenu(null)
  if (process.platform === 'darwin') {
    const name = arg('title') || app.name
    Menu.setApplicationMenu(Menu.buildFromTemplate([
      { label: name, submenu: [
        { role: 'hide', label: `Hide ${name}` }, { role: 'hideOthers' }, { role: 'unhide' },
        { type: 'separator' }, { role: 'quit', label: `Quit ${name}` },
      ] },
      { role: 'editMenu' },
      { role: 'windowMenu', submenu: [{ role: 'minimize' }, { role: 'zoom' }, { role: 'close' }, { type: 'separator' }, { role: 'front' }] },
    ]))
  }
  // The page may ask for nothing (camera, location, its own notifications:
  // those come from the app, desktop.rs); only copying text and fullscreen.
  // More only when the app says (Window { permissions }: clipboard-read…).
  const ALLOWED = new Set(['clipboard-sanitized-write', 'fullscreen', ...SHELL.allow])
  session.defaultSession.setPermissionRequestHandler((_wc, p, cb) => cb(ALLOWED.has(p)))
  session.defaultSession.setPermissionCheckHandler((_wc, p) => ALLOWED.has(p))
  // Every other session is someone else's site (a <Browser>, a child
  // window): no permission (fullscreen when the app says), no device, no
  // download unless the app says. A refusal is said once per kind and
  // origin, 32 at most.
  const said = new Set()
  const say = (key, line) => {
    if (said.has(key) || said.size > 32) return
    said.add(key)
    console.error(`[aio-shell] ${line}${said.size > 32 ? ' (further refusals are not said)' : ''}`)
  }
  const guestOk = (p) => p === 'fullscreen' && SHELL.guestFullscreen
  const guard = (ses) => {
    if (ses === session.defaultSession) return
    ses.setPermissionRequestHandler((wc, p, cb, d) => {
      const from = G.origin((d && d.requestingUrl) || (wc && wc.getURL()) || '')
      if (!guestOk(p)) say(`perm ${p} ${from}`, `permission "${p}" DENIED to ${from || 'an embedded page'}: a page that is not the app's gets none`)
      cb(guestOk(p))
    })
    ses.setPermissionCheckHandler((_wc, p) => guestOk(p))
    ses.setDevicePermissionHandler(() => false)
    for (const ev of ['select-hid-device', 'select-usb-device']) ses.on(ev, (e, _d, cb) => { e.preventDefault(); cb() })
    ses.on('select-serial-port', (e, _p, _wc, cb) => { e.preventDefault(); cb('') })
    ses.on('will-download', (e, item) => {
      if (SHELL.downloads) return
      e.preventDefault()
      const from = G.origin(item.getURL())
      say(`download ${from}`, `download CANCELLED from ${from}: a page that is not the app's may not save files (Web { downloads: true } allows it)`)
    })
  }
  app.on('session-created', guard)
  // The page, and local images by absolute path (images only: the page
  // can show them but never read other files).
  const IMAGE = /\.(png|jpe?g|gif|webp|avif|svg|bmp|ico)$/i
  protocol.handle('aio', (req) => {
    const u = new URL(req.url)
    if (u.host === 'app' && (u.pathname === '/' || u.pathname === '/index.html')) {
      return enet.fetch(pathToFileURL(path.join(dir, 'index.html')).toString())
    }
    if (u.host === 'app' && u.pathname.startsWith('/file/')) {
      const p = path.resolve(decodeURIComponent(u.pathname.slice('/file'.length)))
      if (IMAGE.test(p)) return enet.fetch(pathToFileURL(p).toString())
    }
    // The app's built-in files ([build] assets), at their own paths.
    const assets = path.join(dir, 'assets')
    let own = null
    try { own = path.resolve(assets, '.' + decodeURIComponent(u.pathname)) } catch (_) {}
    if (u.host === 'app' && own && own.startsWith(assets + path.sep) && fs.existsSync(own) && fs.statSync(own).isFile()) {
      return enet.fetch(pathToFileURL(own).toString())
    }
    return new Response('', { status: 404 })
  })
  // The app's control line. Rust → here: "focus", "print", "quit", where to
  // open ({"t":"place"}), an open or save dialog ({"t":"pick"}, {"t":"save"}), a notification
  // ({"t":"notify"}) or the tray's tooltip; here → Rust: the screens, the
  // window's bounds as they change (Rust places and saves: aio-shell's
  // place.rs), what was picked ({"t":"picked"}) and clicks on a
  // notification or a tray item ({"t":"clicked"}, {"t":"tray"}).
  const ctl = net.connect(arg('ctl'))
  const tell = (v) => { if (!ctl.destroyed) ctl.write(JSON.stringify(v) + '\n') }
  ctl.on('connect', () => tell({
    t: 'screens',
    screens: screen.getAllDisplays().map((d) => d.workArea),
    primary: screen.getPrimaryDisplay().workArea,
  }))
  let w = null
  let icon
  let pending = ''
  ctl.on('data', (d) => {
    const lines = (pending + d.toString()).split('\n')
    pending = lines.pop()
    lines.forEach(line)
  })
  // One line from the app (the print key says one too).
  function line(l) {
    if (l.startsWith('{')) {
      const m = JSON.parse(l)
      if (m.t === 'place' && !w) open(m)
      if (m.t === 'pick') pick(m)
      if (m.t === 'save') save(m)
      if (m.t === 'notify') notify(m)
      if (m.t === 'tooltip' && tray) tray.setToolTip(m.text)
      if (m.t === 'icon') setIcon(m)
      if (m.t === 'cdp') devtools(m)
      if (m.t === 'cdp-end') undevtools()
      if (m.t === 'child') child(m)
      if (m.t === 'clear-partition') clearPartition(m)
    }
    if (l === 'focus' || l === 'show') forward()
    // The window's verbs (aio_shell::page): close is the title bar's own,
    // so close-to-tray hides and the bounds are saved, whoever asked.
    if (w && !w.isDestroyed()) {
      if (l === 'minimize') w.minimize()
      if (l === 'maximize') w.isMaximized() ? w.unmaximize() : w.maximize()
      if (l === 'close') w.close()
      if (l === 'hide') w.hide()
    }
    // CSS backgrounds and colours printed too (aio's printBackground).
    if (l === 'print' && w && !w.isDestroyed()) {
      w.webContents.print({ silent: false, printBackground: true }, (ok, why) => { if (!ok && why !== 'Print job canceled') console.error(`[aio-shell] print: ${why}`) })
    }
    // The app is shutting down (a signal): close like the user did, past
    // the page's veto.
    if (l === 'quit') leave()
  }
  ctl.on('error', (e) => console.error(`[aio-shell] control: ${e.message}`))
  // No answer (an old app, a broken line): open at the declared size anyway.
  setTimeout(() => { if (!w) open({}) }, 2000)
  ipcMain.on('aio:wire', (e, bytes) => {
    if (!w || e.sender !== w.webContents) return
    const b = Buffer.from(bytes)
    const head = Buffer.alloc(4)
    head.writeUInt32BE(b.length)
    wire.write(Buffer.concat([head, b]))
  })
  // One connection to the app per page load: the app renders a fresh view
  // tree for each (its state is in its cells). Frames wait until the page
  // has loaded. The app closing the current one ends the runtime.
  let loaded = false
  let early = []
  const connect = () => {
    const s = net.connect(arg('sock'))
    s.on('data', frames((f) => {
      if (s !== wire) return
      if (loaded && w && !w.isDestroyed() && !w.webContents.isCrashed()) w.webContents.send('aio:wire', f)
      else early.push(f)
    }))
    s.on('error', (e) => console.error(`[aio-shell] app socket: ${e.message}`))
    s.on('close', () => { if (s === wire) leave() })
    return s
  }
  let wire = connect()
  // The page anew, on a new connection (a crash, a reload key).
  function reload(clean) {
    loaded = false
    early = []
    const old = wire
    wire = connect()
    old.destroy()
    if (clean) w.webContents.reloadIgnoringCache()
    else w.webContents.reload()
  }

  // A <Browser>'s host keys (guest.js hostKeys), declared by the app's page
  // for a guest of its own, read once: each pressed in the guest, in any of
  // its frames, goes back to the page (keyDown only, and the guest still
  // gets it); an undeclared key never leaves this process.
  const keyed = new WeakSet()
  ipcMain.on('aio:keys', (e, id, keys) => {
    if (!w || e.sender !== w.webContents) return
    const g = webContents.fromId(id)
    if (!g || g.getType() !== 'webview' || g.hostWebContents !== w.webContents || keyed.has(g)) return
    const set = G.hostKeys(keys)
    if (!set) return console.error(`[aio-shell] <Browser> host keys IGNORED: ${JSON.stringify(keys).slice(0, 200)}: 1 to 16 KeyboardEvent.key names`)
    keyed.add(g)
    g.on('before-input-event', (_e, i) => {
      if (i.type !== 'keyDown' || !set.has(i.key) || w.isDestroyed()) return
      w.webContents.send('aio:key', id, { key: i.key, code: i.code, ctrl: !!i.control, shift: !!i.shift, alt: !!i.alt, meta: !!i.meta, repeat: !!i.isAutoRepeat })
    })
  })

  // Pop-ups from the app's page, a guest or a child window (guest.js
  // popup): an http(s) link after real input, to the system browser; never
  // a window. A script (an island's library) can't open one by itself.
  function popups(wc, app) {
    const st = { input: 0, opened: 0 }
    wc.on('input-event', (_e, i) => {
      if (/^(mouseDown|mouseUp|rawKeyDown|keyDown|touchStart|touchEnd|gestureTap)$/.test(i.type)) st.input = Date.now()
    })
    wc.setWindowOpenHandler(({ url }) => {
      const r = G.popup(url, Date.now(), st)
      const from = G.origin(wc.getURL())
      if (r.why) say(`popup ${from} ${r.why.slice(0, 16)}`, `pop-up BLOCKED from ${from}: ${r.why}`)
      else if (arg('no-open')) console.error(`[aio-shell] not opened (AIORT_NO_OPEN, a contained run): ${r.open}`)
      else {
        st.opened = Date.now()
        shell.openExternal(r.open)
      }
      return { action: 'deny' }
    })
    if (!app) wc.on('select-bluetooth-device', (e, _d, cb) => { e.preventDefault(); cb('') })
  }

  // A child window (aio_shell::open_window, Web { windows }): an http(s)
  // page in a window of its own, sandboxed, in a session of its own (never
  // the app's), with a declared preload or none; where it may go is
  // guest.js childNav. Answered {"t":"opened"}, with the reason it was not.
  function child(m) {
    const answer = (r) => tell({ t: 'opened', id: m.id, ...r })
    if (!SHELL.windows) return answer({ error: 'child windows are off: set Window { web: Web { windows: true, .. } }' })
    const part = m.partition || 'persist:aio-child'
    if (session.fromPartition(part) === session.defaultSession) return answer({ error: `partition ${JSON.stringify(part)} is the app's own session` })
    const p = m.preload ? G.preload(G.PRELOAD + m.preload, SHELL.preloads) : null
    if (p && p.why) return answer({ error: `preload refused: ${p.why}` })
    try {
      const c = new BrowserWindow({
        width: m.width || 1100,
        height: m.height || 800,
        icon,
        webPreferences: { contextIsolation: true, nodeIntegration: false, sandbox: true, partition: part, preload: p ? path.join(dir, 'guest', ...p.name.split('/')) : undefined },
      })
      const wc = c.webContents
      popups(wc)
      const ok = new Set([G.origin(m.url), ...(m.origins || []).map(G.origin)])
      let landed = false
      wc.once('did-navigate', (_e, url) => { landed = true; ok.add(G.origin(url)) })
      const nav = (e) => {
        const why = G.childNav(e.url, m.origins, ok, landed)
        if (!why) return
        e.preventDefault()
        say(`child nav ${G.origin(m.url)} ${G.origin(e.url)}`, `navigation BLOCKED in the child window on ${G.origin(m.url)}: ${why}`)
      }
      wc.on('will-navigate', nav)
      wc.on('will-redirect', (e) => { if (e.isMainFrame) nav(e) })
      c.setMenuBarVisibility(false)
      c.loadURL(m.url).catch((e) => console.error(`[aio-shell] child window ${G.origin(m.url)}: ${e.message}`))
      answer({})
    } catch (e) {
      answer({ error: e.message })
    }
  }

  // A guest's session wiped (aio_shell::clear_partition, aio's
  // __aioShell.clearPartition: an app that locks drops what its guests
  // stored): storage, cache and HTTP auth. Never the app's own session,
  // by whatever spelling. Answered {"t":"done"}, with the reason it was not.
  function clearPartition(m) {
    const answer = (r) => tell({ t: 'done', id: m.id, ...r })
    const name = m.partition
    if (typeof name !== 'string' || !name) return answer({ error: "pass the partition's name (a <Browser>'s, or a child window's)" })
    const ses = session.fromPartition(name)
    if (ses === session.defaultSession) return answer({ error: `${JSON.stringify(name)} is the app's own session, not a partition: clearing it would sign the app itself out` })
    Promise.all([ses.clearStorageData(), ses.clearCache(), ses.clearAuthCache()]).then(() => answer({}), (e) => answer({ error: e.message }))
  }

  // The window's DevTools protocol for `aio surface`, `trigger`, `shot`
  // and `eval` (inspect.rs): run in this process, no port opened. Attached
  // on the first call, let go when the last asker leaves ({"t":"cdp-end"}).
  let dbg = null
  function devtools(m) {
    const answer = (r) => tell({ t: 'cdp', id: m.id, ...r })
    if (!w || w.isDestroyed()) return answer({ error: 'the window is not open' })
    dbg = w.webContents.debugger
    // Detached also by a crashed renderer: attached again.
    if (!dbg.isAttached()) {
      try {
        dbg.attach('1.3')
      } catch (e) {
        return answer({ error: `DevTools protocol: ${e.message}` })
      }
      dbg.removeAllListeners('message')
      dbg.on('message', (_e, method, params) => tell({ t: 'cdp-event', method, params }))
    }
    dbg.sendCommand(m.method, m.params).then((result) => answer({ result }), (e) => answer({ error: e.message }))
  }
  function undevtools() {
    const d = dbg
    dbg = null
    if (d && d.isAttached()) d.detach()
  }

  // The window to the front: restored, shown (also from the tray), focused.
  function forward() {
    if (!w || w.isDestroyed()) return
    if (w.isMinimized()) w.restore()
    w.show()
    w.focus()
  }
  // macOS: opening the running app again (its Dock icon, Finder, `open`)
  // only activates it; a window hidden to the tray or minimized comes back.
  app.on('activate', forward)
  // The page's own window.close() (preload.js): the close path above.
  ipcMain.on('aio:close', (e) => { if (w && !w.isDestroyed() && e.sender === w.webContents) w.close() })

  // Notifications from the main process: no permission to ask, the app's
  // icon, and a click that works while the window is hidden. Each is held
  // until it is done (one nothing holds is collected, its click lost); a
  // tag replaces the one before it.
  const notes = new Map()
  let noteSeq = 0
  let unsupportedSaid = false
  function notify(m) {
    if (!Notification.isSupported()) {
      if (!unsupportedSaid) console.error(`[aio-shell] notify: this desktop shows no notifications; "${m.title}" was not shown`)
      unsupportedSaid = true
      return
    }
    const key = m.tag || `#${++noteSeq}`
    notes.get(key)?.close()
    const n = new Notification({ title: m.title, body: m.body, silent: !!m.silent, icon })
    notes.set(key, n)
    const done = () => { if (notes.get(key) === n) notes.delete(key) }
    n.on('click', () => {
      done()
      forward()
      tell({ t: 'clicked', route: m.route })
    })
    n.on('close', done)
    // Linux too: no notification service on the session bus is a failure without a reason.
    n.on('failed', (_e, why) => {
      done()
      console.error(`[aio-shell] notify: "${m.title}" was not shown: ${why || 'the desktop has no notification service'}`)
    })
    n.show()
  }

  // The tray (aio ui.tray): the app's items, then Show, Hide and Quit; a
  // click on the icon shows or hides the window. An item runs in the app
  // ({"t":"tray","i":n}); one with a route brings the window forward first.
  let tray = null
  function openTray() {
    // Linux: a tray needs a host (a StatusNotifierWatcher on the session
    // bus); without one the icon never shows, so close-to-tray minimizes
    // instead. null: unknown (no dbus-send), hide.
    let host = null
    if (TRAY.closeToTray && process.platform === 'linux') {
      require('node:child_process').execFile('dbus-send', ['--session', '--print-reply', '--dest=org.freedesktop.DBus',
        '/org/freedesktop/DBus', 'org.freedesktop.DBus.NameHasOwner', 'string:org.kde.StatusNotifierWatcher'],
      { timeout: 3000 }, (err, out) => {
        if (err && err.code === 'ENOENT') return
        host = !err && /boolean true/.test(String(out))
      })
    }
    if (TRAY.closeToTray) {
      let said = false
      w.on('close', (e) => {
        if (quitting) return
        e.preventDefault()
        if (host !== false) return w.hide()
        w.minimize()
        if (!said) console.error('[aio-shell] tray: this desktop has no tray (no org.kde.StatusNotifierWatcher), so the window was minimized, not hidden')
        said = true
      })
    }
    try {
      tray = new Tray(icon || nativeImage.createEmpty())
    } catch (e) {
      return console.error(`[aio-shell] tray: no system tray on this desktop: ${e.message}`)
    }
    tray.setToolTip(TRAY.tooltip)
    const items = TRAY.menu.map((it) => it === '-' ? { type: 'separator' } : {
      label: it.label,
      click: () => {
        if (it.show) forward()
        tell({ t: 'tray', i: it.i })
      },
    })
    if (items.length) items.push({ type: 'separator' })
    items.push({ label: 'Show', click: forward })
    items.push({ label: 'Hide', click: () => { if (!w.isDestroyed()) w.hide() } })
    items.push({ label: 'Quit', click: () => app.quit() })
    tray.setContextMenu(Menu.buildFromTemplate(items))
    tray.on('click', () => { if (w.isDestroyed()) return; if (w.isVisible()) w.hide(); else forward() })
  }

  // appIcon: the window's or the tray's icon, one image or frames looped
  // here (a frame costs no message); none: the app's own again. Kept until
  // there is a window or a tray to show it.
  const looks = {}
  const timers = {}
  function setIcon(m) {
    looks[m.target] = m
    showIcon(m.target)
  }
  function showIcon(target) {
    const m = looks[target]
    const put = target === 'tray' ? (i) => tray && tray.setImage(i) : (i) => w && !w.isDestroyed() && w.setIcon(i)
    clearInterval(timers[target])
    if (!m || (target === 'tray' ? !tray : !w)) return
    const frames = m.frames.map((u) => {
      const i = nativeImage.createFromDataURL(u)
      return i.getSize().width > 256 ? i.resize({ width: 256, quality: 'best' }) : i
    })
    if (!frames.length) return put(icon || nativeImage.createEmpty())
    let n = 0
    put(frames[0])
    if (frames.length > 1) timers[target] = setInterval(() => put(frames[++n % frames.length]), 1000 / m.fps)
  }

  // O3: the dialog belongs to the window; every outcome is answered.
  // A failure is told as one, never as a cancel (no paths).
  function pick(m) {
    const answer = (r) => tell({ t: 'picked', id: m.id, ...r })
    if (!w || w.isDestroyed()) return answer({ error: 'the window is not open' })
    const properties = [m.folder ? 'openDirectory' : 'openFile']
    if (m.many) properties.push('multiSelections')
    const o = { title: m.title, properties, filters: m.filters }
    if (m.start) o.defaultPath = m.start
    dialog.showOpenDialog(w, o).then(
      (r) => answer({ paths: r.canceled ? [] : r.filePaths }),
      (e) => answer({ error: e.message }),
    )
  }

  // pickSave: replacing a file is asked on every OS (Linux only when told),
  // so a path that comes back is one the user agreed to write.
  function save(m) {
    const answer = (r) => tell({ t: 'picked', id: m.id, ...r })
    if (!w || w.isDestroyed()) return answer({ error: 'the window is not open' })
    const o = { title: m.title, properties: ['showOverwriteConfirmation', 'createDirectory'], filters: m.filters }
    if (m.path) o.defaultPath = m.path
    dialog.showSaveDialog(w, o).then(
      (r) => answer({ paths: r.canceled || !r.filePath ? [] : [r.filePath] }),
      (e) => answer({ error: e.message }),
    )
  }

  function open(p) {
    // the OS theme's background until the page paints (no white flash).
    const [light, dark] = (arg('bg') || '#f3f5f9|#0d1016').split('|')
    // the icon at most 256 px a side (Rust decides the size).
    if (arg('icon')) {
      const [iw, ih] = arg('icon-size').split('x').map(Number)
      icon = nativeImage.createFromPath(arg('icon'))
      if (iw && icon.getSize().width !== iw) icon = icon.resize({ width: iw, height: ih, quality: 'best' })
    }
    w = new BrowserWindow({
      x: p.x, y: p.y,
      width: p.width || Number(arg('width')) || 1180,
      height: p.height || Number(arg('height')) || 800,
      title: arg('title'),
      icon,
      backgroundColor: nativeTheme.shouldUseDarkColors ? dark : light,
      show: false,
      webPreferences: { contextIsolation: true, nodeIntegration: false, sandbox: true, preload: path.join(dir, 'preload.js'), webviewTag: SHELL.browser },
    })
    if (p.maximized) w.maximize()
    if (TRAY) openTray()
    for (const t of Object.keys(looks)) showIcon(t)
    // The normal rect (also while maximized or fullscreen) and the flags.
    let timer = null
    const report = () => {
      clearTimeout(timer)
      if (!w.isDestroyed()) tell({ t: 'bounds', normal: w.getNormalBounds(), maximized: w.isMaximized(), fullscreen: w.isFullScreen() })
    }
    const later = () => { clearTimeout(timer); timer = setTimeout(report, 400) }
    for (const ev of ['resize', 'move', 'maximize', 'unmaximize', 'enter-full-screen', 'leave-full-screen']) w.on(ev, later)
    w.on('close', report)
    // shown once painted, at most SHELL.showCapMs later (Window { show_cap_ms }); 0: at once.
    const show = () => endSplash(() => { if (!w.isDestroyed() && !w.isVisible()) w.show() })
    let painted = false
    w.once('ready-to-show', () => { painted = true; show() })
    if (!SHELL.showCapMs) show()
    else setTimeout(() => {
      if (!painted) console.error(`[aio-shell] the page did not paint within ${SHELL.showCapMs} ms: the window is shown anyway (Window { show_cap_ms })`)
      show()
    }, SHELL.showCapMs)
    w.webContents.on('did-finish-load', () => {
      loaded = true
      for (const f of early.splice(0)) w.webContents.send('aio:wire', f)
      tell({ t: 'window', state: 'loaded' })
    })
    // a hang is told; a crashed renderer is told and reloaded on a new
    // connection, at most 3 times a minute (a crash loop closes the app).
    // A mouse's back key (Windows, Linux) is Back, as Android's button.
    w.on('app-command', (_e, cmd) => { if (cmd === 'browser-backward') tell({ t: 'back' }) })
    w.on('unresponsive', () => tell({ t: 'window', state: 'unresponsive' }))
    w.on('responsive', () => tell({ t: 'window', state: 'responsive' }))
    const crashes = []
    w.webContents.on('render-process-gone', (_e, d) => {
      if (d.reason === 'clean-exit') return
      const now = Date.now()
      while (crashes.length && now - crashes[0] > 60000) crashes.shift()
      crashes.push(now)
      const again = crashes.length <= 3
      tell({ t: 'window', state: 'gone', reason: d.reason, reloading: again })
      if (!again) {
        dialog.showErrorBox(arg('title'), 'The window keeps crashing (4 times in a minute), so the app closes. A problem report was saved.')
        return app.exit(1)
      }
      reload(false)
    })
    // The window's own keys (guest.js shortcut).
    const dev = arg('dev') === '1'
    w.webContents.on('before-input-event', (e, i) => {
      const k = G.shortcut(i, SHELL.keys, dev)
      if (!k) return
      e.preventDefault()
      if (k === 'reload') reload(true)
      if (k === 'print') line('print')
      if (k === 'devtools') w.webContents.toggleDevTools()
      if (k === 'clear') w.webContents.session.clearCache().then(() => w.webContents.session.clearStorageData()).then(() => reload(true))
    })
    // A <Browser>'s guest, before it exists: it never gets Node, a weaker
    // sandbox or the app's own session (it serves aio://app: a page there
    // could read the app); a preload only one the app declared (Web {
    // preloads }), by the name the page gives it.
    w.webContents.on('will-attach-webview', (_e, wp, params) => {
      Object.assign(wp, { nodeIntegration: false, nodeIntegrationInSubFrames: false, contextIsolation: true, webSecurity: true, sandbox: true })
      delete wp.preloadURL
      const part = wp.partition || params.partition
      if (!part || session.fromPartition(part) === session.defaultSession) {
        wp.partition = 'persist:aio-webview'
        say('guest session', `a <Browser> with no partition (or the app's own) gets the session "persist:aio-webview": in the app's session a page could read what it serves on aio://`)
      }
      const p = G.preload(params.preload || wp.preload, SHELL.preloads)
      delete wp.preload
      delete params.preload
      if (p && p.name) wp.preload = path.join(dir, 'guest', ...p.name.split('/'))
      if (p && p.why) console.error(`[aio-shell] <Browser> preload REFUSED: ${p.why}. The guest loads with no preload; declare it in Window { web: Web { preloads: &[(name, include_str!(..))], .. } }`)
      Object.assign(params, { nodeintegration: false, nodeintegrationinsubframes: false, disablewebsecurity: false })
    })
    w.webContents.on('did-attach-webview', (_e, guest) => popups(guest))
    // A contained run (AIORT_NO_OPEN) leaves the person's browser alone.
    popups(w.webContents, true)
    w.webContents.on('will-navigate', (e) => e.preventDefault())
    w.webContents.on('did-fail-load', (_e, code, desc, url) => console.error(`[aio-shell] load failed ${code} ${desc} ${url}`))
    w.webContents.on('preload-error', (_e, p, err) => console.error(`[aio-shell] preload ${p} failed: ${err && err.stack || err}`.replace(/\n/g, '\\n')))
    // The page's console goes to the app's log at its own level (log and
    // info as INFO; debug in a dev build only), with the file it came from:
    // 8 KB each, at most 100 a second (one line says how many were
    // dropped). The log tells the runtime's own errors from the app's by
    // both (aio::log::runtime_noise).
    let second = 0
    let count = 0
    w.webContents.on('console-message', (e) => {
      if (e.level === 'debug' && !dev) return
      const now = Math.floor(Date.now() / 1000)
      if (now !== second) {
        if (count > 100) console.error(`WARN  [page] ${count - 100} more messages dropped (over 100 a second)`)
        second = now
        count = 0
      }
      if (++count > 100) return
      const word = { error: 'ERROR', warning: 'WARN ', debug: 'DEBUG' }[e.level] || 'INFO '
      // file:line as written: no page script is bundled, so it needs no source map.
      const line = Number.isInteger(e.lineNumber) && e.lineNumber > 0 ? `:${e.lineNumber}` : ''
      const src = e.sourceId ? ` src=${String(e.sourceId).slice(0, 512).replace(/\s/g, '%20')}${line}` : ''
      console.error(`${word} [page] ${String(e.message).slice(0, 8192).replace(/\n/g, '\\n')}${src}`)
    })
    w.loadURL('aio://app/')
  }
})
app.on('window-all-closed', () => app.quit())
