// Solid in an aio-rs page (an `Island::js`), on the page's own cells
// through aio-rs's client: bundled by aio-rs-shell's `js_client` test.
import { createEffect, createRoot } from "solid-js";
import { connect } from "../../index.js";
import { createCell } from "../../solid.js";

export default (el) => {
  const cell = connect({ storage: null }).cell({ id: "tally", v: 1 });
  return createRoot((dispose) => {
    const n = createCell(cell, (s) => s.n);
    const span = el.appendChild(document.createElement("span"));
    span.id = span.dataset.t = "solid-n";
    let runs = 0;
    // Runs again only when `n` changed, not on every patch of the cell.
    createEffect(() => {
      span.textContent = String(n() ?? "…");
      span.dataset.runs = ++runs;
    });
    const b = el.appendChild(document.createElement("button"));
    b.id = b.dataset.t = "solid-add";
    b.onclick = () => cell.call("add", [3], { predict: (s) => ({ ...s, n: s.n + 3 }) });
    return () => (dispose(), cell.close());
  });
};
