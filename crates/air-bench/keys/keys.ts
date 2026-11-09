// Key to paint, aio vs aio-rs, on the bench app of this folder (rs/: aio-rs,
// aio/: aio's same app). The method of docs/aio-vs-aio-rs.md#key-to-paint:
// keys by CDP, a requestAnimationFrame recorder in the page (stamp: the
// frame's argument; ran: performance.now() in that frame), the CPU of the
// app's process tree (every thread's time on a CPU) less its idle rate, and
// the time the page's keydown handlers took (page) and air's edits in it (apply).
// One row per key to <work>/keys.csv;
// `python3 -I table.py <work>/keys.csv` makes the table.
//
//   deno run -A keys.ts <side> <runs> <label> [first run's number]   side: aio | rs
//
// Env: KEYS_WORK (a scratch folder: the runs' homes and data), KEYS_DISPLAY
// (an X display of its own, e.g. Xvfb's), KEYS_RUNTIME (an Electron 44
// runtime folder), KEYS_AIO (aio's app: its AppImage extracted, the AppRun),
// KEYS_RS (aio-rs's app: its release binary, built with `devtools: true`).
// The aio app: `am create keysaio --target=electron`, aio/ into its src/,
// `deno task compile`; aio's shared runtime store under <work>/h1 holds
// KEYS_RUNTIME's copy. The aio-rs app: `ax create keysrs`, rs/lib.rs as its
// src/lib.rs, rs/main.rs as app/src/main.rs, `cargo build --release`.
const need = (k: string) => Deno.env.get(k) ?? (() => { throw new Error(`${k} is not set`); })();
const T = need("KEYS_WORK");
const DISPLAY = need("KEYS_DISPLAY");
const PIN = ["taskset", "-c", "0-11"];
const now = () => performance.timeOrigin + performance.now();
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const load1 = () => Number(Deno.readTextFileSync("/proc/loadavg").split(" ")[0]);

function env(home: string, extra: Record<string, string>): Record<string, string> {
  return {
    PATH: Deno.env.get("PATH") ?? "/usr/bin:/bin", LANG: "C.UTF-8", HOME: home, DISPLAY, DBUS_SESSION_BUS_ADDRESS: "disabled:",
    XDG_CONFIG_HOME: `${home}/c`, XDG_DATA_HOME: `${home}/.local/share`, XDG_CACHE_HOME: `${home}/k`,
    XDG_RUNTIME_DIR: `${home}/r`, TMPDIR: `${home}/t`, ...extra,
  };
}
type Side = { cmd: string[]; env: Record<string, string>; cdp: number; data: string };
const RT = need("KEYS_RUNTIME");
function side(name: string): Side {
  const data = `${T}/run/${name}`;
  if (name === "aio") return { cmd: [need("KEYS_AIO"), "--cdp=9311"], cdp: 9311, data, env: env(`${T}/h1`, { AIO_APPS_DIR: data }) };
  return { cmd: [need("KEYS_RS"), "--cdp=9312"], cdp: 9312, data,
    env: env(`${T}/h2`, { AIORT_DATA: data, AIORT_CDP: "9312", AIORT_RUNTIME: RT }) };
}

function children(): Map<number, number[]> {
  const kids = new Map<number, number[]>();
  for (const e of Deno.readDirSync("/proc")) {
    const pid = Number(e.name);
    if (!pid) continue;
    try {
      const st = Deno.readTextFileSync(`/proc/${pid}/stat`);
      const ppid = Number(st.slice(st.lastIndexOf(")") + 2).split(" ")[1]);
      kids.set(ppid, [...(kids.get(ppid) ?? []), pid]);
    } catch { /* gone */ }
  }
  return kids;
}
function tree(root: number): number[] {
  const kids = children(), out = [root];
  for (let i = 0; i < out.length; i++) out.push(...(kids.get(out[i]) ?? []));
  return out;
}
function cpuMs(pids: number[]): number { // time on a CPU of every thread of the tree, ms
  let t = 0;
  for (const p of pids) {
    try {
      for (const e of Deno.readDirSync(`/proc/${p}/task`)) {
        t += Number(Deno.readTextFileSync(`/proc/${p}/task/${e.name}/schedstat`).split(" ")[0]) / 1e6;
      }
    } catch { /* gone */ }
  }
  return t;
}

class Cdp {
  id = 0;
  pending = new Map<number, (v: any) => void>();
  constructor(public ws: WebSocket) {
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (m.id && this.pending.has(m.id)) { this.pending.get(m.id)!(m); this.pending.delete(m.id); }
    };
  }
  static async page(port: number, until: number): Promise<Cdp> {
    while (now() < until) {
      try {
        const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
        const p = list.find((t: any) => t.type === "page" && !String(t.url).startsWith("devtools"));
        if (p) {
          const ws = new WebSocket(p.webSocketDebuggerUrl);
          await new Promise((ok, no) => { ws.onopen = ok; ws.onerror = no; });
          return new Cdp(ws);
        }
      } catch { /* not up yet */ }
      await sleep(20);
    }
    throw new Error(`no page on CDP ${port}`);
  }
  send(method: string, params: unknown = {}): Promise<any> {
    const id = ++this.id;
    return new Promise((r) => { this.pending.set(id, r); this.ws.send(JSON.stringify({ id, method, params })); });
  }
  fire(method: string, params: unknown = {}) { this.ws.send(JSON.stringify({ id: ++this.id, method, params })); }
  async eval(expr: string): Promise<any> {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.result?.exceptionDetails) throw new Error(`${expr.slice(0, 80)}: ${JSON.stringify(r.result.exceptionDetails).slice(0, 300)}`);
    return r.result?.result?.value;
  }
}
const KEYS: Record<string, [string, number]> = { ArrowDown: ["ArrowDown", 40], ArrowUp: ["ArrowUp", 38], End: ["End", 35], Home: ["Home", 36], Tab: ["Tab", 9] };
function key(c: Cdp, k: string, type: "keyDown" | "keyUp", wait = false) {
  const [code, vk] = KEYS[k];
  const p = { type: type === "keyDown" ? "rawKeyDown" : "keyUp", key: k, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk };
  return wait ? c.send("Input.dispatchKeyEvent", p) : (c.fire("Input.dispatchKeyEvent", p), Promise.resolve());
}
async function press(c: Cdp, k: string) { await key(c, k, "keyDown", true); await key(c, k, "keyUp", true); }

const RECORDER = `(() => { const H = window.__k = { keys: [], ups: [], frames: [] }; let last = null;
  let t0 = 0; H.work = []; H.apply = [];
  const painted = window.__aioPainted;
  if (painted) window.__aioPainted = (t) => { H.apply.push(performance.now() - t); painted(t); };
  addEventListener('keydown', e => { t0 = performance.now(); if (e.isTrusted) H.keys.push([e.timeStamp, e.key]); }, true);
  addEventListener('keydown', e => { if (e.isTrusted) H.work.push(performance.now() - t0); });
  addEventListener('keyup', e => { if (e.isTrusted) H.ups.push([e.timeStamp, e.key]); }, true);
  const loop = ts => { const ran = performance.now(); const c = document.querySelector('[data-cursor]'); const id = c ? c.id : '';
    if (id !== last) { H.frames.push([ts, ran, id]); last = id; } requestAnimationFrame(loop); };
  requestAnimationFrame(loop); return true; })()`;
const take = `(() => { const H = window.__k; const o = { keys: H.keys, ups: H.ups, frames: H.frames, work: H.work, apply: H.apply }; H.keys = []; H.ups = []; H.frames = []; H.work = []; H.apply = []; return o; })()`;
const at = (id: string) => { const m = id.match(/^nav-p(\d+)-i(\d+)$/); return m ? { p: Number(m[1]), i: Number(m[2]) } : null; };

const OUT = `${T}/keys.csv`;
function row(label: string, metric: string, name: string, run: number, stamp: number, ran: number, cpu: number, work = NaN, apply = NaN) {
  const r = (v: number) => (Number.isFinite(v) ? Math.round(v * 100) / 100 : "");
  const line = [label, metric, name, run, r(stamp), r(ran), r(cpu), load1(), r(work), r(apply)].join(",");
  console.log(line);
  Deno.writeTextFileSync(OUT, line + "\n", { append: true });
}

async function session(label: string, name: string, run: number) {
  const s = side(name);
  try { Deno.removeSync(s.data, { recursive: true }); } catch { /* none */ }
  Deno.mkdirSync(s.data, { recursive: true });
  for (const d of ["c", "k", "r", "t"]) Deno.mkdirSync(`${s.env.HOME}/${d}`, { recursive: true });
  const child = new Deno.Command(PIN[0], { args: [...PIN.slice(1), ...s.cmd], env: s.env, clearEnv: true, stdin: "null", stdout: "null", stderr: "null" }).spawn();
  try {
    const c = await Cdp.page(s.cdp, now() + 90000);
    for (let i = 0; ; i++) {
      let ok = false;
      try { ok = await c.eval(`!!document.querySelector('#nav [data-cursor]')`); } catch { /* navigating */ }
      if (ok) break;
      if (i > 600) throw new Error("the app never showed its cursor");
      await sleep(100);
    }
    await sleep(6000);
    await c.eval(`document.getElementById('nav').focus({ preventScroll: true }), true`);
    await c.eval(RECORDER);
    const pids = () => tree(child.pid);
    const idle0 = cpuMs(pids()); await sleep(3000); const idleRate = (cpuMs(pids()) - idle0) / 3000;
    await c.eval(take);
    const N = 60, GAP = 33;
    for (const k of ["ArrowDown", "ArrowUp"]) {
      const startIdx = at(await c.eval(`document.querySelector('[data-cursor]')?.id || ''`))?.i ?? 0;
      const cpu0 = cpuMs(pids()), tH = now();
      for (let i = 0; i < N; i++) { key(c, k, "keyDown"); await sleep(GAP); }
      key(c, k, "keyUp");
      await sleep(1500);
      const cpu = (cpuMs(pids()) - cpu0 - idleRate * (now() - tH)) / N;
      const r = await c.eval(take);
      const downs = r.keys.filter((x: any) => x[1] === k).map((x: any) => x[0]);
      const frames = r.frames.map((f: any) => [f[0], f[1], at(f[2])]).filter((f: any) => f[2]?.p === 0);
      const st: number[] = [], rn: number[] = [];
      downs.forEach((t: number, j: number) => {
        const want = k === "ArrowDown" ? startIdx + j + 1 : startIdx - j - 1;
        if (want < 0) return;
        const f = frames.find((f: any) => (k === "ArrowDown" ? f[2].i >= want : f[2].i <= want));
        if (f) { st.push(f[0] - t); rn.push(f[1] - t); }
      });
      const tag = k === "ArrowDown" ? "down" : "up";
      st.forEach((v, j) => row(label, `hold_${tag}`, name, run, v, rn[j], cpu, r.work[j]));
      await sleep(500);
    }
    for (const k of Array.from({ length: 40 }, (_, i) => (i % 2 ? "Home" : "End"))) {
      await sleep(800);
      await c.eval(take);
      const cpu0 = cpuMs(pids()), tK = now();
      await press(c, k);
      await sleep(1000);
      const cpu = cpuMs(pids()) - cpu0 - idleRate * (now() - tK);
      const r = await c.eval(take);
      const t = r.keys.find((x: any) => x[1] === k)?.[0];
      const f = r.frames.find((f: any) => { const a = at(f[2]); return a && a.p === 0 && (k === "Home" ? a.i === 0 : a.i >= 999); });
      row(label, `key_${k.toLowerCase()}`, name, run, f && t !== undefined ? f[0] - t : NaN, f && t !== undefined ? f[1] - t : NaN, cpu, r.work[0], (r.apply.length ? r.apply.reduce((a: number, b: number) => a + b, 0) : NaN));
    }
    for (let i = 0; i < 40; i++) {
      await sleep(600);
      const before = at(await c.eval(`document.querySelector('[data-cursor]')?.id || ''`))?.p;
      await c.eval(take);
      const cpu0 = cpuMs(pids()), tK = now();
      await press(c, "Tab");
      await sleep(600);
      const cpu = cpuMs(pids()) - cpu0 - idleRate * (now() - tK);
      const r = await c.eval(take);
      const t = r.keys.find((x: any) => x[1] === "Tab")?.[0];
      const f = r.frames.find((f: any) => { const a = at(f[2]); return a && a.p !== before; });
      row(label, "key_tab", name, run, f && t !== undefined ? f[0] - t : NaN, f && t !== undefined ? f[1] - t : NaN, cpu, r.work[0], (r.apply.length ? r.apply.reduce((a: number, b: number) => a + b, 0) : NaN));
    }
    c.ws.close();
  } finally {
    try { child.kill("SIGTERM"); } catch { /* gone */ }
    const t = setTimeout(() => { try { child.kill("SIGKILL"); } catch { /* gone */ } }, 20000);
    await child.status; clearTimeout(t);
    await sleep(1000);
  }
}

const [name, runs, label, first = "1"] = Deno.args;
for (let r = Number(first); r < Number(first) + Number(runs); r++) await session(label, name, r);
