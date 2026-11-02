// The UI tests' browser (`aio::testing`): the pinned Electron runtime with
// no window on screen. Each TestUi is a hidden window showing the
// test page at aio://app/ (as the app's window does) in a session of its
// own, driven through its debugger (the DevTools protocol). Lines of JSON:
// `{id, open: {width, height}}`, `{id, page, method, params}`,
// `{id, shot}` (a PNG, base64: a hidden window draws for no DevTools
// screenshot) and `{id, close}` in on stdin; `{id, result}` or `{id, error}`, and the
// page's warnings and errors `{page, console}`, its refused requests
// `{page, refused}` and the ones a test answers `{page, paused}`, out on stdout. Stdin's end
// quits, and the folder this runs from goes with it.
const { app, BrowserWindow, protocol, session } = require('electron')
const fs = require('fs')
const net = require('net')
const path = require('path')

app.setPath('userData', path.join(__dirname, 'data'))
// macOS: no Dock icon, no menu bar, never brought to the front.
if (process.platform === 'darwin') app.setActivationPolicy('accessory')
protocol.registerSchemesAsPrivileged([{ scheme: 'aio', privileges: { standard: true, secure: true } }])
const PAGE = fs.readFileSync(path.join(__dirname, 'page.html'))
const out = (m) => process.stdout.write(JSON.stringify(m) + '\n')
const windows = new Map()
let next = 0

// The real network is refused (aio testUI): only this machine, and the
// hosts a test allowed (`Mount::network`; `*`: any). The app's page never
// needs it; a picture or a font from the internet would make a test flaky.
const LOCAL = /^(localhost|127(\.\d+){3}|\[::1\])$/
const reaches = (url, hosts) => {
  const u = new URL(url)
  return !/^(https?|wss?):$/.test(u.protocol) || LOCAL.test(u.hostname) || hosts.includes('*') || hosts.includes(u.hostname)
}

async function open({ width, height, network = [] }) {
  const id = ++next
  // In memory and per page: one test's storage never reaches another's.
  const s = session.fromPartition(`aio-test-${id}`)
  s.webRequest.onBeforeRequest((d, cb) => {
    if (reaches(d.url, network)) return cb({})
    out({ page: id, refused: d.url })
    cb({ cancel: true })
  })
  s.protocol.handle('aio', (req) => {
    const u = new URL(req.url)
    if (u.host === 'app' && u.pathname === '/') return new Response(PAGE, { headers: { 'content-type': 'text/html' } })
    return new Response('', { status: 404 })
  })
  // Never shown, yet drawing at full speed (the flags it starts with): an
  // offscreen window copies every frame, which 30 tests at once make crawl.
  const w = new BrowserWindow({ show: false, width, height, webPreferences: { session: s, backgroundThrottling: false } })
  const wc = w.webContents
  // As the app's window: the page never navigates away and opens no window.
  wc.setWindowOpenHandler(() => ({ action: 'deny' }))
  wc.on('will-navigate', (e) => e.preventDefault())
  wc.on('console-message', (e) => {
    if (e.level === 'error' || e.level === 'warning') out({ page: id, console: `${e.level}: ${e.message}` })
  })
  wc.debugger.attach('1.3')
  // A request a test answers (`Mount::fetch`, through `Fetch.enable`): to the test.
  wc.debugger.on('message', (_e, method, params) => {
    if (method === 'Fetch.requestPaused') out({ page: id, paused: params })
  })
  windows.set(id, w)
  await w.loadURL('aio://app/')
  return { page: id }
}

function handle(m) {
  if (m.open) return open(m.open)
  const w = windows.get(m.page ?? m.shot ?? m.close)
  if (!w) return Promise.reject(new Error(`no page ${m.page ?? m.shot ?? m.close}`))
  if (m.shot) return w.webContents.capturePage(undefined, { stayHidden: true }).then((i) => ({ png: i.toPNG().toString('base64') }))
  if (m.close) {
    windows.delete(m.close)
    w.destroy()
    return Promise.resolve({})
  }
  return w.webContents.debugger.sendCommand(m.method, m.params || {})
}

app.whenReady().then(() => {
  let buf = ''
  // fd 0 as a socket, not `process.stdin`: on Windows that is an empty
  // stream in Electron (it ends at once), the pipe behind it is not.
  const input = new net.Socket({ fd: 0, readable: true, writable: false })
  input.setEncoding('utf8')
  input.on('data', (d) => {
    buf += d
    for (let i; (i = buf.indexOf('\n')) >= 0; buf = buf.slice(i + 1)) {
      const m = JSON.parse(buf.slice(0, i))
      handle(m).then((result) => out({ id: m.id, result }), (e) => out({ id: m.id, error: String((e && e.message) || e) }))
    }
  })
  input.on('end', () => app.quit())
  out({ ready: true })
})
app.on('window-all-closed', () => {})
// On Windows the runtime's own files are still open here: the next
// browser takes the folder away (`electron.rs`).
process.on('exit', () => {
  try {
    fs.rmSync(__dirname, { recursive: true, force: true })
  } catch {}
})
