// The page's only way out: bytes to and from the app, and the host keys of
// its <Browser>s (main.js checks the sender and the guest).
const { contextBridge, ipcRenderer } = require('electron')

let onFrame = null
const early = []
ipcRenderer.on('aio:wire', (_e, f) => (onFrame ? onFrame(f) : early.push(f)))

contextBridge.exposeInMainWorld('__aioWire', {
  send: (bytes) => ipcRenderer.send('aio:wire', bytes),
  listen: (f) => {
    onFrame = f
    early.splice(0).forEach(f)
  },
})

let onKey = null
ipcRenderer.on('aio:key', (_e, id, k) => onKey && onKey(id, k))

contextBridge.exposeInMainWorld('__aioGuest', {
  keys: (id, keys) => ipcRenderer.send('aio:keys', id, keys),
  onKey: (f) => { onKey = f },
})

// The page's own window.close() closes through the window, as its title
// bar's button and aio_shell::page::close() do: left to Chromium the page
// is destroyed first and the window's 'close' never fires (close-to-tray
// and the saved bounds with it; aio measured it on Electron 44).
contextBridge.exposeInMainWorld('__aioClose', () => ipcRenderer.send('aio:close'))
try {
  contextBridge.executeInMainWorld({ func: () => { window.close = () => window.__aioClose() } })
} catch (e) {
  console.error(`[aio-shell] window.close() is not routed through the window: ${(e && e.message) || e}`)
}
