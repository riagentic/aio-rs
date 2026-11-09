// The islands of aio-rs-shell's `js_client` test, each bundled for an
// `Island`: an IIFE whose body returns its default export.
// `node test/e2e/build.mjs <out dir>`.
import { build } from "esbuild";
import { compile } from "svelte/compiler";
import { readFile } from "node:fs/promises";

const svelte = {
  name: "svelte",
  setup(b) {
    b.onLoad({ filter: /\.svelte$/ }, async (a) => ({
      contents: compile(await readFile(a.path, "utf8"), { filename: a.path, generate: "client" }).js.code,
      loader: "js",
    }));
  },
};

const out = process.argv[2];
await Promise.all(
  ["react.jsx", "solid.js", "svelte.js", "vue.js"].map((entry) =>
    build({
      entryPoints: [new URL(entry, import.meta.url).pathname],
      bundle: true,
      minify: true,
      format: "iife",
      globalName: "m",
      jsx: "automatic",
      conditions: ["browser"],
      define: { "process.env.NODE_ENV": '"production"', __VUE_OPTIONS_API__: "false", __VUE_PROD_DEVTOOLS__: "false", __VUE_PROD_HYDRATION_MISMATCH_DETAILS__: "false" },
      footer: { js: "return m.default" },
      outfile: `${out}/${entry.replace(/\.\w+$/, "")}.js`,
      plugins: [svelte],
      logLevel: "warning",
    }),
  ),
);
