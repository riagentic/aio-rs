import type { ShallowRef } from "vue";
import type { Cell, Spec, Status } from "./index.js";

export function useCell<S extends Spec, R = S["state"]>(
  cell: Cell<S>,
  select?: (state: S["state"]) => R,
  eq?: (a: R, b: R) => boolean,
): Readonly<ShallowRef<R>>;
export function useStatus(cell: Cell): Readonly<ShallowRef<Status>>;
