<script lang="ts">
  interface Props {
    value: number;
    min: number;
    max: number;
    step?: number;
    label: string;
    unit?: string;
    disabled?: boolean;
    onchange: (value: number) => void;
  }
  let { value, min, max, step = 1, label, unit = '', disabled = false, onchange }: Props = $props();

  function commit(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const v = Number(el.value);
    if (Number.isFinite(v) && el.value.trim() !== '') onchange(Math.min(max, Math.max(min, v)));
    else el.value = String(value);
  }
</script>

<label class="nf">
  <span>{label}</span>
  <span class="in">
    <input type="number" {min} {max} {step} {disabled} value={Math.round(value * 100) / 100} aria-label={label} onchange={commit} />
    {#if unit}<em>{unit}</em>{/if}
  </span>
</label>

<style>
  .nf {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 11px;
    color: var(--text-dim);
  }
  .in {
    display: flex;
    align-items: center;
    gap: 3px;
  }
  input {
    width: 72px;
    min-height: 30px;
    background: #0d0e11;
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 6px;
    font: inherit;
    font-size: 13px;
  }
  em {
    font-style: normal;
  }
</style>
