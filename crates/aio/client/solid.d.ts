import type { Accessor } from "solid-js";
import type { Cell, Spec, Status } from "./index.js";

export function createCell<S extends Spec, R = S["state"]>(
  cell: Cell<S>,
  select?: (state: S["state"]) => R,
  eq?: (a: R, b: R) => boolean,
): Accessor<R>;
export function createStatus(cell: Cell): Accessor<Status>;
