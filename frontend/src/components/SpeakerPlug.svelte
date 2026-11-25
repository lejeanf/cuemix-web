<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import type { PlugKind, SpeakerPlugSettings } from '../lib/protocol';
  import { PLUG_KINDS, buildUrls, defaultPlugSettings, errorSince, normalizeSettings, sameSettings, settingsProblem, urlProblem } from '../lib/plug';

  // Speaker plug settings (docs/IOS-PLAN.md 5.2b). The form edits a draft; Save sends it,
  // the server answers with a `speaker_plug` event (saved) or `error`. The tests always
  // use the saved settings, so a changed form must be saved first.
  const saved = $derived(store.c.plug?.settings ?? null);
  const initial = untrack(() => store.c.plug?.settings ?? defaultPlugSettings());
  let base = { ...initial };
  let draft = $state<SpeakerPlugSettings>({ ...initial });
  let warmS = $state<number | null>(initial.warmup_ms / 1000);

  // Server settings arriving (first load, another client): take them unless the user is mid-edit.
  $effect(() => {
    const s = saved;
    if (!s) return;
    untrack(() => {
      if (sameSettings(cur, base)) {
        draft = { ...s };
        warmS = s.warmup_ms / 1000;
      }
      base = { ...s };
    });
  });

  const cur = $derived<SpeakerPlugSettings>({ ...draft, warmup_ms: warmS === null ? Number.NaN : Math.round(warmS * 1000) });
  // What is validated, previewed and sent: trimmed host and URLs, hidden fields blanked.
  const norm = $derived(normalizeSettings(cur));
  const custom = $derived(cur.kind === 'custom');
  const urls = $derived(buildUrls(norm));
  const urlErr = $derived(urls ? null : urlProblem(norm));
  const problem = $derived(settingsProblem(cur));
  const dirty = $derived(saved === null ? !sameSettings(cur, defaultPlugSettings()) : !sameSettings(cur, saved));
  const plugState = $derived(store.c.plug?.state ?? null);

  // Only errors that arrive after this panel sent a command are shown as its reply.
  let errSeq = $state(untrack(() => store.c.lastError?.seq ?? 0));
  let saving = $state(false);
  let justSaved = $state(false);
  let testing = $state<boolean | null>(null);
  let needSave = $state(false);
  const replyErr = $derived(errorSince(store.c.lastError, errSeq));
  const result = $derived(store.c.plugResult);

  $effect(() => {
    if (replyErr) {
      saving = false;
      testing = null;
    }
  });
  $effect(() => {
    if (saving && saved && !dirty) {
      saving = false;
      justSaved = true;
    }
  });
  $effect(() => {
    if (result) testing = null;
  });
  // A request must not hang forever: the socket dropped, or no answer came.
  $effect(() => {
    if (!store.ready) {
      saving = false;
      testing = null;
    }
  });
  $effect(() => {
    if (!saving && testing === null) return;
    const t = setTimeout(() => {
      saving = false;
      testing = null;
    }, REQUEST_TIMEOUT_MS);
    return () => clearTimeout(t);
  });
  $effect(() => {
    if (dirty) justSaved = false;
    else needSave = false;
  });

  const REQUEST_TIMEOUT_MS = 8000;

  function save() {
    if (problem || saving) return;
    errSeq = store.c.lastError?.seq ?? 0;
    justSaved = false;
    saving = store.setSpeakerPlug(norm);
  }
  function test(on: boolean) {
    if (dirty) {
      needSave = true;
      return;
    }
    errSeq = store.c.lastError?.seq ?? 0;
    testing = store.testSpeakerPlug(on) ? on : null;
  }
  function close() {
    store.plugOpen = false;
  }
  // The backdrop closes the panel only for a full click on it (press and release), never mid-edit.
  let downOnBack = false;

  let dlg: HTMLElement;
  let first: HTMLInputElement;
  onMount(() => {
    const prev = document.activeElement as HTMLElement | null;
    store.c.plugResult = null;
    first?.focus();
    return () => {
      // The menu item that opened us is gone by now: go back to the menu button.
      const to = prev && prev.isConnected && prev !== document.body ? prev : document.querySelector<HTMLElement>('[data-more-menu]');
      to?.focus?.();
    };
  });
  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if (e.key === 'Tab' && dlg) {
      // Keep focus inside the dialog.
      const f = [...dlg.querySelectorAll<HTMLElement>('input, select, button')].filter((x) => !(x as HTMLInputElement).disabled);
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
    if (downOnBack && e.target === e.currentTarget && !dirty) close();
    downOnBack = false;
  }}
>
  <div class="dlg" role="dialog" aria-modal="true" aria-labelledby="plug-title" bind:this={dlg}>
    <h2 id="plug-title">{S.plug.title}</h2>
    <p class="note">{S.plug.help}</p>

    <label class="sw">
      <input type="checkbox" role="switch" bind:this={first} bind:checked={draft.enabled} />
      <span>{S.plug.enabled}</span>
    </label>
    {#if plugState}<small class="st {plugState}" role="status">{S.plugState[plugState]}</small>{/if}

    <label>{S.plug.type}
      <select value={draft.kind} onchange={(e) => (draft.kind = e.currentTarget.value as PlugKind)}>
        {#each PLUG_KINDS as k (k)}<option value={k}>{S.plug.kinds[k]}</option>{/each}
      </select>
    </label>

    {#if custom}
      <label>{S.plug.onUrl}
        <input type="text" inputmode="url" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={draft.on_url} placeholder={S.plug.onUrlPlaceholder} />
      </label>
      <label>{S.plug.offUrl}
        <input type="text" inputmode="url" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={draft.off_url} placeholder={S.plug.offUrlPlaceholder} />
      </label>
    {:else}
      <div class="row2">
        <label class="grow">{S.plug.host}
          <input type="text" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={draft.host} placeholder={S.plug.hostPlaceholder} aria-invalid={urlErr === 'host'} />
        </label>
        <label class="id">{S.plug.switchId}
          <input type="number" min="0" max="15" step="1" bind:value={draft.switch_id} aria-invalid={urlErr === 'switchId'} />
        </label>
      </div>
    {/if}

    <label>{S.plug.warmup}
      <input type="number" min="0" max="10" step="0.5" bind:value={warmS} aria-invalid={problem === 'warmup'} />
      <small class="note">{S.plug.warmupHint}</small>
    </label>

    <div class="prev" role="group" aria-label={S.plug.preview}>
      <span class="cap">{S.plug.preview}</span>
      {#if urls}
        <code><b>{S.plug.previewOn}</b> {urls.on}</code>
        <code><b>{S.plug.previewOff}</b> {urls.off}</code>
      {:else if urlErr && (draft.enabled || norm.host || norm.on_url || norm.off_url)}
        <small class="err" role="alert">{S.plug.errors[urlErr]}</small>
      {/if}
    </div>
    {#if problem && problem !== urlErr}<small class="err" role="alert">{S.plug.errors[problem]}</small>{/if}

    <div class="tests">
      <button type="button" disabled={!store.ready || testing !== null} onclick={() => test(true)}>{testing === true ? S.plug.testing : S.plug.testOn}</button>
      <button type="button" disabled={!store.ready || testing !== null} onclick={() => test(false)}>{testing === false ? S.plug.testing : S.plug.testOff}</button>
    </div>
    <div class="msgs" aria-live="polite">
      {#if needSave && dirty}<small class="warn">{S.plug.saveFirst}</small>{/if}
      {#if result && !dirty}
        <small class={result.ok ? 'ok' : 'err'}>{(result.ok ? S.plug.resultOk : S.plug.resultFail)(result.on, result.message)}</small>
      {/if}
      {#if replyErr}<small class="err" role="alert">{replyErr}</small>{/if}
      {#if justSaved && !dirty}<small class="ok">{S.plug.saved}</small>{/if}
      {#if !store.ready}<small class="warn">{S.plug.noServer}</small>{/if}
    </div>

    <div class="row">
      <button type="button" class="primary" disabled={!store.ready || problem !== null || !dirty || saving} onclick={save}>{saving ? S.plug.saving : S.plug.save}</button>
      <button type="button" onclick={close}>{dirty ? S.plug.cancel : S.plug.close}</button>
    </div>
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
    box-shadow: 0 8px 24px #000a;
    box-sizing: border-box;
  }
  h2 {
    margin: 0;
    font-size: 16px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-dim);
  }
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
  .st {
    margin-top: -6px;
    color: var(--text-dim);
  }
  .st.on {
    color: var(--meter-green);
  }
  .st.warming {
    color: var(--amber);
  }
  .st.error {
    color: var(--red);
  }
  .row2 {
    display: flex;
    gap: 8px;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .id {
    width: 84px;
    flex: none;
  }
  select,
  input[type='text'],
  input[type='number'] {
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
  [aria-invalid='true'] {
    border-color: var(--red);
  }
  .note {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
  }
  .prev {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px;
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 6px;
    min-height: 20px;
  }
  .cap {
    font-size: 12px;
    color: var(--text-dim);
  }
  code {
    font-size: 12px;
    overflow-wrap: anywhere;
    color: var(--text);
  }
  code b {
    color: var(--text-dim);
    font-weight: 600;
    margin-right: 4px;
  }
  .err {
    color: var(--red);
  }
  .ok {
    color: var(--meter-green);
  }
  .warn {
    color: var(--amber);
  }
  .msgs {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-height: 16px;
  }
  .tests,
  .row {
    display: flex;
    gap: 8px;
  }
  .tests button,
  .row button {
    flex: 1;
    min-height: 40px;
    background: var(--bg-alt);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
  }
  .row .primary {
    border-color: var(--cyan);
    color: var(--cyan);
  }
  button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
