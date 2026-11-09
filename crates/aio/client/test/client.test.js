// The client against a stand-in server (a fake WebSocket the test speaks
// for): `node --test test/`. A real server and browser: aio-rs-shell's
// `tests/js_client.rs`.
import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { DEAD, PENDING_MAX, apply, connect, jsDigest, sha256, uuidv7, wsUrl } from "../index.js";

// Each test's clients, closed after it: no timer outlives it.
const clients = [];
const open = (o) => (clients.push(connect(o)), clients.at(-1));
afterEach(() => clients.splice(0).forEach((c) => c.close()));

const tick = () => new Promise((r) => setTimeout(r, 0));
/** Queued opens run (with timers mocked, `tick` never would). */
const micro = () => Promise.resolve();

/** A socket the test answers for; `all` holds every one made. */
function fakes() {
  const all = [];
  class WS {
    constructor(url, proto) {
      Object.assign(this, { url, proto, sent: [], readyState: 0, protocol: "" });
      all.push(this);
    }
    accept(mux = true) {
      this.protocol = mux ? "aio-mux" : "";
      this.readyState = 1;
      this.onopen?.();
    }
    send(t) {
      this.sent.push(t);
    }
    close() {
      if (this.readyState === 3) return;
      this.readyState = 3;
      this.onclose?.();
    }
    /** A message of link `chan` (muxed), or of the socket. */
    push(m, chan = 1) {
      this.onmessage({ data: this.protocol ? `${chan}:${JSON.stringify(m)}` : JSON.stringify(m) });
    }
    /** What link `chan` sent, parsed. */
    of(chan = 1) {
      return this.sent
        .filter((t) => (this.protocol ? t.startsWith(`${chan}:`) : true))
        .map((t) => JSON.parse(this.protocol ? t.slice(String(chan).length + 1) : t));
    }
  }
  return { WS, all };
}

function memory() {
  const m = new Map();
  return { getItem: (k) => m.get(k) ?? null, setItem: (k, v) => m.set(k, v), m };
}

const state = (st, seq = 1, user = "ana") => ({ t: "state", seq, state: st, user, v: 1, can: ["paths", "seen"] });

/** A client on a fresh fake, its cell `tally` linked and given `st`. */
async function linked(st = { n: 0, items: [] }, opts = {}) {
  const { WS, all } = fakes();
  const c = open({ url: "ws://h/ws?wire=2", WebSocket: WS, storage: null, ...opts });
  const cell = c.cell({ id: "tally", v: 1 });
  all[0].accept();
  all[0].push(state(st));
  await cell.ready;
  return { c, cell, ws: all[0], all, WS };
}

test("a patch applies without touching its input, sharing what it didn't change", () => {
  const v = { a: { x: 1 }, b: { y: [1, 2, 3] }, s: "x".repeat(300) };
  const out = apply(v, [
    { op: "set", path: ["a", "x"], value: 2 },
    { op: "insert", path: ["b", "y", 1], value: 9 },
    { op: "del", path: ["b", "y", 3] },
    { op: "append", path: ["s"], value: "yz" },
    { op: "set", path: ["c"], value: true },
  ]);
  assert.deepEqual(out, { a: { x: 2 }, b: { y: [1, 9, 2] }, s: "x".repeat(300) + "yz", c: true });
  assert.deepEqual(v, { a: { x: 1 }, b: { y: [1, 2, 3] }, s: "x".repeat(300) });
  const untouched = { keep: { deep: [1] }, other: 1 };
  assert.equal(apply(untouched, [{ op: "set", path: ["other"], value: 2 }]).keep, untouched.keep);
  assert.deepEqual(apply({ a: 1 }, [{ op: "del", path: ["a"] }]), {});
  assert.deepEqual(apply(1, [{ op: "set", path: [], value: { b: 2 } }]), { b: 2 });
  for (const bad of [
    { op: "set", path: ["no", "x"], value: 1 },
    { op: "del", path: ["no"] },
    { op: "insert", path: ["a", 5], value: 1 },
    { op: "append", path: ["a"], value: "x" },
    { op: "set", path: ["a", 9], value: 1 },
    { op: "move", path: [] },
  ]) {
    assert.throws(() => apply({ a: [1] }, [bad]), undefined, JSON.stringify(bad));
  }
});

test("the digest is SHA-256 of the view as JavaScript holds it, the same the server computes", () => {
  for (const n of [0, 3, 55, 56, 63, 64, 65, 119, 1000]) {
    const b = Uint8Array.from({ length: n }, (_, i) => (i * 31) & 255);
    assert.equal(sha256(b), createHash("sha256").update(b).digest("hex"), `${n} bytes`);
  }
  // crates/aio/src/patch.rs pins the same digest of this text: 1.0 reads as 1, 2^53 + 1 as 2^53,
  // -0 as 0, keys in code point order ("￿" before the emoji, unlike JavaScript's sort)
  const fixture = String.raw`{"a":1.0,"big":9007199254740993,"e":1e21,"s":"q\"\n\u0001é","￿":0,"😀":-0.0,"f":1.5e-7,"m":[0.1,-2.5e-6,123e-20,18446744073709551615,[],{}]}`;
  assert.equal(jsDigest(JSON.parse(fixture)), "d927e64dc549c00e27bdf6711408285f");
});

test("a patch whose digest differs from the view it makes is dropped for the server's whole view", async () => {
  const { WS, all } = fakes();
  const c = open({ url: "ws://h/ws?wire=2", WebSocket: WS, storage: null });
  const cell = c.cell({ id: "tally", v: 1 });
  all[0].accept();
  all[0].push(state({ n: 1, s: "a" }, 1));
  await cell.ready;
  const set = (value) => [{ op: "set", path: ["n"], value }];
  all[0].push({ t: "patch", prev: 1, seq: 2, ops: set(2), jsdigest: jsDigest({ n: 2, s: "a" }) });
  assert.deepEqual(cell.get(), { n: 2, s: "a" });
  all[0].push({ t: "patch", prev: 2, seq: 3, ops: set(3), jsdigest: jsDigest({ n: 3, s: "b" }) });
  assert.deepEqual(cell.get(), { n: 2, s: "a" }, "a drifted view is never shown");
  assert.deepEqual(all[0].of().slice(1), [{ t: "resync" }]);
  all[0].push(state({ n: 3, s: "b" }, 3));
  assert.deepEqual(cell.get(), { n: 3, s: "b" });
  all[0].push({ t: "patch", prev: 3, seq: 4, ops: set(4) });
  assert.deepEqual(cell.get(), { n: 4, s: "b" }, "a server that sends no digest is followed as before");
  c.close();
});

test("a call id is a UUIDv7 of now, and the page's /ws is the server that served it", () => {
  const at = Date.UTC(2026, 0, 2, 3, 4, 5, 6);
  const id = uuidv7(at);
  assert.match(id, /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  assert.equal(parseInt(id.replace(/-/g, "").slice(0, 12), 16), at);
  assert.notEqual(uuidv7(at), id);
  assert.equal(wsUrl({ protocol: "https:", host: "h:1", search: "?a=1&token=k%2B1" }), "wss://h:1/ws?wire=2&token=k%2B1");
  assert.equal(wsUrl({ protocol: "http:", host: "h", search: "" }), "ws://h/ws?wire=2");
});

test("it says hello with the cell and version, follows patches and asks again at a gap", async () => {
  const { WS, all } = fakes();
  const c = open({ url: "ws://h/ws?wire=2", WebSocket: WS, storage: null });
  const cell = c.cell({ id: "tally", v: 3, paths: ["n"] });
  assert.equal(c.cell({ id: "tally", v: 3 }), cell, "one copy per cell");
  assert.equal(all[0].proto, "aio-mux");
  all[0].accept();
  assert.deepEqual(all[0].of(), [{ t: "hello", cell: "tally", v: 3, digest: "js", paths: ["n"] }]);
  assert.equal(cell.get(), undefined);
  all[0].push(state({ n: 1 }, 4));
  await cell.ready;
  assert.deepEqual(cell.get(), { n: 1 });
  all[0].push({ t: "patch", prev: 4, seq: 5, ops: [{ op: "set", path: ["n"], value: 2 }] });
  assert.deepEqual(cell.get(), { n: 2 });
  all[0].push({ t: "patch", prev: 4, seq: 5, ops: [{ op: "set", path: ["n"], value: 9 }] });
  assert.deepEqual(cell.get(), { n: 2 }, "an old patch is dropped");
  all[0].push({ t: "patch", prev: 7, seq: 8, ops: [] });
  all[0].push({ t: "patch", prev: 5, seq: 6, ops: [{ op: "del", path: ["zz"] }] });
  assert.deepEqual(all[0].of().slice(1), [{ t: "resync" }, { t: "resync" }], "a gap and a misfit: never guessed");
  all[0].push(state({ n: 7 }, 9));
  assert.deepEqual(cell.get(), { n: 7 });
  c.close();
});

test("a predicted call shows at once, and the server's answer settles it", async () => {
  const { cell, ws } = await linked();
  const add = (by) => (s) => ({ ...s, n: s.n + by });
  const p = cell.call("add", [2], { predict: add(2) });
  assert.deepEqual(cell.get(), { n: 2, items: [] });
  assert.deepEqual(cell.confirmed(), { n: 0, items: [] });
  const [call] = ws.of().slice(1);
  assert.deepEqual([call.t, call.method, call.args], ["call", "add", [2]]);
  assert.deepEqual(cell.status.get(), { online: true, pending: 1, conn: "syncing", degraded: false });
  assert.ok(cell.lastSync() > 0);
  // Its change, named for it, before its reply: shown once, not twice.
  ws.push({ t: "patch", prev: 1, seq: 2, op: call.op, ops: [{ op: "set", path: ["n"], value: 2 }] });
  assert.deepEqual(cell.get(), { n: 2, items: [] });
  ws.push({ t: "reply", op: call.op, ok: 2 });
  assert.equal(await p, 2);
  assert.equal(cell.status.get().conn, "online");
});

test("a refusal rolls the prediction back, rejects with its code and is told", async () => {
  const { cell, ws } = await linked();
  const told = [];
  cell.onRefused((m, e) => told.push([m, e.message, e.code]));
  const p = cell.call("reset", [], { predict: () => ({ n: -1, items: [] }) });
  assert.equal(cell.get().n, -1);
  const [call] = ws.of().slice(1);
  ws.push({ t: "reply", op: call.op, err: "not yours", code: "ACCESS_DENIED" });
  await assert.rejects(p, (e) => e.name === "Rejected" && e.code === "ACCESS_DENIED" && e.message === "not yours");
  assert.equal(cell.get().n, 0);
  assert.deepEqual(told, [["reset", "not yours", "ACCESS_DENIED"]]);
  const own = cell.call("add", [0]);
  ws.push({ t: "reply", op: ws.of().at(-1).op, err: "add something", code: null });
  await assert.rejects(own, (e) => e.code === null);
  assert.equal(cell.call("x", [], { predict: () => { throw new Error("bad prediction"); } }) instanceof Promise, true);
  assert.equal(cell.get().n, 0, "a prediction that throws shows nothing");
});

test("an async method answers at its end; its reply only says it started", async () => {
  const { cell, ws } = await linked();
  let answer;
  cell.call("fetch", ["x"]).then((v) => (answer = v));
  const op = ws.of().at(-1).op;
  ws.push({ t: "reply", op, ok: null, async: true });
  await tick();
  assert.equal(answer, undefined);
  assert.equal(cell.status.get().pending, 1);
  ws.push({ t: "end", op, ok: "done" });
  await tick();
  assert.equal(answer, "done");
  let why;
  cell.call("fetch", ["y"]).catch((e) => (why = e.message));
  const op2 = ws.of().at(-1).op;
  ws.push({ t: "reply", op: op2, ok: null, async: true });
  ws.push({ t: "end", op: op2, err: "timeout", code: null });
  await tick();
  assert.equal(why, "timeout");
});

test("a selector follows only its part", async () => {
  const { cell, ws } = await linked({ n: 0, items: [{ t: "a" }] });
  const items = cell.select((s) => s.items);
  const sizes = cell.select((s) => ({ len: s.items.length }), (a, b) => a.len === b.len);
  const seen = [], lens = [];
  items.listen((v) => seen.push(v));
  const first = sizes.get();
  sizes.subscribe((v) => lens.push(v));
  ws.push({ t: "patch", prev: 1, seq: 2, ops: [{ op: "set", path: ["n"], value: 5 }] });
  assert.equal(seen.length, 0, "n changed, not items");
  assert.equal(sizes.get(), first, "an equal selection keeps its value");
  ws.push({ t: "patch", prev: 2, seq: 3, ops: [{ op: "insert", path: ["items", 1], value: { t: "b" } }] });
  assert.deepEqual(seen, [[{ t: "a" }, { t: "b" }]]);
  assert.deepEqual(lens, [{ len: 1 }, { len: 2 }], "subscribe: now, then each change");
  const whole = [];
  const off = cell.subscribe((s) => whole.push(s.n));
  off();
  ws.push({ t: "patch", prev: 3, seq: 4, ops: [{ op: "set", path: ["n"], value: 6 }] });
  assert.deepEqual(whole, [5], "unsubscribed");
});

test("calls made offline wait, survive a reload and are sent once linked", async () => {
  const storage = memory();
  const { WS, all } = fakes();
  let c = open({ url: "ws://h/ws?wire=2", WebSocket: WS, storage, app: "demo" });
  let cell = c.cell({ id: "tally", v: 1 });
  all[0].accept();
  all[0].push(state({ n: 1 }));
  await cell.ready;
  all[0].close();
  assert.equal(cell.status.get().conn, "offline");
  const p = cell.call("add", [2], { predict: (s) => ({ n: s.n + 2 }) });
  assert.equal(cell.get().n, 3, "shown offline");
  const kept = JSON.parse(storage.m.get("aio-rs:demo:tally"));
  assert.deepEqual([kept.v, kept.user, kept.view, kept.pending.map((q) => q.method)], [1, "ana", { n: 1 }, ["add"]]);
  // The page reloads offline: the kept copy shows, its call waits.
  c.close();
  const before = all.length;
  c = open({ url: "ws://h/ws?wire=2", WebSocket: WS, storage, app: "demo" });
  cell = c.cell({ id: "tally", v: 1 });
  await cell.ready;
  assert.deepEqual(cell.get(), { n: 1 }, "a reloaded call shows once the server has it");
  assert.equal(cell.status.get().pending, 1);
  const ws = all[before];
  ws.accept();
  ws.push(state({ n: 1 }));
  const sent = ws.of().filter((m) => m.t === "call");
  assert.deepEqual(sent.map((m) => [m.method, m.args, m.op]), [["add", [2], kept.pending[0].op]]);
  ws.push({ t: "patch", prev: 1, seq: 2, op: sent[0].op, ops: [{ op: "set", path: ["n"], value: 3 }] });
  ws.push({ t: "reply", op: sent[0].op, ok: 3 });
  assert.equal(cell.get().n, 3);
  assert.equal(cell.status.get().pending, 0);
  assert.equal(JSON.parse(storage.m.get("aio-rs:demo:tally")).pending.length, 0);
  // The first page's promise waits for nothing now: its page is gone.
  void p;
  // A copy of another version is not taken.
  const other = open({ url: "ws://h/ws?wire=2", WebSocket: WS, storage, app: "demo" }).cell({ id: "tally", v: 2 });
  assert.equal(other.get(), undefined);
  // Another user's waiting calls are never sent as this one.
  c.close();
  const n = all.length;
  storage.m.set("aio-rs:demo:tally", JSON.stringify({ ...kept, user: "bo" }));
  const bo = open({ url: "ws://h/ws?wire=2", WebSocket: WS, storage, app: "demo" }).cell({ id: "tally", v: 1 });
  await bo.ready;
  all[n].accept();
  all[n].push(state({ n: 1 }, 1, "ana"));
  assert.equal(all[n].of().filter((m) => m.t === "call").length, 0);
  assert.equal(bo.status.get().pending, 0);
});

test("a full queue refuses the next call", async () => {
  const { cell, ws } = await linked();
  ws.close();
  for (let i = 0; i < PENDING_MAX; i++) cell.call("add", [1]);
  assert.equal(cell.status.get().conn, "blocked");
  assert.equal(cell.status.get().degraded, true);
  await assert.rejects(cell.call("add", [1]), (e) => e.code === "QUEUE_OVERFLOW");
});

test("every cell rides one socket; its pings are answered, a silent one is dropped and opened again", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { WS, all } = fakes();
  const c = open({ url: "ws://h/ws?wire=2", WebSocket: WS, storage: null });
  const a = c.cell({ id: "a", v: 1 });
  const b = c.cell({ id: "b", v: 1 });
  all[0].accept();
  await micro();
  assert.equal(all.length, 1, "one socket");
  assert.deepEqual([all[0].of(1)[0].cell, all[0].of(2)[0].cell], ["a", "b"]);
  all[0].push(state({ x: 1 }), 1);
  all[0].push(state({ y: 2 }), 2);
  assert.deepEqual([a.get(), b.get()], [{ x: 1 }, { y: 2 }]);
  all[0].onmessage({ data: "2?0a0b" });
  assert.equal(all[0].sent.at(-1), "2.0a0b");
  all[0].onmessage({ data: "9?00" });
  assert.equal(all[0].sent.at(-1), "9!1000 ", "a ping of no link closes it");
  b.close();
  assert.equal(all[0].sent.at(-1), "2!1000 ", "one link closed, the socket stays");
  t.mock.timers.tick(DEAD);
  assert.equal(all[0].readyState, 3, "nothing heard: dead");
  assert.equal(a.status.get().online, false);
  t.mock.timers.tick(1000);
  assert.equal(all.length, 2, "opened again");
  all[1].accept();
  await micro();
  assert.deepEqual(all[1].of(1)[0], { t: "hello", cell: "a", v: 1, digest: "js" });
  c.close();
  t.mock.timers.reset();
});

test("an older server, without one socket for all: a socket per cell", async () => {
  const { WS, all } = fakes();
  const c = open({ url: "ws://h/ws?wire=2", WebSocket: WS, storage: null });
  const a = c.cell({ id: "a", v: 1 });
  const b = c.cell({ id: "b", v: 1 });
  all[0].accept(false);
  assert.equal(all.length, 2);
  all[1].accept(false);
  assert.deepEqual([all[0].of()[0].cell, all[1].of()[0].cell], ["a", "b"]);
  all[1].push(state({ y: 1 }));
  assert.deepEqual([a.get(), b.get()], [undefined, { y: 1 }]);
  c.close();
  assert.deepEqual([all[0].readyState, all[1].readyState], [3, 3]);
});

test("a refusal is not tried again; another version of the cell is (it may be updated)", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const warn = console.warn;
  console.warn = () => {};
  try {
    const { WS, all } = fakes();
    open({ url: "u", WebSocket: WS, storage: null }).cell({ id: "a", v: 1 });
    all[0].accept();
    all[0].push({ t: "refused", why: "this server has cell b" });
    t.mock.timers.tick(20000);
    assert.equal(all.length, 1);
    open({ url: "u", WebSocket: WS, storage: null }).cell({ id: "a", v: 1 });
    all[1].accept();
    all[1].push({ t: "refused", why: "update the app", skew: true });
    t.mock.timers.tick(1000);
    assert.equal(all.length, 3);
  } finally {
    console.warn = warn;
    t.mock.timers.reset();
  }
});

test("a selector runs only once a state is held", async () => {
  const { WS, all } = fakes();
  const cell = open({ url: "u", WebSocket: WS, storage: null }).cell({ id: "a", v: 1 });
  const got = [];
  cell.select((s) => s.n).subscribe((n) => got.push(n));
  all[0].accept();
  all[0].push(state({ n: 4 }));
  assert.deepEqual(got, [undefined, 4]);
});

test("predictions by method show a call restored after a reload", async () => {
  const storage = memory();
  const { WS, all } = fakes();
  const add = { add: (s, by) => ({ ...s, n: s.n + by }) };
  let cell = open({ url: "u", WebSocket: WS, storage }).cell({ id: "tally", v: 1 });
  cell.predict(add);
  all[0].accept();
  all[0].push(state({ n: 1 }));
  await cell.ready;
  all[0].close();
  cell.call("add", [2]);
  assert.equal(cell.get().n, 3, "a call without its own predict shows by the method's");
  clients.splice(0).forEach((c) => c.close());
  // The page reloads (offline): the call comes back, and so does its prediction.
  cell = open({ url: "u", WebSocket: WS, storage }).cell({ id: "tally", v: 1 });
  await cell.ready;
  assert.equal(cell.get().n, 1, "nothing to predict it by yet");
  cell.predict(add);
  assert.equal(cell.get().n, 3, "shown again once its predictor is given");
});

test("twin tabs share their waiting calls: one offline, the other sends them", async () => {
  const storage = memory();
  const bus = new EventTarget();
  const [on, off] = [globalThis.addEventListener, globalThis.removeEventListener];
  globalThis.addEventListener = bus.addEventListener.bind(bus);
  globalThis.removeEventListener = bus.removeEventListener.bind(bus);
  // A browser tells the other tabs of each write.
  const shared = { getItem: storage.getItem, setItem: (k, val) => {
    const was = storage.getItem(k);
    storage.setItem(k, val);
    if (was !== val) bus.dispatchEvent(Object.assign(new Event("storage"), { key: k, newValue: val }));
  } };
  try {
    const { WS, all } = fakes();
    const online = open({ url: "u", WebSocket: WS, storage: shared }).cell({ id: "tally", v: 1 });
    online.predict({ add: (s, by) => ({ ...s, n: s.n + by }) });
    all[0].accept();
    all[0].push(state({ n: 1 }));
    await online.ready;
    // The other tab: never linked (its socket doesn't open), started from the kept copy.
    const offline = open({ url: "u", WebSocket: WS, storage: shared }).cell({ id: "tally", v: 1 });
    await offline.ready;
    offline.call("add", [5]);
    const sent = all[0].of().filter((m) => m.t === "call");
    assert.deepEqual(sent.map((m) => [m.method, m.args]), [["add", [5]]], "the linked tab sent it");
    assert.equal(online.get().n, 6, "and shows it by its predictor");
    all[0].push({ t: "patch", prev: 1, seq: 2, op: sent[0].op, ops: [{ op: "set", path: ["n"], value: 6 }] });
    all[0].push({ t: "reply", op: sent[0].op, ok: 6 });
    assert.equal(online.status.get().pending, 0);
    // The offline tab writes again (its own copy still holds the call): never put back.
    offline.call("add", [1]);
    assert.deepEqual(all[0].of().filter((m) => m.t === "call").map((m) => m.args), [[5], [1]]);
    assert.equal(online.get().n, 7);
  } finally {
    globalThis.addEventListener = on;
    globalThis.removeEventListener = off;
  }
});

test("a kept call the server's view already holds isn't shown twice after a reload", async () => {
  const storage = memory();
  const { WS, all } = fakes();
  const add = { add: (s, by) => ({ ...s, n: s.n + by }) };
  let cell = open({ url: "u", WebSocket: WS, storage }).cell({ id: "tally", v: 1 });
  cell.predict(add);
  all[0].accept();
  all[0].push(state({ n: 1 }));
  await cell.ready;
  cell.call("add", [2]);
  const op = all[0].of().at(-1).op;
  all[0].push({ t: "patch", prev: 1, seq: 2, op, ops: [{ op: "set", path: ["n"], value: 3 }] });
  // The page goes before the reply came.
  clients.splice(0).forEach((c) => c.close());
  cell = open({ url: "u", WebSocket: WS, storage }).cell({ id: "tally", v: 1 });
  cell.predict(add);
  await cell.ready;
  assert.equal(cell.get().n, 3);
});
