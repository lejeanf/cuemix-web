<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { ProbeWindow, changedFrom, type ProbeStats } from '../lib/probe';

  // Diagnostic view: shows the MOTU's raw meter frame (prop 6000, one byte per
  // meter point) so the layout can be mapped by playing audio on one known
  // input/output at a time. Nothing here is interpreted yet.
  const win = new ProbeWindow(45);
  let stats = $state<ProbeStats>({ mean: [], min: [], max: [] });
  let baseline = $state<number[] | null>(null);
  let note = $state('');
  let copied = $state(false);

  $effect(() => {
    const raw = store.meterRaw;
    if (!raw) return;
    win.push(raw);
    stats = win.stats();
  });

  const raw = $derived(store.meterRaw ?? []);
  const patch = $derived(store.patch);
  /** Source points that currently carry signal (above -120 dB). */
  const live = (src: number) => src < 255 && (raw[src] ?? 255) < 240;
  const changed = $derived(new Set(baseline ? changedFrom(baseline, stats.mean) : []));
  const moving = (i: number) => (stats.max[i] ?? 0) - (stats.min[i] ?? 0) > 3;

  function setBaseline() {
    baseline = [...stats.mean];
  }
  async function copy() {
    const r = (a: number[]) => a.map((v) => Math.round(v));
    const data = {
      note,
      frames: win.count,
      mean: r(stats.mean),
      min: r(stats.min),
      max: r(stats.max),
      baseline: baseline ? r(baseline) : null,
      changed: [...changed],
    };
    try {
      await navigator.clipboard.writeText(JSON.stringify(data));
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      copied = false;
    }
  }
</script>

<div class="probe">
  <div class="bar">
    <strong>Meter probe</strong>
    <span class="dim">{raw.length} points · raw byte per point · window {win.count} frames</span>
    <button onclick={setBaseline} disabled={!raw.length}>Set baseline</button>
    {#if baseline}<button onclick={() => (baseline = null)}>Clear baseline</button>{/if}
    <input type="text" bind:value={note} placeholder="What is playing (e.g. music on Line 3-4 only)" aria-label="Note" />
    <button onclick={copy} disabled={!raw.length}>{copied ? 'Copied' : 'Copy snapshot'}</button>
  </div>
  <p class="dim help">
    1. Silence everything and press <em>Set baseline</em>. 2. Play audio on one known input or output. 3. Cells that changed turn
    blue; cells that flicker get an outline. 4. Write what is playing, press <em>Copy snapshot</em> and paste it in the chat.
  </p>
  <div class="patch">
    <strong>Patch (5010)</strong>
    {#if !patch}
      <span class="dim">Not read (demo mode or not sent by the card).</span>
    {:else}
      <span class="dim">destination &rarr; source point; blue = source has signal now</span>
      <div class="grid">
        {#each patch as src, dest (dest)}
          <div class="cell" class:idle={src === 255} class:chg={live(src)} title="destination {dest} from source {src}, meter byte {raw[src] ?? 'n/a'}">
            <span class="i">{dest}</span>
            <span class="v">{src === 255 ? '-' : src}</span>
          </div>
        {/each}
      </div>
    {/if}
  </div>
  {#if !raw.length}
    <p class="dim">No meter frames yet. They come from the MOTU once it is online (not in demo mode).</p>
  {:else}
    <div class="grid">
      {#each raw as v, i (i)}
        <div class="cell" class:idle={v === 255} class:chg={changed.has(i)} class:mov={moving(i)} title="#{i}: now {v}, mean {Math.round(stats.mean[i] ?? 0)}, min {stats.min[i]}, max {stats.max[i]}{baseline ? `, baseline ${Math.round(baseline[i] ?? 0)}` : ''}">
          <span class="i">{i}</span>
          <span class="v">{v}</span>
          <span class="b"><span style:width="{(v / 255) * 100}%"></span></span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .probe {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .bar input {
    flex: 1 1 220px;
    min-height: 26px;
    background: var(--field);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 0 8px;
    font: inherit;
    font-size: 12px;
  }
  button {
    min-height: 26px;
    padding: 0 10px;
    font-size: 12px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 5px;
    cursor: pointer;
  }
  .dim {
    color: var(--text-dim);
    font-size: 12px;
  }
  .patch {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .help {
    margin: 0;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(64px, 1fr));
    gap: 3px;
  }
  .cell {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0 4px;
    padding: 3px 5px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 5px;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .cell.idle {
    opacity: 0.35;
  }
  .cell.mov {
    border-color: var(--amber);
  }
  .cell.chg {
    background: color-mix(in srgb, var(--cyan) 30%, var(--card));
    opacity: 1;
  }
  .i {
    color: var(--text-dim);
  }
  .v {
    text-align: right;
    font-weight: 600;
  }
  .b {
    grid-column: 1 / -1;
    height: 3px;
    background: var(--field);
    border-radius: 2px;
    overflow: hidden;
  }
  .b span {
    display: block;
    height: 100%;
    background: var(--cyan);
  }
</style>
