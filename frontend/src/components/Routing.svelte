<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';

  const st = $derived(store.state);
  const off = $derived(!store.ready);
  let hr = $state(-1);
  let hc = $state(-1);
</script>

{#if st}
  <p class="hint">{S.routingHint}</p>
  <div class="wrap" role="region" aria-label="Routing matrix" tabindex="-1">
    <table onpointerleave={() => ((hr = -1), (hc = -1))}>
      <thead>
        <tr>
          <th class="corner" scope="col"></th>
          {#each { length: st.topology.routing_outputs } as _, o (o)}
            <th scope="col" class:hl={hc === o}>Out {o + 1}</th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each st.routing.matrix as row, i (i)}
          <tr class:hl={hr === i}>
            <th scope="row" class:hl={hr === i}>In {i + 1}</th>
            {#each row as on, o (o)}
              <td class:hl={hr === i || hc === o} onpointerenter={() => ((hr = i), (hc = o))}>
                <button
                  class="cell"
                  class:on
                  aria-pressed={on}
                  aria-label="Route input {i + 1} to output {o + 1}"
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
    margin: 0 0 8px;
    color: var(--text-dim);
    font-size: 12px;
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
    padding: 6px 8px;
    white-space: nowrap;
    z-index: 2;
  }
  thead th {
    top: 0;
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
    width: 34px;
    height: 34px;
    background: none;
    border: 0;
    cursor: pointer;
    position: relative;
  }
  .cell::after {
    content: '';
    position: absolute;
    inset: 9px;
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
