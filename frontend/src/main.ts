import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { admin } from './lib/admin.svelte';

// A `#pair=…` link is consumed before anything else reads the address.
void admin.start();

const app = mount(App, {
  target: document.getElementById('app')!,
});

export default app;
