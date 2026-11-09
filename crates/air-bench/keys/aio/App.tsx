import type { JSX } from "aio";
import { afterRender } from "aio/air";
import { nav, type Row, SIZES } from "./cell.ts";

const SHOWN = 60;
const money = (a: number) => (a < 0 ? "" : "+") + (a / 100).toFixed(2);

function Line(props: { p: number; i: number; row: Row; cur: boolean }): JSX.Element {
  const { p, i, row, cur } = props;
  return (
    <div class="row" id={`nav-p${p}-i${i}`} data-cursor={cur ? "1" : undefined}>
      <span class="name">{row.name}</span>
      <span class={row.amount < 0 ? "amt neg" : "amt pos"}>{money(row.amount)}</span>
      <span class="tag">{row.tag}</span>
    </div>
  );
}

function Panel(props: { p: number }): JSX.Element {
  const p = props.p, len = SIZES[p];
  const rows = p === 0 ? nav.left : nav.right;
  const c = nav.cursor[p], on = nav.active === p;
  const from = Math.min(Math.max(0, c - SHOWN / 2), Math.max(0, len - SHOWN));
  const shown = [];
  for (let i = from; i < Math.min(from + SHOWN, len); i++) {
    shown.push(<Line key={i} p={p} i={i} row={rows[i]} cur={on && c === i} />);
  }
  if (on) afterRender(() => document.querySelector(`#nav-panel-${p} [data-cursor]`)?.scrollIntoView({ block: "nearest" }));
  return <div class="panel" id={`nav-panel-${p}`}>{shown}</div>;
}

export default function App(): JSX.Element {
  const keys = (e: KeyboardEvent) => {
    const k = e.key;
    if (k === "ArrowDown") nav.step(1);
    else if (k === "ArrowUp") nav.step(-1);
    else if (k === "Home") nav.go(0);
    else if (k === "End") nav.go(1e9);
    else if (k === "Tab") nav.tab();
    else return;
    e.preventDefault();
  };
  return (
    <div id="nav" tabIndex={0} onKeyDown={keys}>
      <Panel p={0} />
      <Panel p={1} />
    </div>
  );
}
