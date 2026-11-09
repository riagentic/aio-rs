// Written by `ax tsgen fixture` from its source: its shared cells' state and
// their #[client] methods, for aio-rs's JS client. Run it again after a
// change; edits here are lost then.
import type { CellRef } from "aio-rs-client";

export type Board = { notes: Record<string, Note>; filter: Filter; shapes: Shape[]; owner: Id | null; seen: number | { $hidden: true } };
export type Filter = "All" | { Tag: string } | { Range: [number, number] } | { Due: { before: number } };
export type Id = string;
export type Note = { text: string; dueAt: number | null; tags?: string[] };
export type Shape = { kind: "circle"; r: number } | { kind: "dot" };

export const board: CellRef<{
  state: Board;
  methods: {
    add: { args: [text: string, due_at: number | null]; ok: string };
    clear: { args: []; ok: null };
    pick: { args: [filter: Filter]; ok: null };
  };
}> = { id: "board", v: 2 };
