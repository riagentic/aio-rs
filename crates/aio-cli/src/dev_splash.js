// ax dev's splash (tooling, never in an app): up while the first build runs,
// cargo's own count (N/M) and the crates it is on, a failed build's errors;
// gone once the app's window is up. ax drives it by JSON lines on stdin:
// {t:'show'}, {t:'progress', n, m, names}, {t:'built'}, {t:'failed', text, start?},
// {t:'close'}. Closing it ends only the splash: the build goes on.
const { app, BrowserWindow } = require('electron')

const arg = (k) => (process.argv.find((a) => a.startsWith(`--ax-${k}=`)) || '').slice(k.length + 6)
const title = arg('title') || 'ax dev'
app.commandLine.appendSwitch('disable-features', 'CloudPrintEnable')

const esc = (s) => String(s).replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c])
// A string literal inside the page's <script>: no `</script>` can end it.
const js = (s) => JSON.stringify(String(s)).replace(/</g, '\\u003c')
const page = `<!doctype html><html><head><meta charset="utf-8"><title>${esc(title)}</title><style>
html,body{margin:0;height:100%;background:#1b1b1f;color:#e8e8ec;font:13px/1.45 system-ui,sans-serif}
body{display:flex;flex-direction:column;justify-content:center;padding:0 28px;box-sizing:border-box}
h1{font-size:17px;font-weight:600;margin:0 0 14px}
#bar{height:6px;border-radius:3px;background:#33333a;overflow:hidden}
#fill{height:100%;width:0;background:#7aa2ff;transition:width .15s}
#n{margin:10px 0 0;color:#a8a8b3;font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
#err{display:none;margin:12px 0 0;max-height:62vh;overflow:auto;background:#2a1517;color:#ffd7d5;padding:8px 10px;border-radius:6px;font:12px/1.4 ui-monospace,monospace;white-space:pre-wrap}
body.failed #bar,body.failed #n{display:none}body.failed{justify-content:flex-start;padding-top:22px}
</style></head><body><h1 id="t">Building ${esc(title)}…</h1><div id="bar"><div id="fill"></div></div><p id="n">starting cargo</p><pre id="err"></pre>
<script>
window.update = (m) => {
  const $ = (id) => document.getElementById(id)
  if (m.t === 'progress') {
    document.body.classList.remove('failed')
    $('t').textContent = 'Building ' + ${js(title)} + '…'
    $('fill').style.width = (m.m ? Math.min(100, 100 * m.n / m.m) : 0) + '%'
    $('n').textContent = m.n + '/' + m.m + ' crates' + (m.names ? ': ' + m.names : '')
  } else if (m.t === 'built') {
    document.body.classList.remove('failed')
    $('fill').style.width = '100%'
    $('t').textContent = 'Starting ' + ${js(title)} + '…'
    $('n').textContent = 'built'
  } else if (m.t === 'failed') {
    document.body.classList.add('failed')
    $('t').textContent = ${js(title)} + (m.start ? " didn't start: save a fix, or ax restart" : " doesn't build: save a fix")
    $('err').style.display = 'block'
    $('err').textContent = m.text || ''
  }
}
</script></body></html>`

let w = null
let loaded = false
const queue = []
function send(m) {
  if (!w || w.isDestroyed()) return
  if ((m.t === 'show' || m.t === 'failed') && !w.isVisible()) {
    w.show()
    // ax says it once, on its terminal.
    process.stdout.write('shown\n')
  }
  if (m.t === 'show') return
  w.webContents.executeJavaScript(`update(${JSON.stringify(m)})`).catch(() => {})
}
app.whenReady().then(() => {
  w = new BrowserWindow({
    width: 560, height: 340, show: false, title, backgroundColor: '#1b1b1f', autoHideMenuBar: true,
    webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false },
  })
  w.webContents.on('will-navigate', (e) => e.preventDefault())
  w.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
  w.webContents.once('did-finish-load', () => { loaded = true; for (const m of queue.splice(0)) send(m) })
  w.loadURL('data:text/html;charset=utf-8,' + encodeURIComponent(page))
  w.on('closed', () => app.quit())
})
let buf = ''
process.stdin.setEncoding('utf8')
process.stdin.on('data', (d) => {
  buf += d
  let i
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i)
    buf = buf.slice(i + 1)
    let m
    try { m = JSON.parse(line) } catch { continue }
    if (m.t === 'close') return app.quit()
    if (!loaded) queue.push(m)
    else send(m)
  }
})
process.stdin.on('end', () => app.quit())
app.on('window-all-closed', () => app.quit())
