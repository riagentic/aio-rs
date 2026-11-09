// An aio app (from a copy of aio named by `AIO_DENO_SRC`) on its local socket
// only (aio's desktop default; no display: no window), its peers proven: `who`
// lists the ids `resolvePeer` was handed. Run by tests/compat_peer.rs
// (`deno run <copy of this> <aio src> <app id>`, beside an empty App.tsx).
const [src, appId] = Deno.args;
// A file URL of aio's mod.ts from a Unix or a Windows path.
const root = src.replaceAll("\\", "/").replace(/^(?!\/)/, "/");
const { aio, cell } = await import(`file://${root}/../mod.ts`);
const seen: string[] = [];
const svc = cell("svc", {
  state: { n: 0 },
  visible: "all",
  // A proven app only: an anonymous call is refused.
  access: (u: { role?: string } | null) => u?.role === "app",
  methods: {
    add(s: { n: number }, x: number) {
      s.n += x;
      return s.n;
    },
    who() {
      return seen;
    },
  },
});
await aio.run({
  appId,
  cells: [svc],
  client: "electron",
  persist: false,
  resolvePeer: (p: { app: string }) => {
    seen.push(p.app);
    return { id: p.app, role: "app" };
  },
});
