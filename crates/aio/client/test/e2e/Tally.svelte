<script>
  // Svelte in an aio-rs page: the client's stores as they are (`$n`).
  let { cell } = $props();
  const n = cell.select((s) => s.n);
  let span, runs = 0;
  // Runs again only when `n` changed, not on every patch of the cell.
  $effect(() => {
    $n;
    span.dataset.runs = String(++runs);
  });
</script>

<span id="svelte-n" data-t="svelte-n" bind:this={span}>{$n ?? "…"}</span>
<button id="svelte-add" data-t="svelte-add" onclick={() => cell.call("add", [4])}>+4</button>
