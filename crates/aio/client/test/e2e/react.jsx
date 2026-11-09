// A React tree in an aio-rs page (an `Island::react`), on the page's own
// cells through aio-rs's client: bundled by aio-rs-shell's `js_client` test.
import * as react from "react";
import * as reactDomClient from "react-dom/client";
import { connect } from "../../index.js";
import { useCall, useCell, useStatus } from "../../react.js";

const cell = connect({ storage: null }).cell({ id: "tally", v: 1 });

function Tally() {
  const renders = react.useRef(0);
  renders.current++;
  const n = useCell(cell, (s) => s.n);
  const st = useStatus(cell);
  const [add] = useCall(cell, "add", (s, by) => ({ ...s, n: s.n + by }));
  const [reset, r] = useCall(cell, "reset", (s) => ({ ...s, n: 0 }));
  return (
    <div>
      <span id="react-n" data-t="react-n" data-renders={renders.current}>{n ?? "…"}</span>
      <span id="react-conn" data-t="react-conn">{st.conn}</span>
      <span id="react-err" data-t="react-err">{r.error?.code ?? ""}</span>
      <button id="react-add" data-t="react-add" onClick={() => add(2)}>+2</button>
      <button id="react-reset" data-t="react-reset" onClick={() => reset()}>reset</button>
    </div>
  );
}

export default { react, reactDomClient, component: Tally };
