// The window's side of `ax shot --video` where no ffmpeg is (video.rs, aio
// media/encoder): recorded JPEG frames in, Chromium's own WebCodecs encoder
// (VP8 for .webm, H.264 for .mp4), encoded frames out as base64.
// open {format, data}: sized by the first frame (even: both codecs need it)
// → {w, h, codec}; put {data, us, dur, key} → {chunks}; done → {chunks,
// description} (H.264's avcC). Any failure is {error}.
(async (op, a) => {
  const K = Symbol.for('aio.video')
  const S = globalThis[K] || (globalThis[K] = {})
  const b64 = (u8) => {
    let s = ''
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000))
    return btoa(s)
  }
  const image = (data) => createImageBitmap(new Blob([Uint8Array.from(atob(data), (c) => c.charCodeAt(0))], { type: 'image/jpeg' }))
  try {
    if (op === 'open') {
      if (typeof VideoEncoder !== 'function') return { error: 'this window has no WebCodecs encoder' }
      const first = await image(a.data)
      const w = first.width & ~1
      const h = first.height & ~1
      first.close()
      // H.264's level admits the frame size (aio codecCandidates).
      const mbs = Math.ceil(w / 16) * Math.ceil(h / 16)
      const level = [[3600, '1f'], [8192, '28'], [22080, '32'], [36864, '33']].find(([max]) => mbs <= max)
      if (a.format === 'mp4' && (w < 16 || h < 16 || !level)) return { error: `${w}x${h} can't be H.264: write .webm` }
      const codecs = a.format === 'webm' ? ['vp8'] : ['6400', '4d00', '4200'].map((p) => `avc1.${p}${level[1]}`)
      for (const codec of codecs) {
        const cfg = { codec, width: w, height: h, bitrate: Math.max(1e6, w * h * 4), framerate: 30 }
        if (a.format === 'mp4') cfg.avc = { format: 'avc' }
        if (!(await VideoEncoder.isConfigSupported(cfg).catch(() => ({}))).supported) continue
        Object.assign(S, { out: [], error: null, description: null, w, h })
        S.enc = new VideoEncoder({
          output: (c, m) => {
            const d = new Uint8Array(c.byteLength)
            c.copyTo(d)
            S.out.push({ key: c.type === 'key', us: c.timestamp, data: b64(d) })
            const desc = m && m.decoderConfig && m.decoderConfig.description
            if (desc) S.description = b64(new Uint8Array(desc.buffer || desc, desc.byteOffset || 0, desc.byteLength))
          },
          error: (e) => { S.error = e.message },
        })
        S.enc.configure(cfg)
        S.canvas = new OffscreenCanvas(w, h)
        S.ctx = S.canvas.getContext('2d')
        return { w, h, codec }
      }
      return { error: `this window's Chromium encodes no ${a.format === 'webm' ? 'VP8' : 'H.264'} at ${w}x${h}` }
    }
    if (op === 'put') {
      const img = await image(a.data)
      // The same size (an odd pixel cropped): 1:1; a resized window: fitted, centred.
      const fit = Math.abs(img.width - S.w) > 1 || Math.abs(img.height - S.h) > 1
      const k = fit ? Math.min(S.w / img.width, S.h / img.height) : 1
      const [dw, dh] = [img.width * k, img.height * k]
      S.ctx.fillStyle = '#000'
      S.ctx.fillRect(0, 0, S.w, S.h)
      S.ctx.drawImage(img, fit ? (S.w - dw) / 2 : 0, fit ? (S.h - dh) / 2 : 0, dw, dh)
      img.close()
      const f = new VideoFrame(S.canvas, { timestamp: a.us, duration: a.dur })
      S.enc.encode(f, { keyFrame: a.key })
      f.close()
      while (S.enc.encodeQueueSize > 2) await new Promise((r) => S.enc.addEventListener('dequeue', r, { once: true }))
    }
    if (op === 'done') {
      await S.enc.flush()
      S.enc.close()
    }
    if (S.error) return { error: S.error }
    const r = { chunks: S.out.splice(0), description: S.description }
    if (op === 'done') delete globalThis[K]
    return r
  } catch (e) {
    return { error: String(e && e.message || e) }
  }
})
