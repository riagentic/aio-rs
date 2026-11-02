// The held-arrow harness: the shell's wire (a socket, 4-byte length
// frames, `aio:wire` IPC as aio-shell's main.js) to a hidden, headless
// window showing page.html at aio://app/. Once the views settled it holds
// ↓ (`--keys` presses, `--gap` ms apart, as a key repeat) through the
// DevTools protocol and prints, per key, the time from the page's keydown
// to the last frame applied after it, and the bytes those frames carried.
const { app, BrowserWindow, ipcMain, protocol, session } = require('electron')
const fs = require('fs')
const net = require('net')
const path = require('path')

const arg = (k) => (process.argv.find((a) => a.startsWith(`--${k}=`)) || '').split('=')[1]
app.setPath('userData', path.join(__dirname, 'data'))
protocol.registerSchemesAsPrivileged([{ scheme: 'aio', privileges: { standard: true, secure: true } }])
const PAGE = fs.readFileSync(path.join(__dirname, 'page.html'))
const KEYS = Number(arg('keys') || 200), GAP = Number(arg('gap') || 40)
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

app.whenReady().then(async () => {
  const s = session.fromPartition('bench')
  s.protocol.handle('aio', () => new Response(PAGE, { headers: { 'content-type': 'text/html' } }))
  const w = new BrowserWindow({
    show: false, width: 900, height: 700,
    webPreferences: { session: s, preload: path.join(__dirname, 'preload.js'), backgroundThrottling: false },
  })
  const wc = w.webContents
  wc.on('console-message', (e) => { if (e.level === 'error' || e.level === 'warning') console.error(`[page] ${e.message}`) })
  const sock = net.connect(arg('sock'))
  let buf = Buffer.alloc(0), loaded = false
  const early = []
  sock.on('data', (d) => {
    buf = Buffer.concat([buf, d])
    while (buf.length >= 4 && buf.length >= 4 + buf.readUInt32BE(0)) {
      const n = buf.readUInt32BE(0), f = buf.subarray(4, 4 + n)
      buf = buf.subarray(4 + n)
      if (loaded) wc.send('aio:wire', f)
      else early.push(Buffer.from(f))
    }
  })
  ipcMain.on('aio:wire', (_e, bytes) => {
    const b = Buffer.from(bytes), head = Buffer.alloc(4)
    head.writeUInt32BE(b.length)
    sock.write(Buffer.concat([head, b]))
  })
  // Frames applied: [time, bytes]; keydowns: time.
  let frames = [], downs = []
  ipcMain.on('bench:frame', (_e, t, n) => frames.push([t, n]))
  ipcMain.on('bench:down', (_e, t) => downs.push(t))
  await w.loadURL('aio://app/')
  loaded = true
  for (const f of early.splice(0)) wc.send('aio:wire', f)
  // Settled: no frame for 500 ms.
  for (let quiet = 0; quiet < 500; quiet += 50) {
    const n = frames.length
    await sleep(50)
    if (frames.length !== n) quiet = 0
  }
  const nodes = await wc.executeJavaScript(`document.querySelectorAll('li').length`)
  await wc.executeJavaScript(`document.querySelector('[data-t=keys]').focus()`)
  wc.debugger.attach('1.3')
  await wc.debugger.sendCommand('Emulation.setFocusEmulationEnabled', { enabled: true })
  frames = []
  downs = []
  const key = { key: 'ArrowDown', code: 'ArrowDown', windowsVirtualKeyCode: 40, nativeVirtualKeyCode: 40 }
  for (let i = 0; i < KEYS; i++) {
    await wc.debugger.sendCommand('Input.dispatchKeyEvent', { type: 'rawKeyDown', autoRepeat: i > 0, ...key })
    await wc.debugger.sendCommand('Input.dispatchKeyEvent', { type: 'keyUp', ...key })
    await sleep(GAP)
  }
  await sleep(500)
  // Each frame belongs to the last keydown before it.
  const lat = [], bytes = []
  downs.forEach((t, i) => {
    const end = downs[i + 1] ?? Infinity
    const mine = frames.filter(([ft]) => ft >= t && ft < end)
    lat.push(mine.length ? mine[mine.length - 1][0] - t : null)
    bytes.push(mine.reduce((a, [, n]) => a + n, 0))
  })
  const selected = await wc.executeJavaScript(`[...document.querySelectorAll('li')].findIndex((l) => l.hasAttribute('aria-selected') && l.getAttribute('aria-selected') !== 'false')`)
  process.stdout.write(JSON.stringify({ bench: true, nodes, selected, keys: downs.length, lat, bytes }) + '\n')
  sock.destroy()
  app.exit(0)
})
