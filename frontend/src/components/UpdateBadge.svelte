<script lang="ts">
  import { onMount } from 'svelte';
  import { store } from '../lib/store.svelte';
  import { updates } from '../lib/updates.svelte';
  import { admin } from '../lib/admin.svelte';
  import { S } from '../lib/strings';
  import { classicLatest, failedResult } from '../lib/update';

  // GET /api/update (see src/web/update.rs), shared with the Version panel. Nothing shows
  // unless the update units are installed on the server and a newer release exists.
  const info = $derived(updates.info);
  let open = $state(false);
  let root = $state<HTMLElement>();
  let message = $state('');
  const refresh = () => updates.refresh();

  onMount(() => {
    void refresh();
    const t = setInterval(refresh, 10 * 60 * 1000);
    return () => clearInterval(t);
  });

  // A reconnect after the install means the new version is running.
  $effect(() => {
    if (store.socketOpen) void refresh();
  });

  const latest = $derived(classicLatest(info));
  const available = $derived(!!latest && latest !== store.version);
  const failed = $derived(failedResult(info));
  const inProgress = $derived(updates.inProgress);
  const when = (s: string) => {
    const d = new Date(s);
    return Number.isNaN(d.getTime()) ? '' : d.toLocaleString();
  };

  async function install() {
    message = '';
    message = (await updates.install()) ?? S.update.installing;
  }

  function outside(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }
</script>

<svelte:window onpointerdown={outside} />

{#if available || failed || inProgress}
  <span class="wrap" bind:this={root}>
    <button class="badge" class:warn={!!failed && !available && !inProgress} onclick={() => (open = !open)} aria-expanded={open}>
      {#if inProgress}{S.update.badgeUpdating}{:else if available && latest}{S.update.badgeAvailable(latest)}{:else}{S.update.badgeFailed}{/if}
    </button>
    {#if open && info}
      <div class="pop" role="dialog" aria-label={S.update.popTitle}>
        <p>{S.update.installedChannel(info.installed, info.channel)}</p>
        {#if latest}
          <p>
            <strong>{S.update.latest}:</strong> v{latest}
            {#if info.status?.release_url}(<a href={info.status.release_url} target="_blank" rel="noopener noreferrer">{S.update.releaseNotes}</a>){/if}
            {#if info.status?.checked_at && when(info.status.checked_at)}<br /><small>{S.update.checked(when(info.status.checked_at))}</small>{/if}
          </p>
        {/if}
        {#if inProgress}<p class="dim" role="status">{S.update.installingVersion(updates.installingVersion)}</p>{/if}
        {#if failed && !inProgress}
          <p class="err">{S.update.lastResult(failed.outcome, failed.code, failed.version ?? null, when(failed.at))}</p>
        {/if}
        {#if available}
          <p class="dim">{S.update.restartNote}</p>
          <button class="go" disabled={inProgress || admin.locked} title={admin.locked ? S.admin.locked : undefined} onclick={install}>{inProgress ? S.update.installingVersion(null) : S.update.installBtn(latest ?? '')}</button>
        {/if}
        {#if message}<p class="dim">{message}</p>{/if}
      </div>
    {/if}
  </span>
{/if}

<style>
  .wrap {
    position: relative;
  }
  .badge {
    min-height: 20px;
    padding: 0 8px;
    font-size: 11px;
    background: color-mix(in srgb, var(--cyan) 20%, var(--card));
    color: var(--text);
    border: 1px solid var(--cyan);
    border-radius: 999px;
    cursor: pointer;
  }
  .badge.warn {
    border-color: var(--amber);
    background: color-mix(in srgb, var(--amber) 15%, var(--card));
  }
  .pop {
    position: absolute;
    right: 0;
    bottom: calc(100% + 8px);
    width: min(320px, 90vw);
    padding: 10px 12px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow-pop);
    font-size: 12px;
    white-space: normal;
    z-index: 40;
  }
  .pop p {
    margin: 0 0 8px;
  }
  .pop a {
    color: var(--cyan);
  }
  .dim {
    color: var(--text-dim);
  }
  .err {
    color: var(--amber);
  }
  .go {
    min-height: 28px;
    padding: 0 12px;
    background: var(--cyan);
    color: var(--on-accent);
    font-weight: 700;
    border: 0;
    border-radius: 6px;
    cursor: pointer;
  }
  .go:disabled {
    opacity: 0.5;
    cursor: wait;
  }
</style>
