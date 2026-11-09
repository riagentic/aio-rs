/** A cell's types: its state as a client sees it, its methods' arguments and answers (`ax tsgen` writes them). */
export interface Spec {
  state: unknown;
  methods: { [method: string]: { args: unknown[]; ok: unknown } };
}

/** A cell by id and version (`#[cell(version = n)]`, 1 unless set), typed by `S`. */
export interface CellRef<S extends Spec = Spec> {
  id: string;
  v: number;
  /** The parts held (`"items"`, `"feeds.*.title"`); none: all of it. */
  paths?: string[];
  /** Carries `S` only. */
  readonly spec?: S;
}

/** A value that changes: Svelte's store contract, which Solid's `from` takes too. */
export interface Store<T> {
  get(): T;
  /** `f` after each change; returns its unsubscribe. */
  listen(f: (v: T) => void): () => void;
  /** `f` with the value now, then after each change. */
  subscribe(f: (v: T) => void): () => void;
}

export interface Status {
  online: boolean;
  pending: number;
  conn: "online" | "offline" | "syncing" | "blocked";
  /** More than 80% of `PENDING_MAX` calls wait. */
  degraded: boolean;
}

export type Code =
  | "ACCESS_DENIED"
  | "ACTION_REFUSED"
  | "CONNECTION_LOST"
  | "TX_CONFLICT"
  | "REDUCE_ERROR"
  | "INIT_ERROR"
  | "QUEUE_OVERFLOW"
  | "DISPATCH_CLOSED"
  | "DISPATCH_ABORTED"
  | "DISPATCH_LOOP"
  | "PERSIST_ERROR";

export class Rejected extends Error {
  /** `null`: the method's own refusal (`ensure!`, `reject!`). */
  code: Code | (string & {}) | null;
  constructor(why: string, code?: string | null);
}

export interface Cell<S extends Spec = Spec> {
  readonly id: string;
  /** Settles once a state is held (the server's, or the copy kept offline). */
  readonly ready: Promise<void>;
  /** What to show: the server's state plus the waiting calls' predictions; `undefined` until `ready`. */
  get(): S["state"];
  listen(f: () => void): () => void;
  subscribe(f: (state: S["state"]) => void): () => void;
  /** The server's state as last heard, without predictions. */
  confirmed(): S["state"];
  /** When the server's state last came (ms since the epoch), or `null`: not in this page's run. */
  lastSync(): number | null;
  /** `f(state)` as a store; `undefined` (`f` not run) until `ready`. */
  select<R = S["state"]>(f?: (state: S["state"]) => R, eq?: (a: R, b: R) => boolean): Store<R>;
  readonly status: Store<Status>;
  /** The server's answer; `predict` shows the call at once. Offline it waits and is sent on reconnect, once. */
  call<M extends keyof S["methods"] & string>(
    method: M,
    args: S["methods"][M]["args"],
    opts?: { predict?: (state: S["state"]) => S["state"] },
  ): Promise<S["methods"][M]["ok"]>;
  /** Predictions by method name: a call without its own `predict`, one restored after a reload or taken from a twin tab, shows by them. */
  predict(by: { [M in keyof S["methods"]]?: (state: S["state"], ...args: S["methods"][M]["args"]) => S["state"] }): void;
  onRefused(f: (method: string, why: Rejected) => void): () => void;
  close(): void;
}

export interface Client {
  readonly url: string;
  /** The copy of cell `ref` (one per id). */
  cell<S extends Spec>(ref: CellRef<S>): Cell<S>;
  close(): void;
}

export interface Options {
  /** The server's `/ws` (default: the page's server, its `?token=` passed on). */
  url?: string;
  WebSocket?: typeof WebSocket;
  /** Keeps each copy and its waiting calls (default `localStorage`; `null`: none). */
  storage?: Pick<Storage, "getItem" | "setItem"> | null;
  /** Names this app's copies in `storage`. */
  app?: string;
}

export const PENDING_MAX: 500;
export const DEAD: 5000;
export function connect(opts?: Options): Client;
export function wsUrl(loc: { protocol: string; host: string; search: string }): string;
export function uuidv7(now?: number): string;
export type Step = string | number;
export type Op =
  | { op: "set"; path: Step[]; value: unknown }
  | { op: "del"; path: Step[] }
  | { op: "insert"; path: Step[]; value: unknown }
  | { op: "append"; path: Step[]; value: string };
export function apply<T>(view: T, ops: Op[]): T;
/** SHA-256 of `bytes`, in hex. */
export function sha256(bytes: Uint8Array): string;
/** The `jsdigest` the server sends with a patch: SHA-256 of `view`'s canonical JSON (keys in code point order, numbers as `String(x)`), first 16 bytes in hex. */
export function jsDigest(view: unknown): string;
