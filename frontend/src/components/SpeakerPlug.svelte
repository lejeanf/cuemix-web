<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import Dialog from './Dialog.svelte';
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
  onMount(() => {
    store.c.plugResult = null;
  });
</script>

<Dialog title={S.plug.title} titleId="plug-title" {dirty} onclose={close}>
    <p class="note">{S.plug.help}</p>

    <label class="sw">
      <input type="checkbox" role="switch" bind:checked={draft.enabled} />
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
</Dialog>

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
  .tests {
    display: flex;
    gap: 8px;
  }
</style>
