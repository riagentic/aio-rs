// Vue on aio-rs's client: a cell (or the part a selector picks) as a
// shallow ref; what reads it renders again only when it changed.
import { onScopeDispose, shallowRef } from "vue";

const ref = (store) => {
  const r = shallowRef(store.get());
  onScopeDispose(store.listen((v) => (r.value = v)));
  return r;
};

/** A ref of cell `cell`'s state, or of `select(state)` (kept while `eq` says it is the same). */
export const useCell = (cell, select, eq) => ref(cell.select(select, eq));

/** A ref of the cell's link: `{ online, pending, conn, degraded }`. */
export const useStatus = (cell) => ref(cell.status);
