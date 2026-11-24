// QrScanner's script, on its box `el`: the camera into a <video>, and what
// it sees out (`aio.send`, a letter then its text: `o` on, `t` a code's
// text, `f` a frame, `d` denied, `n` no camera, `x` failed). The browser's
// BarcodeDetector reads the codes where it reads QR codes; elsewhere a grey
// frame goes to the app (`decode_qr`), the next once it said it read the
// last (`aio.recv`). The camera is let go once the box leaves the page.
let stream = null;
const video = document.createElement('video');
video.muted = true;
video.playsInline = true;
el.prepend(video);
const stop = () => {
  clearInterval(watch);
  if (stream) stream.getTracks().forEach((t) => t.stop());
  stream = null;
  video.remove();
};
const watch = setInterval(() => el.isConnected || stop(), 200);
const md = navigator.mediaDevices;
if (!md || !md.getUserMedia) return aio.send('nno camera API in this page (it needs https or localhost)');
try {
  stream = await md.getUserMedia({ video: { facingMode: 'environment' } });
} catch (e) {
  const n = e && e.name;
  const t = n === 'NotAllowedError' || n === 'SecurityError' ? 'd' : n === 'NotFoundError' || n === 'OverconstrainedError' ? 'n' : 'x';
  return aio.send(t + ((e && e.message) || e));
}
if (!el.isConnected) return stop();
stream.getVideoTracks().forEach((t) => t.addEventListener('ended', () => stream && aio.send('xthe camera stopped')));
video.srcObject = stream;
await video.play().catch(() => {});
let detector = null;
try {
  if ((await BarcodeDetector.getSupportedFormats()).includes('qr_code')) detector = new BarcodeDetector({ formats: ['qr_code'] });
} catch (e) {}
aio.send('o');
const canvas = document.createElement('canvas');
const g = canvas.getContext('2d', { willReadFrequently: true });
while (stream) {
  if (video.readyState >= 2 && video.videoWidth) {
    if (detector) {
      const codes = await detector.detect(video).catch(() => []);
      if (codes.length && stream) aio.send('t' + codes[0].rawValue);
    } else {
      // At most 480 px a side: its width and height (2 bytes each, high
      // first), then its grey levels, in base64.
      const k = Math.min(1, 480 / Math.max(video.videoWidth, video.videoHeight));
      const w = Math.round(video.videoWidth * k), h = Math.round(video.videoHeight * k);
      if (canvas.width !== w || canvas.height !== h) [canvas.width, canvas.height] = [w, h];
      g.drawImage(video, 0, 0, w, h);
      const px = g.getImageData(0, 0, w, h).data, b = new Uint8Array(4 + w * h);
      b.set([w >> 8, w & 255, h >> 8, h & 255]);
      for (let i = 0; i < w * h; i++) b[4 + i] = (px[4 * i] * 77 + px[4 * i + 1] * 150 + px[4 * i + 2] * 29) >> 8;
      let s = '';
      for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode.apply(null, b.subarray(i, i + 0x8000));
      aio.send('f' + btoa(s));
      await aio.recv();
    }
  }
  await new Promise((r) => setTimeout(r, 120));
}
