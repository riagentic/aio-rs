// aio-rs's client for any view library (aio's `aio/state-core`): a page, or
// Node or Deno, holds a copy of an aio-rs app's cells over the app's own
// wire (wire 2 on `/ws`, every cell on one socket), follows their patches,
// calls their methods (shown at once with a prediction, answered by the
// server) and keeps the calls made offline until the server takes them.
// No dependency. Served by every aio-rs server at `/__aio/client.js`.

/** Calls waiting for the server, at most; then a call is refused (`QUEUE_OVERFLOW`). */
export const PENDING_MAX = 500;
/** The answered calls a copy remembers, so a twin tab's write never puts one back. */
const DONE_MAX = 1024;
/** A muxed link heard nothing for this long (ms) is dead: the server pings every 2 s. */
export const DEAD = 5000;
const MUX = "aio-mux";

/** The server's `/ws` for a page at `loc` (its `?token=` passed on). */
export function wsUrl(loc) {
  const token = new URLSearchParams(loc.search).get("token");
  return `${loc.protocol === "https:" ? "wss" : "ws"}://${loc.host}/ws?wire=2${token ? `&token=${encodeURIComponent(token)}` : ""}`;
}

/** A call id the server takes: a UUIDv7 (its time, then random bits). */
export function uuidv7(now = Date.now()) {
  const b = crypto.getRandomValues(new Uint8Array(16));
  for (let i = 0; i < 6; i++) b[i] = Math.floor(now / 2 ** (8 * (5 - i))) & 255;
  b[6] = (b[6] & 15) | 0x70;
  b[8] = (b[8] & 63) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

/**
 * `view` with the server's patch `ops` applied (pure: `view` is untouched;
 * what no op touched is shared, so a selector of an untouched part keeps
 * its value). Throws when an op doesn't fit.
 */
export function apply(view, ops) {
  const fresh = new Set();
  const own = (x) => {
    if (fresh.has(x)) return x;
    const c = Array.isArray(x) ? x.slice() : { ...x };
    fresh.add(c);
    return c;
  };
  const container = (x, step) =>
    typeof step === "number" ? Array.isArray(x) : x !== null && typeof x === "object" && !Array.isArray(x);
  // `edit(parent)` on the container at `path`, copied on the way down.
  const at = (root, path, edit) => {
    if (path.length === 0) return edit(root);
    const [s, ...rest] = path;
    if (!container(root, s) || !(s in root)) throw new Error(`no ${typeof s === "number" ? "index" : "key"} ${s}`);
    const c = own(root);
    c[s] = at(root[s], rest, edit);
    return c;
  };
  const parentOf = (root, path, edit) => {
    const last = path[path.length - 1];
    return at(root, path.slice(0, -1), (p) => {
      if (!container(p, last)) throw new Error(`path step ${JSON.stringify(last)} doesn't fit`);
      const c = own(p);
      edit(c, last);
      return c;
    });
  };
  for (const op of ops) {
    const p = op.path;
    if (op.op === "set" && p.length === 0) view = op.value;
    else if (op.op === "set")
      view = parentOf(view, p, (c, k) => {
        if (typeof k === "number" && k >= c.length) throw new Error(`no index ${k}`);
        c[k] = op.value;
      });
    else if (op.op === "del")
      view = parentOf(view, p, (c, k) => {
        if (!(k in c)) throw new Error(`no ${typeof k === "number" ? "index" : "key"} ${k}`);
        typeof k === "number" ? c.splice(k, 1) : delete c[k];
      });
    else if (op.op === "insert")
      view = parentOf(view, p, (c, k) => {
        if (typeof k !== "number" || k > c.length) throw new Error(`no array to insert into at index ${k}`);
        c.splice(k, 0, op.value);
      });
    else if (op.op === "append")
      view = parentOf(view, p, (c, k) => {
        if (typeof c[k] !== "string") throw new Error("append to a missing or non-string value");
        c[k] += op.value;
      });
    else throw new Error(`unknown op ${op.op}`);
  }
  return view;
}

const K = Uint32Array.from(
  "428a2f98 71374491 b5c0fbcf e9b5dba5 3956c25b 59f111f1 923f82a4 ab1c5ed5 d807aa98 12835b01 243185be 550c7dc3 72be5d74 80deb1fe 9bdc06a7 c19bf174 e49b69c1 efbe4786 0fc19dc6 240ca1cc 2de92c6f 4a7484aa 5cb0a9dc 76f988da 983e5152 a831c66d b00327c8 bf597fc7 c6e00bf3 d5a79147 06ca6351 14292967 27b70a85 2e1b2138 4d2c6dfc 53380d13 650a7354 766a0abb 81c2c92e 92722c85 a2bfe8a1 a81a664b c24b8b70 c76c51a3 d192e819 d6990624 f40e3585 106aa070 19a4c116 1e376c08 2748774c 34b0bcb5 391c0cb3 4ed8aa4a 5b9cca4f 682e6ff3 748f82ee 78a5636f 84c87814 8cc70208 90befffa a4506ceb bef9a3f7 c67178f2"
    .split(" ")
    .map((h) => parseInt(h, 16)),
);

/** SHA-256 of `bytes` (FIPS 180-4), synchronous: a page that isn't a secure context has no `crypto.subtle`. */
export function sha256(bytes) {
  const len = bytes.length, total = Math.ceil((len + 9) / 64) * 64;
  const m = new Uint8Array(total);
  m.set(bytes);
  m[len] = 0x80;
  const dv = new DataView(m.buffer);
  dv.setUint32(total - 8, Math.floor(len / 0x20000000));
  dv.setUint32(total - 4, (len * 8) >>> 0);
  const h = Uint32Array.of(0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19);
  const w = new Uint32Array(64);
  const rot = (x, n) => (x >>> n) | (x << (32 - n));
  for (let o = 0; o < total; o += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(o + i * 4);
    for (let i = 16; i < 64; i++) {
      const a = w[i - 15], b = w[i - 2];
      w[i] = w[i - 16] + (rot(a, 7) ^ rot(a, 18) ^ (a >>> 3)) + w[i - 7] + (rot(b, 17) ^ rot(b, 19) ^ (b >>> 10));
    }
    let [a, b, c, d, e, f, g, k] = h;
    for (let i = 0; i < 64; i++) {
      const t1 = (k + (rot(e, 6) ^ rot(e, 11) ^ rot(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i]) >>> 0;
      const t2 = ((rot(a, 2) ^ rot(a, 13) ^ rot(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) >>> 0;
      [k, g, f, e, d, c, b, a] = [g, f, e, (d + t1) >>> 0, c, b, a, (t1 + t2) >>> 0];
    }
    [a, b, c, d, e, f, g, k].forEach((x, i) => (h[i] = (h[i] + x) >>> 0));
  }
  return Array.from(h, (x) => x.toString(16).padStart(8, "0")).join("");
}

const order = (a, b) => {
  for (let i = 0, j = 0; ; ) {
    const x = a.codePointAt(i), y = b.codePointAt(j);
    if (x === undefined || y === undefined) return x === undefined ? (y === undefined ? 0 : -1) : 1;
    if (x !== y) return x - y;
    i += x > 0xffff ? 2 : 1;
    j += y > 0xffff ? 2 : 1;
  }
};
const canon = (v) =>
  v === null || typeof v !== "object"
    ? JSON.stringify(v)
    : Array.isArray(v)
      ? `[${v.map(canon).join(",")}]`
      : `{${Object.keys(v).sort(order).map((k) => `${JSON.stringify(k)}:${canon(v[k])}`).join(",")}}`;

/** The `jsdigest` of a view: SHA-256 of its canonical JSON (keys in code point order, numbers as `String(x)`), first 16 bytes in hex. */
export const jsDigest = (v) => sha256(new TextEncoder().encode(canon(v))).slice(0, 32);

/** A refusal: the method's own reason (`code` null), or one of aio-rs's codes. */
export class Rejected extends Error {
  constructor(why, code = null) {
    super(why);
    this.name = "Rejected";
    this.code = code;
  }
}

/** A store of `get()`: `listen(f)` on each change, `subscribe(f)` too, with the value now (Svelte's store contract; Solid's `from` takes it). */
function store(get, listen) {
  return { get, listen, subscribe: (f) => (f(get()), listen(() => f(get()))) };
}

/** `get` narrowed by `select`, its value kept while `eq` says it is the same. */
function selected(get, listen, select, eq) {
  let src, val, has = false;
  const read = () => {
    const s = get();
    if (!has || s !== src) {
      const v = select(s);
      if (!has || !eq(v, val)) val = v;
      (src = s), (has = true);
    }
    return val;
  };
  return store(read, (f) => {
    let last = read();
    return listen(() => {
      const v = read();
      if (v !== last) (last = v), f(v);
    });
  });
}

const shallow = (a, b) => Object.keys(a).every((k) => Object.is(a[k], b[k])) && Object.keys(a).length === Object.keys(b).length;

/**
 * A client of the aio-rs server at `url` (default: the one that served
 * this page; in the desktop window, the app itself). `storage` (default the page's `localStorage`; `null`: none)
 * keeps each cell's copy and its waiting calls across reloads.
 */
export function connect({ url, WebSocket: WS, storage, app = "" } = {}) {
  // The desktop window's page may connect nowhere: its socket is the
  // window's own wire to the app (`aio::kit::use_js_client`).
  const window = url === undefined && WS === undefined ? globalThis.__aioSyncSocket : undefined;
  WS ??= window ?? globalThis.WebSocket;
  url ??= window ? "aio:window" : wsUrl(globalThis.location);
  if (storage === undefined) {
    try {
      storage = globalThis.localStorage ?? null;
    } catch {
      storage = null;
    }
  }
  const cells = new Map();
  let shared = null;

  // A socket: shared by every link when the server says `aio-mux`, else one per link.
  const socket = (pooled) => {
    if (pooled && shared && !shared.gone) return shared;
    const ws = new WS(url, MUX);
    const s = { ws, mux: null, gone: false, chans: new Map(), next: 1 };
    if (pooled) shared = s;
    ws.onopen = () => {
      s.mux = ws.protocol === MUX;
      if (!s.mux && shared === s) shared = null;
      for (const c of s.chans.values()) c.open();
    };
    ws.onmessage = (e) => {
      if (typeof e.data !== "string") return ws.close();
      if (!s.mux) return s.chans.forEach((c) => c.text(e.data));
      const m = /^(\d+)([:?.!])([\s\S]*)$/.exec(e.data);
      if (!m) return ws.close();
      const [, id, kind, rest] = m, c = s.chans.get(+id);
      if (!c) return kind === "?" && ws.send(`${id}!1000 `);
      c.heard();
      if (kind === "?") ws.send(`${id}.${rest}`);
      if (kind === ":") c.text(rest);
      if (kind === "!") s.chans.delete(+id), c.closed();
    };
    ws.onclose = () => {
      s.gone = true;
      if (shared === s) shared = null;
      const cs = [...s.chans.values()];
      s.chans.clear();
      cs.forEach((c) => c.closed());
    };
    return s;
  };

  // One link: `on.open()`, `on.text(t)`, `on.closed()`; `send(t)`, `close()`.
  const link = (on) => {
    let s, id, dead;
    const join = (pooled) => {
      s = socket(pooled);
      id = s.next++;
      s.chans.set(id, chan);
      if (s.mux !== null) queueMicrotask(chan.open);
    };
    const live = () => (clearTimeout(dead), s.mux && (dead = setTimeout(() => s.ws.close(), DEAD)));
    const chan = {
      open: () => {
        if (!s.mux && id > 1) return s.chans.delete(id), join(false); // an older server: a socket of its own
        live();
        on.open();
      },
      text: on.text,
      heard: live,
      closed: () => (clearTimeout(dead), on.closed()),
    };
    join(true);
    return {
      send: (t) => s.ws.readyState === 1 && (s.ws.send(s.mux ? `${id}:${t}` : t), true),
      close: () => {
        clearTimeout(dead);
        if (!s.chans.delete(id)) return;
        if (s.mux && s.chans.size) s.ws.send(`${id}!1000 `);
        else s.ws.close();
      },
    };
  };

  // Cell `{ id, v, paths? }`: one copy per id.
  const cell = (ref) => {
    if (!cells.has(ref.id)) cells.set(ref.id, open(ref, link, storage && { storage, key: `aio-rs:${app}:${ref.id}` }));
    return cells.get(ref.id);
  };
  return {
    url,
    cell,
    close: () => {
      cells.forEach((c) => c.close());
      cells.clear();
    },
  };
}

function open({ id, v, paths }, link, saved) {
  const doc = load(saved, id, v);
  let seq = doc?.seq ?? 0, view = doc?.view, user = doc?.user;
  // Calls not answered yet: {op, method, args, at, predict?, ok?, no?, done?} (`done`: the server's view has it)
  let pending = (doc?.pending ?? []).map((q) => ({ ...q }));
  let online = false, lastSync = null, closed = false, l = null, wait = 250, timer;
  const listeners = new Set(), refusals = new Set();
  // Predictions by method: `(state, ...args) => state`, for calls without their own.
  const predictors = {};
  // The calls this copy saw answered, oldest first.
  const done = new Set();
  let ready;
  const readied = new Promise((ok) => (ready = ok));
  if (doc) ready();

  // The confirmed view plus each waiting call's prediction (one that throws is skipped).
  const shown = () =>
    pending.reduce((s, q) => {
      const by = predictors[q.method];
      const f = q.predict ?? (by && ((st) => by(st, ...q.args)));
      if (!f || q.done) return s;
      try {
        return f(s);
      } catch {
        return s;
      }
    }, view);
  const status = () => {
    const n = pending.length;
    const conn = n >= PENDING_MAX ? "blocked" : !online ? "offline" : n > 0 ? "syncing" : "online";
    return { online, pending: n, conn, degraded: n * 5 > PENDING_MAX * 4 };
  };
  const tell = () => {
    const st = status();
    snap = { state: shown(), status: shallow(snap.status, st) ? snap.status : st };
    keep();
    listeners.forEach((f) => f());
  };
  let snap = { state: view, status: status() };
  const keep = () => {
    if (!saved || view === undefined) return;
    const d = { v, user, seq, view, pending: pending.map(({ op, method, args, at, done }) => ({ op, method, args, at, ...(done ? { done } : {}) })) };
    try {
      saved.storage.setItem(saved.key, JSON.stringify(d));
    } catch {
      // A full or refused storage: the copy goes on in this page.
    }
  };
  const send = (m) => l?.send(JSON.stringify(m));
  const msg = (q) => ({ t: "call", op: q.op, method: q.method, args: q.args });
  const settle = (q, f) => {
    pending = pending.filter((p) => p !== q);
    done.add(q.op);
    if (done.size > DONE_MAX) done.delete(done.values().next().value);
    f();
  };

  const take = (m) => {
    if (m.t === "state" || m.t === "patch" || m.t === "reply") lastSync = Date.now();
    switch (m.t) {
      case "state":
        if (m.user !== undefined && user !== undefined && m.user !== user) pending = pending.filter((q) => q.ok); // another user's calls are never sent as this one
        user = m.user;
        seq = m.seq;
        view = m.state;
        mine(m.op);
        ready();
        return tell();
      case "patch":
        if (m.seq <= seq) return;
        if (m.prev !== seq) return send({ t: "resync" }); // a gap: never guess
        try {
          const next = apply(view, m.ops);
          if (m.jsdigest !== undefined && jsDigest(next) !== m.jsdigest) throw 0; // drifted: take the server's whole view
          view = next;
        } catch {
          return send({ t: "resync" });
        }
        seq = m.seq;
        mine(m.op);
        return tell();
      case "reply": {
        const q = pending.find((p) => p.op === m.op);
        if (!q) return;
        if (m.err !== undefined) {
          const e = new Rejected(m.err, m.code ?? null);
          settle(q, () => q.no?.(e));
          refusals.forEach((f) => f(q.method, e));
        } else if (m.async) q.done = true;
        else settle(q, () => q.ok?.(m.ok));
        return tell();
      }
      case "end": {
        const q = pending.find((p) => p.op === m.op);
        if (!q) return;
        settle(q, () => (m.err !== undefined ? q.no?.(new Rejected(m.err, m.code ?? null)) : q.ok?.(m.ok)));
        return tell();
      }
    }
  };
  // The server's view now holds call `op`: its prediction is no longer added.
  const mine = (op) => {
    const q = op && pending.find((p) => p.op === op);
    if (q) q.done = true;
  };

  const hello = { t: "hello", cell: id, v, digest: "js", ...(paths?.length ? { paths } : {}) };
  const connect = () => {
    let first = true;
    l = link({
      open: () => send(hello),
      text: (t) => {
        let m;
        try {
          m = JSON.parse(t);
        } catch {
          return l.close(), down(true);
        }
        if (first) {
          first = false;
          if (m.t !== "state") {
            console.warn(`[aio-rs] ${id}: refused: ${m.why}${m.skew ? "" : "; not retrying"}`);
            l.close();
            return down(m.skew);
          }
          take(m);
          online = true;
          wait = 250;
          pending.forEach((q) => send(msg(q))); // the server applies each call id once
          return tell();
        }
        take(m);
      },
      closed: () => down(true),
    });
  };
  // The link dropped: tried again with backoff (250 ms doubling to 10 s, plus jitter).
  const down = (again) => {
    l = null;
    if (online) (online = false), tell();
    if (closed || !again) return;
    clearTimeout(timer);
    timer = setTimeout(connect, wait + Math.random() * 500);
    wait = Math.min(wait * 2, 10000);
  };
  connect();

  // A twin tab (another page of this app in this browser) kept calls this
  // copy lacks: they are shown and sent here too (the server applies each
  // call id once), so a call made in an offline tab goes out from a linked one.
  const twin = (e) => {
    if (e.key !== saved.key || !e.newValue) return;
    let d;
    try {
      d = JSON.parse(e.newValue);
    } catch {
      return;
    }
    if (d.v !== v || (user !== undefined && d.user !== user)) return;
    const have = new Set(pending.map((q) => q.op));
    const fresh = (d.pending ?? []).filter((q) => !have.has(q.op) && !done.has(q.op)).map((q) => ({ ...q }));
    if (!fresh.length) return;
    pending = [...pending, ...fresh].sort((a, b) => a.at - b.at || (a.op < b.op ? -1 : 1));
    fresh.forEach((q) => send(msg(q)));
    tell();
  };
  if (saved) globalThis.addEventListener?.("storage", twin);

  const get = () => snap.state;
  const listen = (f) => (listeners.add(f), () => listeners.delete(f));
  return {
    id,
    ready: readied,
    get,
    listen,
    subscribe: selected(get, listen, (s) => s, Object.is).subscribe,
    /** The server's state as last heard, without predictions. */
    confirmed: () => view,
    /** When the server's state last came (ms since the epoch), or `null`. */
    lastSync: () => lastSync,
    /** `f(state)` as a store; `undefined` (`f` not run) until the first state. */
    select: (f = (s) => s, eq = Object.is) => selected(get, listen, (s) => s === undefined ? undefined : f(s), eq),
    status: selected(() => snap, listen, (s) => s.status, Object.is),
    /**
     * Call `method` with `args`; the answer is the server's. `predict(state)`
     * shows the call at once, until the server's state has it (or refused
     * it). Offline, it waits and is sent on reconnect, once.
     */
    call(method, args = [], { predict } = {}) {
      if (pending.length >= PENDING_MAX)
        return Promise.reject(new Rejected(`blocked: ${PENDING_MAX} calls wait for the server`, "QUEUE_OVERFLOW"));
      return new Promise((ok, no) => {
        const q = { op: uuidv7(), method, args, at: Date.now(), predict, ok, no };
        pending = [...pending, q];
        send(msg(q));
        tell();
      });
    },
    /**
     * Predictions by method name, `{ add: (state, ...args) => state }`: a
     * call without its own `predict` shows by them, a call restored after a
     * reload or taken from a twin tab too.
     */
    predict(by) {
      Object.assign(predictors, by);
      tell();
    },
    /** `f(method, rejected)` when the server refuses a call. */
    onRefused: (f) => (refusals.add(f), () => refusals.delete(f)),
    close() {
      closed = true;
      if (saved) globalThis.removeEventListener?.("storage", twin);
      clearTimeout(timer);
      l?.close();
    },
  };
}

/** A kept copy of cell `id` at version `v`, if this storage has one. */
function load(saved, id, v) {
  if (!saved) return null;
  try {
    const d = JSON.parse(saved.storage.getItem(saved.key));
    return d && d.v === v ? d : null;
  } catch {
    return null;
  }
}
