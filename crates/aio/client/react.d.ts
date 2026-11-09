import type { Cell, Spec, Status } from "./index.js";

export function useCell<S extends Spec, R = S["state"]>(
  cell: Cell<S>,
  select?: (state: S["state"]) => R,
  eq?: (a: R, b: R) => boolean,
): R;
export function useStatus(cell: Cell): Status;
export function useCall<S extends Spec, M extends keyof S["methods"] & string>(
  cell: Cell<S>,
  method: M,
  predict?: (state: S["state"], ...args: S["methods"][M]["args"]) => S["state"],
): [
  (...args: S["methods"][M]["args"]) => Promise<S["methods"][M]["ok"] | undefined>,
  { pending: number; error: Error | null; data: S["methods"][M]["ok"] | undefined },
];
