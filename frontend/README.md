# cuemix-web frontend

Svelte 5 + Vite + TypeScript. Compiled to `frontend/dist/`, embedded in the Rust binary via `rust-embed`.

- Install: `npm ci` (or `npm install`). Package install scripts never run (`.npmrc`: `ignore-scripts=true`).
- Dev: start the Rust backend (listens on `127.0.0.1:8080`), then `npm run dev`. Vite proxies `/ws` and `/api` to the backend.
- Type check: `npm run check`.
- Production: `npm run build` produces `dist/`. It **must** run before `cargo build`, because `rust-embed` embeds `frontend/dist` at compile time.
- Assets use a relative base (`./`) so the bundle works behind a reverse proxy subpath. No external fonts, CDNs or telemetry.
