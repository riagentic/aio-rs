// Key-to-paint bench app: the same app as aio-rs's in ../rs.
import { cell } from "aio";

export const SIZES = [1000, 200];
export type Row = { name: string; amount: number; tag: string };
const rows = (p: number): Row[] =>
  Array.from({ length: SIZES[p] }, (_, i) => ({
    name: `Account ${p}-${String(i).padStart(4, "0")}`,
    amount: ((i * 7919) % 100000) - 50000,
    tag: ["cash", "card", "bank", "loan", "fund"][i % 5],
  }));

export type State = { left: Row[]; right: Row[]; active: number; cursor: number[] };

export const nav = cell("nav", {
  scope: "client",
  state: { left: rows(0), right: rows(1), active: 0, cursor: [0, 0] } as State,
  methods: {
    go(s, to: number) {
      s.cursor[s.active] = Math.max(0, Math.min(SIZES[s.active] - 1, to));
    },
    step(s, d: number) {
      const to = s.cursor[s.active] + d;
      s.cursor[s.active] = Math.max(0, Math.min(SIZES[s.active] - 1, to));
    },
    tab(s) {
      s.active = 1 - s.active;
    },
  },
});
