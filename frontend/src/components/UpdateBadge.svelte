<script lang="ts">
  import { onMount } from 'svelte';
  import { store } from '../lib/store.svelte';

  // GET /api/update (see src/web/update.rs). Nothing shows unless the update
  // units are installed on the server and a newer release exists.
  interface UpdateInfo {
    installed: string;
    channel: string;
    enabled: boolean;
    pending: boolean;
    status: { latest: string; tag: string; update_available: boolean; checked_at: string; release_url: string } | null;
    last_result: { code: number; outcome: string; at: string } | null;
  }

  let info = $state<UpdateInfo | null>(null);
  let open = $state(false);
  let busy = $state(false);
  let message = $state('');
  let root = $state<HTMLElement>();

  async function refresh() {
    try {
      const r = await fetch('/api/update', { cache: 'no-store' });
      if (r.ok) info = (await r.json()) as UpdateInfo;
    } catch {
      /* offline: keep the last answer */
    }
  }

  onMount(() => {
    void refresh();
    const t = setInterval(refresh, 10 * 60 * 1000);
    return () => clearInterval(t);
  });

  // A reconnect after the install means the new version is running.
  $effect(() => {
    if (store.socketOpen) void refresh();
  });

  const available = $derived(!!info?.enabled && !!info.status?.update_available && info.status.latest !== store.version);
  const failed = $derived(info?.last_result && info.last_result.outcome !== 'ok' ? info.last_result : null);
  const inProgress = $derived(busy || !!info?.pending);

  async function install() {
    busy = true;
    message = '';
    try {
      const r = await fetch('/api/update/install', { method: 'POST', headers: { 'x-cuemix-update': '1' } });
      message = r.ok ? 'Installing… the page reconnects when the new version is running.' : await r.text();
      if (r.ok) setTimeout(refresh, 3000);
    } catch {
      message = 'Request failed';
    } finally {
      busy = false;
    }
  }

  function outside(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }
</script>

<svelte:window onpointerdown={outside} />

{#if available || failed || inProgress}
  <span class="wrap" bind:this={root}>
    <button class="badge" class:warn={!!failed && !available} onclick={() => (open = !open)} aria-expanded={open}>
      {#if inProgress}Updating…{:else if available}v{info?.status?.latest} available{:else}Update failed{/if}
    </button>
    {#if open && info}
      <div class="pop" role="dialog" aria-label="Software update">
        <p><strong>Installed:</strong> v{info.installed} · <strong>channel:</strong> {info.channel}</p>
        {#if info.status}
          <p>
            <strong>Latest:</strong> v{info.status.latest}
            (<a href={info.status.release_url} target="_blank" rel="noopener noreferrer">release notes</a>)
            <br /><small>checked {new Date(info.status.checked_at).toLocaleString()}</small>
          </p>
        {/if}
        {#if failed}
          <p class="err">
            Last install {failed.outcome === 'rolled_back' ? 'failed and was rolled back to the previous version' : `failed (code ${failed.code})`}{failed.at ? `, ${new Date(failed.at).toLocaleString()}` : ''}.
          </p>
        {/if}
        {#if available}
          <p class="dim">The service restarts (a few seconds without control; audio keeps playing). The download is checked (sha256) and a version that does not start is rolled back automatically.</p>
          <button class="go" disabled={inProgress} onclick={install}>{inProgress ? 'Installing…' : `Install v${info.status?.latest}`}</button>
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
    box-shadow: 0 8px 24px #000a;
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
    color: #111;
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
