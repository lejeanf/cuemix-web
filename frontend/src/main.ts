import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { admin } from './lib/admin.svelte';
import { applyTheme, loadTheme } from './lib/theme';

// A `#pair=…` link is consumed before anything else reads the address.
void admin.start();

// Before the first paint, so Night mode never flashes the brighter theme.
try {
  applyTheme(loadTheme(localStorage));
} catch {
  /* storage blocked: default theme */
}

const app = mount(App, {
  target: document.getElementById('app')!,
});

export default app;
