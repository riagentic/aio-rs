// Solid on aio-rs's client: a cell (or the part a selector picks) as an
// accessor; a computation reading it runs again only when it changed.
import { from } from "solid-js";

/** An accessor of cell `cell`'s state, or of `select(state)` (kept while `eq` says it is the same). */
export const createCell = (cell, select, eq) => from(cell.select(select, eq));

/** An accessor of the cell's link: `{ online, pending, conn, degraded }`. */
export const createStatus = (cell) => from(cell.status);
