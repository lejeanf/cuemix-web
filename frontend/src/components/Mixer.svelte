<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { TRIM_MIN_DB } from '../lib/protocol';
  import { formatDb, trimDbToPos, trimPosToDb } from '../lib/fader';
  import Fader from './Fader.svelte';
  import Strip from './Strip.svelte';

  const st = $derived(store.state);
  const off = $derived(!store.ready);
</script>

{#if st}
  <div class="mixer">
    <div class="buses" role="tablist" aria-label="Mix bus">
      {#each st.mixes as _, i (i)}
        <button
          role="tab"
          aria-selected={store.bus === i}
          class:sel={store.bus === i}
          onclick={() => (store.bus = i)}>{store.mixName(i)}</button
        >
      {/each}
    </div>
    <div class="row">
      <div class="strips">
        {#each { length: st.topology.mixer_inputs } as _, ch (ch)}
          <Strip {ch} />
        {/each}
      </div>
      <section class="main" aria-label={S.main}>
        <div class="title">{S.main}</div>
        <Fader
          pos={trimDbToPos(st.main.db)}
          label="{S.main} level"
          valueText="{formatDb(st.main.db, 0)} dB"
          resetPos={1}
          mark={1}
          disabled={off}
          hint={store.hint('main_vol')}
          onchange={(p, f) => store.set({ param: 'main_vol', db: Math.round(trimPosToDb(p, TRIM_MIN_DB)) }, f)}
        />
        <div class="db">{formatDb(st.main.db, 0)} dB</div>
      </section>
    </div>
  </div>
{/if}

<style>
  .mixer {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    gap: 10px;
  }
  .buses {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    flex: none;
  }
  .buses button {
    flex: none;
    min-height: 36px;
    padding: 0 14px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 18px;
    cursor: pointer;
  }
  .buses button.sel {
    background: var(--cyan);
    border-color: var(--cyan);
    color: #111;
    font-weight: 600;
  }
  .row {
    flex: 1;
    min-height: 0;
    display: flex;
    gap: 10px;
  }
  .strips {
    flex: 1;
    display: flex;
    gap: 6px;
    overflow-x: auto;
    overflow-y: hidden;
    padding-bottom: 6px;
    min-width: 0;
    /* popovers inside strips must not be clipped vertically */
    align-items: stretch;
  }
  .main {
    flex: none;
    width: 96px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 8px 6px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .title {
    font-weight: 700;
  }
  .db {
    font-size: 12px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
</style>
