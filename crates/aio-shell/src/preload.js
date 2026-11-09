// The page's only way out: bytes to and from the app, and the host keys of
// its <Browser>s (main.js checks the sender and the guest).
const { contextBridge, ipcRenderer } = require('electron')

let onFrame = null
const early = []
// The app's frames in the order main.js numbered them: a key's answer
// (`ask`) overtakes the frames still on their way, which wait for it.
let next = 1
const held = new Map()
const take = (n, f) => {
  if (n < next) return
  held.set(n, f)
  for (let g; (g = held.get(next)) !== undefined; next++) held.delete(next), onFrame ? onFrame(g) : early.push(g)
}
ipcRenderer.on('aio:wire', (_e, f, n) => take(n, f))

contextBridge.exposeInMainWorld('__aioWire', {
  send: (bytes) => ipcRenderer.send('aio:wire', bytes),
  // An event whose answer the page waits for (main.js `aio:ask`): its
  // frames, and those sent before it not yet here, are applied before
  // this returns.
  ask: (bytes) => { for (const [n, f] of ipcRenderer.sendSync('aio:ask', bytes, next)) take(n, f) },
  listen: (f) => {
    onFrame = f
    early.splice(0).forEach(f)
  },
})

let onKey = null
ipcRenderer.on('aio:key', (_e, id, k) => onKey && onKey(id, k))
// A <Browser>'s preload the shell refused ({ preload, reason }).
let onRefused = null
ipcRenderer.on('aio:guest-refused', (_e, d) => onRefused && onRefused(d))

contextBridge.exposeInMainWorld('__aioGuest', {
  keys: (id, keys) => ipcRenderer.send('aio:keys', id, keys),
  onKey: (f) => { onKey = f },
  onRefused: (f) => { onRefused = f },
})

// The page's own window.close() closes through the window, as its title
// bar's button and aio_shell::page::close() do: left to Chromium the page
// is destroyed first and the window's 'close' never fires (close-to-tray
// and the saved bounds with it; aio measured it on Electron 44).
contextBridge.exposeInMainWorld('__aioClose', () => ipcRenderer.send('aio:close'))
// The window's buttons, for a title bar the page draws (Window { chrome }).
contextBridge.exposeInMainWorld('__aioWindow', {
  minimize: () => ipcRenderer.send('aio:win', 'minimize'),
  maximize: () => ipcRenderer.send('aio:win', 'maximize'),
  close: () => ipcRenderer.send('aio:close'),
})
try {
  contextBridge.executeInMainWorld({ func: () => { window.close = () => window.__aioClose() } })
} catch (e) {
  console.error(`[aio-shell] window.close() is not routed through the window: ${(e && e.message) || e}`)
}
