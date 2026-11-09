// Svelte in an aio-rs page (an `Island::js`), on the page's own cells.
import { mount, unmount } from "svelte";
import { connect } from "../../index.js";
import Tally from "./Tally.svelte";

export default (el) => {
  const cell = connect({ storage: null }).cell({ id: "tally", v: 1 });
  const c = mount(Tally, { target: el, props: { cell } });
  return () => (unmount(c), cell.close());
};
