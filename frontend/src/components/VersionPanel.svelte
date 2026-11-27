<script lang="ts">
  import { onMount } from 'svelte';
  import { store } from '../lib/store.svelte';
  import { updates } from '../lib/updates.svelte';
  import { S } from '../lib/strings';
  import { canPin, classicLatest, failedResult, latestTag, relation, visibleReleases } from '../lib/update';

  // Version panel (contract 0.7.0 B): choose a release to install. Same endpoints and
  // progress as the update badge (the shared `updates` store).
  const info = $derived(updates.info);
  const pin = $derived(canPin(info));
  const showPre = $derived(info?.show_prerelease ?? false);
  const list = $derived(info && pin ? visibleReleases(info.releases ?? [], showPre, info.installed) : []);
  const latest = $derived(latestTag(list));
  const inProgress = $derived(updates.inProgress);
  const failed = $derived(failedResult(info));
  const classic = $derived(classicLatest(info));

  /** Tag waiting for the "older than installed" confirmation. */
  let confirming = $state<string | null>(null);
  /** This panel's own message (install result, prefs error). */
  let message = $state('');

  // Once on open; the badge refreshes on reconnects.
  onMount(() => {
    void updates.refresh();
  });

  function choose(tag: string, older: boolean) {
    if (inProgress) return;
    if (older) confirming = tag;
    else void install(tag);
  }
  async function install(tag?: string) {
    message = '';
    confirming = null;
    message = (await updates.install(tag)) ?? '';
  }
  async function togglePre(on: boolean) {
    message = (await updates.setShowPrerelease(on)) ?? '';
  }
  const date = (s: string | null | undefined) => {
    const d = s ? new Date(s) : null;
    return d && !Number.isNaN(d.getTime()) ? d.toLocaleDateString() : '';
  };
  const dateTime = (s: string | null | undefined) => {
    const d = s ? new Date(s) : null;
    return d && !Number.isNaN(d.getTime()) ? d.toLocaleString() : '';
  };
  function close() {
    store.advancedOpen = false;
  }
</script>

<div class="panel">
  {#if !info}
    <p class="note" role="status">{!store.socketOpen ? S.dialog.noServer : updates.loadFailed ? S.update.requestFailed : S.update.loading}</p>
  {:else}
    <p class="note"><strong>{S.version.installed}:</strong> v{info.installed}{#if dateTime(info.status?.checked_at)} · {S.version.checked(dateTime(info.status?.checked_at))}{/if}</p>

    {#if !info.enabled}
      <p class="note">{S.version.notEnabled}</p>
    {:else if pin}
      <label class="sw">
        <input type="checkbox" role="switch" checked={showPre} disabled={!store.ready} onchange={(e) => void togglePre(e.currentTarget.checked)} />
        <span>{S.version.showPre}</span>
      </label>

      {#if list.length === 0}
        <p class="note">{S.version.noReleases}</p>
      {:else}
        <ul class="rel">
          {#each list as r (r.tag)}
            {@const rel = relation(r, info.installed)}
            <li class:cur={rel === 'installed'}>
              <div class="meta">
                <span class="tag">{r.tag}</span>
                {#if r.prerelease}<span class="chip">{S.version.pre}</span>{/if}
                {#if rel === 'installed'}<span class="chip on">{S.version.installed}</span>{/if}
                {#if r.tag === latest}<span class="chip lat">{S.version.latest}</span>{/if}
                {#if date(r.published_at)}<small class="note">{date(r.published_at)}</small>{/if}
              </div>
              {#if rel !== 'installed'}
                <button type="button" disabled={inProgress || !store.ready} onclick={() => choose(r.tag, rel === 'older')}>{S.version.install}</button>
              {/if}
            </li>
            {#if confirming === r.tag}
              <li class="confirm" role="alert">
                <p class="warn">{S.version.olderWarning(r.tag, `v${info.installed}`)}</p>
                <div class="row">
                  <button type="button" class="primary" disabled={inProgress} onclick={() => void install(r.tag)}>{S.version.confirmInstall(r.tag)}</button>
                  <button type="button" onclick={() => (confirming = null)}>{S.dialog.cancel}</button>
                </div>
              </li>
            {/if}
          {/each}
        </ul>
      {/if}
      <p class="note">{S.version.safeNote}</p>
    {:else}
      <p class="note">{S.version.needsHost}</p>
      {#if classic}
        <button type="button" class="classic" disabled={inProgress || !store.ready} onclick={() => void install()}>{S.version.installLatest(classic)}</button>
      {:else}
        <p class="note">{S.version.upToDate}</p>
      {/if}
    {/if}

    {#if inProgress}<p class="note" role="status">{S.update.installingVersion(updates.installingVersion)} {S.update.restartNote}</p>{/if}
    {#if failed && !inProgress}<p class="warn" role="alert">{S.update.lastResult(failed.outcome, failed.code, failed.version ?? null, dateTime(failed.at))}</p>{/if}
    {#if message}<p class="note" role="status">{message}</p>{/if}
  {/if}

  <div class="row">
    <button type="button" onclick={close}>{S.dialog.close}</button>
  </div>
</div>

<style>
  label.sw {
    flex-direction: row;
    align-items: center;
    gap: 10px;
    font-size: 14px;
    color: var(--text);
  }
  label.sw input {
    width: 20px;
    height: 20px;
    accent-color: var(--cyan);
  }
  .rel {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 50vh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 6px 8px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  li.cur {
    border-color: var(--cyan);
  }
  li.confirm {
    flex-direction: column;
    align-items: stretch;
    border-color: var(--amber);
  }
  li.confirm p {
    margin: 0;
    font-size: 12px;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .tag {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .chip {
    padding: 0 6px;
    font-size: 11px;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--text-dim);
  }
  .chip.on {
    border-color: var(--meter-green);
    color: var(--meter-green);
  }
  .chip.lat {
    border-color: var(--cyan);
    color: var(--cyan);
  }
  li > button,
  .classic {
    flex: none;
    min-height: 36px;
    padding: 0 12px;
    background: var(--card);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
  }
  .classic {
    border-color: var(--cyan);
    color: var(--cyan);
  }
</style>
