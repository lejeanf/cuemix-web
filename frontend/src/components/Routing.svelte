<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { groupSpans, routingInputs, routingOutputs } from '../lib/io';

  const st = $derived(store.state);
  const off = $derived(!store.ready);
  let hr = $state(-1);
  let hc = $state(-1);
  const ins = $derived(st ? routingInputs(st.routing.matrix.length, store.device) : []);
  const outs = $derived(st ? routingOutputs(st.topology.routing_outputs, store.device) : []);
  const outGroups = $derived(groupSpans(outs));
  /** Index of the first item of each group (gets a separator line). */
  const firstOut = $derived(new Set(outGroups.map((g) => g.start).filter((i) => i > 0)));
  const firstIn = $derived(new Set(groupSpans(ins).map((g) => g.start).filter((i) => i > 0)));
</script>

{#if st}
  <p class="hint">
    {S.routingHint}
    <span class="cur">{#if hr >= 0 && hc >= 0}{ins[hr]?.name} → {outs[hc]?.name}{/if}</span>
  </p>
  <div class="wrap" role="region" aria-label="Routing matrix" tabindex="-1">
    <table onpointerleave={() => ((hr = -1), (hc = -1))}>
      <thead>
        <tr class="groups">
          <th class="corner" scope="col">{S.routingCorner}</th>
          {#each outGroups as g (g.start)}
            <th scope="colgroup" colspan={g.span} class="grp" class:sep={g.start > 0}>{g.group}</th>
          {/each}
        </tr>
        <tr class="names">
          <th class="corner" scope="col"></th>
          {#each outs as out, o (o)}
            <th scope="col" class="out" class:hl={hc === o} class:sep={firstOut.has(o)} title={out.name}><span>{out.name}</span></th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each st.routing.matrix as row, i (i)}
          <tr class:hl={hr === i} class:sepr={firstIn.has(i)}>
            <th scope="row" class:hl={hr === i}>{ins[i]?.name}</th>
            {#each row as on, o (o)}
              <td class:hl={hr === i || hc === o} class:sep={firstOut.has(o)} onpointerenter={() => ((hr = i), (hc = o))}>
                <button
                  class="cell"
                  class:on
                  aria-pressed={on}
                  aria-label="Route {ins[i]?.name} to {outs[o]?.name}"
                  disabled={off}
                  data-unmapped={store.unmapped('route') ? '' : undefined}
                  title={store.hint('route')}
                  onclick={() => store.set({ param: 'route', input: i, output: o, on: !on }, true)}
                ></button>
              </td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .hint {
    margin: 0 0 6px;
    color: var(--text-dim);
    font-size: 12px;
    display: flex;
    gap: 16px;
  }
  .cur {
    color: var(--cyan);
    font-weight: 600;
  }
  .wrap {
    overflow: auto;
    max-height: calc(100% - 28px);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--card);
  }
  table {
    border-collapse: separate;
    border-spacing: 0;
  }
  th {
    position: sticky;
    background: var(--bg-alt);
    font-size: 11px;
    font-weight: 600;
    color: var(--text-dim);
    padding: 4px 6px;
    white-space: nowrap;
    z-index: 2;
  }
  thead tr.groups th {
    top: 0;
    height: 22px;
    box-sizing: border-box;
  }
  thead tr.names th {
    top: 22px;
  }
  th.grp {
    text-align: left;
    color: var(--text);
    border-bottom: 1px solid var(--border);
  }
  th.out {
    height: 64px;
    padding: 4px 0;
    vertical-align: bottom;
  }
  th.out span {
    display: inline-block;
    writing-mode: vertical-rl;
    transform: rotate(180deg);
  }
  .sep {
    border-left: 2px solid var(--border) !important;
  }
  tr.sepr > * {
    border-top: 2px solid var(--border);
  }
  tbody th {
    left: 0;
    text-align: left;
    z-index: 1;
  }
  .corner {
    left: 0;
    z-index: 3;
  }
  th.hl {
    color: var(--text);
    background: #232733;
  }
  td {
    padding: 0;
    border: 1px solid #23262e;
  }
  td.hl {
    background: #232733;
  }
  .cell {
    display: block;
    width: 26px;
    height: 26px;
    background: none;
    border: 0;
    cursor: pointer;
    position: relative;
  }
  .cell::after {
    content: '';
    position: absolute;
    inset: 7px;
    border-radius: 50%;
    border: 1px solid var(--border);
  }
  .cell.on::after {
    background: var(--cyan);
    border-color: var(--cyan);
  }
  .cell:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
</style>
