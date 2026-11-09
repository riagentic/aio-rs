// Vue in an aio-rs page (an `Island::js`), on the page's own cells.
import { createApp, h } from "vue";
import { connect } from "../../index.js";
import { useCell } from "../../vue.js";

export default (el) => {
  const cell = connect({ storage: null }).cell({ id: "tally", v: 1 });
  let renders = 0;
  const app = createApp({
    setup() {
      const n = useCell(cell, (s) => s.n);
      return () => [
        h("span", { id: "vue-n", "data-t": "vue-n", "data-renders": ++renders }, String(n.value ?? "…")),
        h("button", { id: "vue-add", "data-t": "vue-add", onClick: () => cell.call("add", [5]) }, "+5"),
      ];
    },
  });
  app.mount(el);
  return () => (app.unmount(), cell.close());
};
