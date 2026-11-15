<script lang="ts">
  // Placeholder state only — no real logic in Phase 1.
  let presetName = $state('default');
  let modified = $state(false);
  let connection = $state('offline / demo');

  let dim = $state(false);
  let mono = $state(false);
  let ab = $state(false);
  let talkback = $state(false);
</script>

<div class="app">
  <header class="topbar">
    <span class="brand">cuemix-web</span>
    <span class="preset">
      Preset : <strong>{presetName}</strong>
      {#if modified}<span class="modified">● Modifié</span>{/if}
    </span>
    <span class="pill">{connection}</span>
  </header>

  <main class="grid" aria-label="Console"></main>

  <footer class="controlroom">
    <span class="label">Control Room</span>
    <button class="toggle amber" class:on={dim} aria-pressed={dim} onclick={() => (dim = !dim)}>DIM</button>
    <button class="toggle" class:on={mono} aria-pressed={mono} onclick={() => (mono = !mono)}>MONO</button>
    <button class="toggle" class:on={ab} aria-pressed={ab} onclick={() => (ab = !ab)}>A/B</button>
    <button class="toggle red" class:on={talkback} aria-pressed={talkback} onclick={() => (talkback = !talkback)}>Talkback</button>
  </footer>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .topbar,
  .controlroom {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 10px 16px;
    background: var(--bg-alt);
    flex: none;
  }
  .topbar {
    border-bottom: 1px solid var(--border);
  }
  .controlroom {
    border-top: 1px solid var(--border);
    gap: 10px;
    flex-wrap: wrap;
  }
  .brand {
    font-weight: 700;
    letter-spacing: 0.02em;
  }
  .preset {
    flex: 1;
    color: var(--text-dim);
  }
  .preset strong {
    color: var(--text);
  }
  .modified {
    margin-left: 8px;
    color: var(--amber);
  }
  .pill {
    padding: 2px 12px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--card);
    color: var(--text-dim);
    font-size: 12px;
  }
  .grid {
    flex: 1;
    overflow: auto;
    padding: 16px;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(88px, 1fr));
    gap: 8px;
    align-content: start;
  }
  .label {
    color: var(--text-dim);
    margin-right: 6px;
  }
  .toggle {
    min-width: 72px;
    min-height: 40px;
    padding: 0 16px;
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    --on: var(--cyan);
  }
  .toggle.amber {
    --on: var(--amber);
  }
  .toggle.red {
    --on: var(--red);
  }
  .toggle.on {
    background: var(--on);
    border-color: var(--on);
    color: #111;
    font-weight: 600;
  }
</style>
