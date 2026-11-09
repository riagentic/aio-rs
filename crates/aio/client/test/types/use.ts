// The generated types at work (`tsc -p test`): what the client and its
// hooks accept for `fixture.ts`'s cell, and, marked `@ts-expect-error`,
// what they refuse. A mark that no longer errs fails the check too.
import { connect } from "aio-rs-client";
import { useCall, useCell } from "aio-rs-client/react";
import { useCell as useVueCell } from "aio-rs-client/vue";
import { board, type Filter } from "./fixture.js";

const cell = connect().cell(board);
const id: Promise<string> = cell.call("add", ["milk", null]);
cell.call("clear", []);
cell.call("pick", [{ Due: { before: 3 } }]);
cell.call("add", ["milk", 5], { predict: (s) => ({ ...s, filter: "All" }) });
const notes: number = Object.keys(cell.get().notes).length;
const due = cell.select((s) => s.notes["a"]?.dueAt).get();
cell.predict({ add: (s, text, due) => ({ ...s, notes: { ...s.notes, [text]: { text, dueAt: due } } }) });
// @ts-expect-error: a predictor of no such method
cell.predict({ wipe: (s) => s });
const filter = useVueCell(cell, (s) => s.filter);
const f: Filter = filter.value;
// @ts-expect-error: no such method (`wipe` isn't #[client])
cell.call("wipe", []);
// @ts-expect-error: an argument of the wrong type
cell.call("add", [1, null]);
// @ts-expect-error: an argument missing
cell.call("add", ["milk"]);
// @ts-expect-error: not a Filter
cell.call("pick", ["Some"]);
// @ts-expect-error: `#[server]` fields never reach a client
cell.get().secret;
// @ts-expect-error: a renamed field is read by its wire name
cell.get().notes["a"].due_at;

export function Count() {
  const n = useCell(cell, (s) => Object.keys(s.notes).length);
  const [add, { pending }] = useCall(cell, "add", (s, text) => ({ ...s, notes: { ...s.notes, x: { text, dueAt: null } } }));
  // @ts-expect-error: add takes (text, due_at)
  add(3);
  const sum: number = n + pending;
  return [add("milk", null), sum];
}
export { id, notes, due, f };
