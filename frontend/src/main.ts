import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { admin } from './lib/admin.svelte';
import { refreshMeterPalette } from './lib/meters';

// A `#pair=…` link is consumed before anything else reads the address.
void admin.start();

// The meters draw on canvases: they take their colours from the app.css tokens.
refreshMeterPalette();

const app = mount(App, {
  target: document.getElementById('app')!,
});

export default app;
