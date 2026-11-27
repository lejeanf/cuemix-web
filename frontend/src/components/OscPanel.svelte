<script lang="ts">
  import { untrack } from 'svelte';
  import { store } from '../lib/store.svelte';
  import { S } from '../lib/strings';
  import { errorSince } from '../lib/plug';
  import { OSC_MAX_TARGETS, normalizeOsc, oscProblems, oscValid, sameOsc } from '../lib/osc';
  import type { OscSettings } from '../lib/protocol';

  // OSC settings (contract 0.7.0 A). The form edits a draft; Save sends it and the server
  // answers with an `osc_settings` event (saved) or `error`. The server also checks that
  // every address is on the local network.
  const REQUEST_TIMEOUT_MS = 8000;
  const saved = $derived(store.c.osc?.settings ?? null);
  const info = $derived(store.c.osc);

  let rowId = 0;
  type Row = { id: number; v: string };
  const rowsOf = (t: readonly string[]): Row[] => t.map((v) => ({ id: ++rowId, v }));

  const initial = untrack(() => store.c.osc?.settings ?? { listen: '', targets: [] });
  let base: OscSettings = { listen: initial.listen, targets: [...initial.targets] };
  let listen = $state(initial.listen);
  let rows = $state<Row[]>(rowsOf(initial.targets));

  const cur = $derived<OscSettings>({ listen, targets: rows.map((r) => r.v) });
  const norm = $derived(normalizeOsc(cur));
  const problems = $derived(oscProblems(cur));
  const valid = $derived(oscValid(cur));
  const dirty = $derived(saved === null ? !sameOsc(cur, { listen: '', targets: [] }) : !sameOsc(cur, saved));
  /** The tab bar needs to know about unsaved edits (switching tabs keeps them; closing asks first). */
  let { onDirty }: { onDirty?: (d: boolean) => void } = $props();
  $effect(() => onDirty?.(dirty));

  // Server settings arriving (first load, another client): take them unless the user is mid-edit.
  $effect(() => {
    const s = saved;
    if (!s) return;
    untrack(() => {
      if (sameOsc(cur, base)) {
        listen = s.listen;
        rows = rowsOf(s.targets);
      }
      base = { listen: s.listen, targets: [...s.targets] };
    });
  });

  // Only errors that arrive after this panel sent a command are shown as its reply.
  let errSeq = $state(untrack(() => store.c.lastError?.seq ?? 0));
  let saving = $state(false);
  let justSaved = $state(false);
  const replyErr = $derived(errorSince(store.c.lastError, errSeq));

  $effect(() => {
    if (replyErr) saving = false;
  });
  // The server answered our Save: take its (canonical, e.g. lower-cased IPv6) settings as the draft.
  let seen = untrack(() => store.c.osc);
  $effect(() => {
    const o = store.c.osc;
    if (o === seen) return;
    seen = o;
    untrack(() => {
      if (!saving || !o) return;
      listen = o.settings.listen;
      rows = rowsOf(o.settings.targets);
      saving = false;
      justSaved = true;
    });
  });
  // A request must not hang forever: the socket dropped, or no answer came.
  $effect(() => {
    if (!store.ready) saving = false;
  });
  $effect(() => {
    if (!saving) return;
    const t = setTimeout(() => (saving = false), REQUEST_TIMEOUT_MS);
    return () => clearTimeout(t);
  });
  $effect(() => {
    if (dirty) justSaved = false;
  });

  function add() {
    if (rows.length >= OSC_MAX_TARGETS) return;
    rows.push({ id: ++rowId, v: '' });
  }
  function remove(id: number) {
    rows = rows.filter((r) => r.id !== id);
  }
  function save() {
    if (!valid || saving) return;
    errSeq = store.c.lastError?.seq ?? 0;
    justSaved = false;
    saving = store.setOscSettings(norm);
  }
  function close() {
    store.advancedOpen = false;
  }
  // Per-row problem: problems.targets indexes the normalized list (empty rows are dropped).
  const rowErr = $derived.by(() => {
    const out = new Map<number, string>();
    const nums: number[] = []; // normalized index -> 1-based row number
    rows.forEach((r, i) => {
      if (r.v.trim() !== '') nums.push(i + 1);
    });
    rows
      .filter((r) => r.v.trim() !== '')
      .forEach((r, k) => {
        const e = problems.targets[k];
        const d = problems.dupOf[k];
        if (e) out.set(r.id, S.osc.errors[e] as string);
        else if (d !== null && d !== undefined) out.set(r.id, S.osc.errors.duplicate(nums[d]));
      });
    return out;
  });
</script>

<div class="panel">
  <p class="note">{S.osc.help}</p>

  {#if info}
    <small class={info.listening ? 'ok' : 'warn'} role="status">{info.listening ? S.osc.listening : S.osc.notListening}</small>
    {#if info.error}<small class="err" role="alert">{S.osc.serverError}: {info.error}</small>{/if}
  {/if}

  <label>{S.osc.listen}
    <input type="text" inputmode="url" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={listen} placeholder={S.osc.listenPlaceholder} aria-invalid={problems.listen !== null} />
    <small class="note">{S.osc.listenHint}</small>
  </label>
  {#if problems.listen}<small class="err" role="alert">{S.osc.errors[problems.listen]}</small>{/if}

  <fieldset>
    <legend>{S.osc.targets}</legend>
    <p class="note">{S.osc.targetsHelp}</p>
    {#if rows.length === 0}<small class="note">{S.osc.noTargets}</small>{/if}
    {#each rows as r, i (r.id)}
      <div class="trow">
        <input
          type="text"
          inputmode="url"
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
          aria-label={S.osc.targetLabel(i + 1)}
          bind:value={r.v}
          placeholder={S.osc.targetPlaceholder}
          aria-invalid={rowErr.has(r.id)}
        />
        <button type="button" class="x" aria-label={S.osc.removeTarget(i + 1)} title={S.osc.removeTarget(i + 1)} onclick={() => remove(r.id)}>×</button>
      </div>
      {#if rowErr.has(r.id)}<small class="err" role="alert">{rowErr.get(r.id)}</small>{/if}
    {/each}
    {#if problems.list}<small class="err" role="alert">{S.osc.errors.tooMany}</small>{/if}
    <button type="button" class="add" disabled={rows.length >= OSC_MAX_TARGETS} onclick={add}>{S.osc.addTarget}</button>
    {#if rows.length >= OSC_MAX_TARGETS}<small class="note">{S.osc.maxTargets}</small>{/if}
  </fieldset>

  <div class="msgs" aria-live="polite">
    {#if replyErr}<small class="err" role="alert">{replyErr}</small>{/if}
    {#if justSaved && !dirty}<small class="ok">{S.dialog.saved}</small>{/if}
    {#if !store.ready}<small class="warn">{S.dialog.noServer}</small>{/if}
  </div>

  <div class="row">
    <button type="button" class="primary" disabled={!store.ready || !valid || !dirty || saving} onclick={save}>{saving ? S.dialog.saving : S.dialog.save}</button>
    <button type="button" onclick={close}>{dirty ? S.dialog.cancel : S.dialog.close}</button>
  </div>
</div>

<style>
  fieldset {
    margin: 0;
    padding: 8px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  legend {
    padding: 0 4px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .trow {
    display: flex;
    gap: 6px;
  }
  .trow input {
    flex: 1;
    min-width: 0;
  }
  .x,
  .add {
    min-height: 36px;
    background: var(--bg-alt);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
  }
  .x {
    flex: none;
    width: 40px;
    font-size: 18px;
    line-height: 1;
  }
</style>
