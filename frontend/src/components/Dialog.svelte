<script lang="ts">
  import { onMount, type Snippet } from 'svelte';

  // Shared modal shell of the ⋯ menu panels (speaker plug, OSC, version): backdrop, focus
  // trap, Escape, focus restore. The panel content brings its own buttons and state.
  let {
    title,
    titleId,
    dirty = false,
    onclose,
    children,
  }: {
    title: string;
    titleId: string;
    /** Unsaved edits: a click on the backdrop must not discard them. */
    dirty?: boolean;
    onclose: () => void;
    children: Snippet;
  } = $props();

  // The backdrop closes the panel only for a full click on it (press and release), never mid-edit.
  let downOnBack = false;
  let dlg: HTMLElement;

  onMount(() => {
    const prev = document.activeElement as HTMLElement | null;
    focusables()[0]?.focus();
    return () => {
      // The menu item that opened us is gone by now: go back to the menu button. Later, so the
      // page behind (inert while we are open) can take focus again.
      setTimeout(() => {
        const to = prev && prev.isConnected && prev !== document.body ? prev : document.querySelector<HTMLElement>('[data-more-menu]');
        to?.focus?.();
      }, 0);
    };
  });

  const focusables = () =>
    dlg ? [...dlg.querySelectorAll<HTMLElement>('input, select, textarea, button, a[href]')].filter((x) => !(x as HTMLInputElement).disabled) : [];

  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      // Like the backdrop: never silently discard unsaved edits (Save or Cancel decides).
      if (!dirty) onclose();
    } else if (e.key === 'Tab' && dlg) {
      // Keep focus inside the dialog.
      const f = focusables();
      if (!f.length) return;
      const a = f[0];
      const z = f[f.length - 1];
      if (e.shiftKey && document.activeElement === a) {
        e.preventDefault();
        z.focus();
      } else if (!e.shiftKey && document.activeElement === z) {
        e.preventDefault();
        a.focus();
      }
    }
  }
</script>

<svelte:window onkeydown={key} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="back"
  onpointerdown={(e) => (downOnBack = e.target === e.currentTarget)}
  onclick={(e) => {
    if (downOnBack && e.target === e.currentTarget && !dirty) onclose();
    downOnBack = false;
  }}
>
  <div class="dlg" role="dialog" aria-modal="true" aria-labelledby={titleId} bind:this={dlg}>
    <h2 id={titleId}>{title}</h2>
    {@render children()}
  </div>
</div>

<style>
  .back {
    position: fixed;
    inset: 0;
    z-index: 55;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    overflow-y: auto;
    background: var(--scrim);
    padding: calc(12px + env(safe-area-inset-top, 0px)) max(12px, env(safe-area-inset-right, 0px)) calc(12px + env(safe-area-inset-bottom, 0px))
      max(12px, env(safe-area-inset-left, 0px));
    box-sizing: border-box;
  }
  .dlg {
    width: min(420px, 100%);
    margin: auto 0;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow-pop);
    box-sizing: border-box;
  }
  h2 {
    margin: 0;
    font-size: 16px;
  }
  /* Shared look of the panels' content (it is rendered by the panel, so :global). */
  .dlg :global(label) {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .dlg :global(select),
  .dlg :global(input[type='text']),
  .dlg :global(input[type='number']) {
    min-height: 36px;
    background: var(--field);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 8px;
    /* 16px: iOS Safari zooms the page when a smaller field is focused */
    font: inherit;
    font-size: 16px;
    width: 100%;
    box-sizing: border-box;
  }
  .dlg :global([aria-invalid='true']) {
    border-color: var(--red);
  }
  .dlg :global(.note) {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
  }
  .dlg :global(.err) {
    color: var(--red);
  }
  .dlg :global(.ok) {
    color: var(--meter-green);
  }
  .dlg :global(.warn) {
    color: var(--amber);
  }
  .dlg :global(.msgs) {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-height: 16px;
  }
  .dlg :global(.row) {
    display: flex;
    gap: 8px;
  }
  .dlg :global(.row button),
  .dlg :global(.tests button) {
    flex: 1;
    min-height: 40px;
    background: var(--bg-alt);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
  }
  .dlg :global(.row .primary) {
    border-color: var(--cyan);
    color: var(--cyan);
  }
  .dlg :global(button:disabled) {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
