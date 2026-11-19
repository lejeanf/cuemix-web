<script lang="ts">
  import { untrack } from 'svelte';
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { LABEL_NAME_MAX } from '../lib/protocol';
  import { isValidLabelName } from '../lib/validate';

  let { ch, onclose }: { ch: number; onclose: () => void } = $props();

  const PALETTE = ['#EF4444', '#F59E0B', '#EAB308', '#10B981', '#3B93E0', '#6366F1', '#8B5CF6', '#EC4899', '#9CA3AF'];

  const cur = untrack(() => store.label(ch));
  let name = $state(cur?.name ?? '');
  let color = $state<string | null>(cur?.color ?? null);
  const valid = $derived(isValidLabelName(name));

  function apply() {
    if (!valid) return;
    store.setLabel(ch, { name, color });
    onclose();
  }
  function clear() {
    store.setLabel(ch, null);
    onclose();
  }
  function onkey(e: KeyboardEvent) {
    if (e.key === 'Enter') apply();
    else if (e.key === 'Escape') onclose();
  }
</script>

<div class="pop" role="dialog" aria-label={S.rename}>
  <!-- svelte-ignore a11y_autofocus -->
  <input
    type="text"
    maxlength={LABEL_NAME_MAX}
    bind:value={name}
    aria-label={S.rename}
    aria-invalid={!valid}
    onkeydown={onkey}
    autofocus
  />
  {#if !valid}<small class="err">{S.labelInvalid}</small>{/if}
  <div class="sw" role="group" aria-label={S.labelColor}>
    {#each PALETTE as c (c)}
      <button
        class="swatch"
        class:sel={color === c}
        style:background={c}
        aria-label="{S.labelColor} {c}"
        aria-pressed={color === c}
        onclick={() => (color = color === c ? null : c)}
      ></button>
    {/each}
  </div>
  <div class="row">
    <button onclick={apply} disabled={!valid}>{S.apply}</button>
    <button onclick={clear}>{S.clearLabel}</button>
    <button onclick={onclose}>{S.cancel}</button>
  </div>
</div>

<style>
  .pop {
    /* fixed: the strip container scrolls horizontally and would clip an absolute popover */
    position: fixed;
    z-index: 50;
    top: calc(72px + env(safe-area-inset-top, 0px));
    left: 50%;
    transform: translateX(-50%);
    width: min(260px, 92vw);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 8px 24px #000a;
  }
  input {
    min-height: 32px;
    background: #0d0e11;
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 8px;
    font: inherit;
  }
  .err {
    color: var(--red);
  }
  .sw {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .swatch {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    border: 2px solid transparent;
    cursor: pointer;
  }
  .swatch.sel {
    border-color: var(--text);
  }
  .row {
    display: flex;
    gap: 6px;
  }
  .row button {
    flex: 1;
    min-height: 32px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: pointer;
  }
  .row button:disabled {
    opacity: 0.4;
  }
</style>
