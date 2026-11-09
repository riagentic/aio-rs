// React hooks on aio-rs's client: a component follows a cell (or the part
// a selector picks) and calls its methods; React re-renders it only when
// what it reads changed.
import { useCallback, useRef, useState, useSyncExternalStore } from "react";

/** Cell `cell`'s state, or `select(state)`, kept while `eq` says it is the same; `undefined` until the first state. */
export function useCell(cell, select = (s) => s, eq = Object.is) {
  const memo = useRef(null);
  const get = () => {
    const s = cell.get(), m = memo.current;
    if (m && m.s === s && m.f === select) return m.v;
    const v = s === undefined ? undefined : select(s);
    memo.current = { s, f: select, v: m && eq(m.v, v) ? m.v : v };
    return memo.current.v;
  };
  return useSyncExternalStore(cell.listen, get, get);
}

/** The cell's link: `{ online, pending, conn, degraded }`. */
export function useStatus(cell) {
  return useSyncExternalStore(cell.status.listen, cell.status.get, cell.status.get);
}

/**
 * `[call, { pending, error, data }]`: `call(...args)` calls `method`, shown
 * at once by `predict(state, ...args)` when given; it resolves to the
 * server's answer, or to `undefined` when refused (`error` says why).
 */
export function useCall(cell, method, predict) {
  const [st, set] = useState({ pending: 0, error: null, data: undefined });
  const call = useCallback(
    (...args) => {
      set((s) => ({ ...s, pending: s.pending + 1 }));
      const opts = predict ? { predict: (s) => predict(s, ...args) } : {};
      return cell.call(method, args, opts).then(
        (data) => (set((s) => ({ pending: s.pending - 1, error: null, data })), data),
        (error) => set((s) => ({ pending: s.pending - 1, error, data: undefined })),
      );
    },
    [cell, method, predict],
  );
  return [call, st];
}
