// aio-shell's preload (`window.__aioWire`), timing each frame once the
// page's handler applied it, and each keydown as the page meets it.
const { contextBridge, ipcRenderer } = require('electron')

let onFrame = null
const early = []
const take = (f) => {
  onFrame(f)
  ipcRenderer.send('bench:frame', performance.now(), f.byteLength)
}
ipcRenderer.on('aio:wire', (_e, f) => (onFrame ? take(f) : early.push(f)))
window.addEventListener('keydown', () => ipcRenderer.send('bench:down', performance.now()), true)

contextBridge.exposeInMainWorld('__aioWire', {
  send: (bytes) => ipcRenderer.send('aio:wire', bytes),
  listen: (f) => {
    onFrame = f
    early.splice(0).forEach(take)
  },
})
