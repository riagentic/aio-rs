// aio's own CRDT sync client (`src/sync/sync-engine.ts`, from a copy of aio
// named by `AIO_DENO_SRC`) against an aio-rs app's `/ws`: two pages, the cells of
// tests/compat_wire.rs's `Port`. Run by `aios_own_sync_client_converges_with_an_aio_rs_app`
// (`deno run <this> <aio src> <addr>`); prints what each page holds, as JSON.
const [src, addr] = Deno.args;
const { createSyncEngine } = await import(`file://${src}/sync/sync-engine.ts`);
const { createOpBuffer, createMemoryStorage } = await import(`file://${src}/sync/op-buffer.ts`);
const { enc } = await import(`file://${src}/protocol/envelope.ts`);
const { REDUCER_FAILED } = await import(`file://${src}/sync/rebase.ts`);

type Cell = Record<string, unknown>;
const config = { merge: {}, identity: {}, offline: { retention: "4h" } };

// The cells' methods as aio's page runs them (the server's are tests/compat_wire.rs's).
function reduce(state: Cell, action: string, payload: unknown, cell?: string) {
  const args = (payload as { args?: unknown[] })?.args ?? [];
  if (cell === "purse" && action === "earn") return { ...state, coins: ((state.coins as number) ?? 0) + (args[0] as number) };
  if (cell === "settings" && action === "theme") return { ...state, theme: args[0] };
  return REDUCER_FAILED;
}

async function page(id: string) {
  const ws = new WebSocket(`ws://${addr}/ws?token=secret`);
  const confirmed: Record<string, Cell> = {};
  const shown: Record<string, Cell> = {};
  const frames: string[] = [];
  const engine = createSyncEngine({
    clientId: id,
    cells: { purse: config, settings: config },
    buffer: createOpBuffer(createMemoryStorage()),
    send: (m: string) => ws.readyState === WebSocket.OPEN && ws.send(m),
    reducer: reduce,
    getConfirmedState: () => confirmed,
    setConfirmedState: (c: string, s: Cell) => (confirmed[c] = s),
    onStateUpdate: (c: string, s: Cell) => (shown[c] = s),
    log: { warn: (m: string) => console.error(`[${id}] ${m}`) },
  });
  const opened = new Promise<void>((ok) => (ws.onopen = () => ok()));
  ws.onmessage = (e) => {
    const f = JSON.parse(e.data);
    frames.push(f.t);
    const d = f.d;
    switch (f.t) {
      case "proto":
        return ws.send(enc("proto", { v: 3, min: 3 }));
      case "sync-ack":
        return void engine.handleAck(d.cell, d.opId, d.serverHlc, d.serverTs, d.serverNow);
      case "op-rejected":
        return void engine.handleRejection(d.cell, d.opId, d.reason, d.serverNow);
      case "op":
        return void engine.handleRemoteOp(d);
      case "sync-res":
        return void engine.handleSyncResponse(d);
      case "sync-err":
        return void setTimeout(() => engine.requestSync(), 50);
    }
  };
  await opened;
  return { ws, engine, confirmed, shown, frames };
}

type Page = Awaited<ReturnType<typeof page>>;
const until = async (what: string, ok: () => boolean) => {
  for (let i = 0; i < 500 && !ok(); i++) await new Promise((r) => setTimeout(r, 10));
  if (!ok()) throw new Error(`timed out: ${what}`);
};
const pending = (p: Page) => p.engine.getStatus("purse").pending + p.engine.getStatus("settings").pending;

const a = await page("client-a"), b = await page("client-b");
await a.engine.requestSync();
await b.engine.requestSync();
await until("both caught up", () => a.confirmed.purse !== undefined && b.confirmed.purse !== undefined);
const start = a.confirmed.purse.coins as number;

// Online: a's change is shown at once, acked, and reaches b as an op.
await a.engine.handleLocalAction("purse", "earn", { args: [5] });
const optimistic = a.shown.purse?.coins;
await until("a acked", () => pending(a) === 0);
await until("b has a's op", () => b.confirmed.purse.coins === start + 5);

// Offline: two changes wait in a's queue, then go in its catch-up.
a.engine.setOnline(false);
await a.engine.handleLocalAction("purse", "earn", { args: [2] });
await a.engine.handleLocalAction("settings", "theme", { args: ["dark"] });
const queued = pending(a);
a.engine.setOnline(true);
await a.engine.requestSync();
await until("a's queue confirmed", () => pending(a) === 0);
await until("b has both", () => b.confirmed.purse.coins === start + 7 && b.confirmed.settings?.theme === "dark");

// The server refuses: a's page goes back to what the server holds.
await a.engine.handleLocalAction("settings", "theme", { args: [""] });
await until("refused", () => pending(a) === 0);

console.log(JSON.stringify({
  start,
  optimistic,
  queued,
  a: { confirmed: a.confirmed, shown: a.shown, rejected: a.frames.filter((t) => t === "op-rejected").length },
  b: { confirmed: b.confirmed, ops: b.frames.filter((t) => t === "op").length },
}));
a.engine.dispose();
b.engine.dispose();
a.ws.close();
b.ws.close();
