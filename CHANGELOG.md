# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). The crate version in `Cargo.toml`
must match the release tag (`v<version>`); the release workflow refuses a
mismatch.

Compatibility notes for each release say whether presets or `config.toml`
need attention. Preset files carry `schema_version`; see
`docs/ARCHITECTURE.md` (decision D15).

## [Unreleased]

Simple view refinements from the UI review. Compatibility: no preset or `config.toml` change.

### Added
- `power` event and `GET /api/v1/summary`: `return_db` per channel, the whole-dB level an On
  fades back to while the channel is off or fading out (the remembered level, else −70 dB), `null`
  while it is on. Older clients ignore it; the web UI reads it when present.

### Changed
- Monitor and Phones knobs (Simple view and Output view) use the same curve as the sliders: most
  of the travel now covers the top 40 dB, where listening levels are (−30 dB sits at 44 % of the
  arc instead of 70 %). Arrow keys, the mouse wheel and screen-reader steps move exactly 1 dB.
  The curve is shared with the future iOS app in `design/vectors/volume-knob.json`.
- On/off buttons: during a fade the button keeps the colour of where it is going and a bar sweeps
  along its bottom edge (a still bar with reduced motion), so it no longer looks disabled. The
  speaker plug warm-up turns the button amber with a countdown ("Warming up · 2 s"), and the plug
  dot is larger.
- An off channel shows "Off" instead of "−100 dB" under its knob (and to screen readers), with the
  level On brings it back to ("returns to −18 dB").
- Presets in the Simple view: a "Preset" row shows the current preset and "● Modified"; it opens a
  list where each preset has its own Load button, so one mis-tap no longer changes every mix.
- Meters: faint ticks at −40, −18 and −6 dBFS, and a red clip mark that stays until the meter is
  tapped.
- Simple view knobs grow on tablets and laptops (up to 220 px, was 150 px).

Quick fixes from the UI review (web UI only; no server or preset change).

### Changed
- Header: a two-part **Simple | Advanced** switch shows the current view (icons on phones) instead
  of one button labelled with the other view. The header buttons now sit on the right in both views.
- Demo mode: the status shows an amber **Demo** pill, always visible (its full text, "Demo — no
  MOTU", is the accessible name). Red is no longer used for demo.
- Error toasts stay until tapped (with a ×) and are announced at once (`role="alert"`); notices and
  warnings still fade after 5 s.
- Input view on phones: preamp cards share each row evenly (three per row) instead of leaving gaps.

### Fixed
- Presets panel on phones: it ran 121 px off the left edge at 375 px, hiding the preset names. Below
  640 px it now spans the screen under the header and scrolls if needed.
- The view follows the URL fragment on an open page: Back/Forward or editing `#output` switches tab.

## [0.7.2] - 2026-11-27

Fixes from a security review: a web page could control the mixer through DNS rebinding,
releases could be downgraded to vulnerable versions, and the release pipeline gave build
code more rights than it needs. Releases are now signed, and an optional admin token can
reserve installs and the plug/OSC settings to paired devices.

### Added
- **Optional admin token** (`[web] admin_token = "auto"`, off by default). The server creates
  a random token in `[storage] admin_token_file` (mode 0600) and logs a pairing link once;
  `cuemix-web --config <path> --pair-link` prints it again (as root). Opening the link on a
  device pairs it (`HttpOnly`, `SameSite=Strict` cookie; the token travels in the URL
  fragment, never to a server log). Only paired devices may install releases, switch
  pre-releases, or change and test the speaker plug and OSC settings; faders, presets and
  power stay open to everyone. Unpaired devices see these controls read-only with a note.
  Scripts can send `Authorization: Bearer <token>`. New routes: `GET /api/admin`,
  `POST /api/admin/pair`, `POST /api/admin/unpair`.

### Security
- **DNS rebinding**: the server now answers only for host names an outside site cannot own:
  IP addresses, `localhost`, `*.local`, the mDNS name, and the new `[web] allowed_hosts` list.
  Any other `Host` (or trusted `X-Forwarded-Host`) gets 421 before any route runs. Before,
  a page whose own domain was pointed at the server sent matching `Host` and `Origin` headers
  and could drive the mixer, change the plug and OSC targets, and queue installs.
- `/api/update`, `/api/update/install` and `/api/update/prefs` also refuse cross-site
  `Origin` headers (403), like `/ws` and `/api/v1`.
- Security headers on every response: a Content-Security-Policy (scripts from this server
  only, no framing, WebSocket only to this server), `X-Frame-Options: DENY` (clickjacking),
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
  `Cross-Origin-Opener-Policy: same-origin`.
- **Security floor**: neither the UI nor the root updater installs a release older than
  v0.7.2. Older releases would bring back the hole above or bugs fixed since. The newest
  release publishes its floor (`min-version` asset): installed updaters can only raise their
  own with it, and the status file passes it to the UI (`min_version`). A root shell can still
  run `cuemix-update --version vX.Y.Z --force`. The floor lives in `src/web/update.rs`,
  `deploy/cuemix-update.sh` and `deploy/min-version`; CI checks they agree.
- `cuemix-update` never runs a binary it has only downloaded as root: the `--version` check
  runs as `nobody` (`setpriv`, no new privileges, empty environment). Only the `cuemix-web`
  member is unpacked from the tarball, and a symlink there is refused.
- Release workflow: no token rights by default. Jobs that run third-party build code get
  `contents: read` only, `publish` alone may write the release and `docker` alone may push the
  image. No checkout keeps the token in `.git/config`, and release builds use no caches.
- OSC: datagrams from outside the local network (private, link-local, loopback) are dropped,
  so a forged source address can no longer make the bridge send state dumps to a stranger.
- HTTP: a client gets 10 s to send its request headers (slow-header attacks), at most 256
  connections are served at once, and WebSockets are capped at 64 in total and 16 per client
  address (503 beyond). The server now runs its own accept loop on hyper (already a dependency
  through axum) because `axum::serve` has no header timeout.
- At most 200 presets: a new name beyond that is refused (overwriting still works), so no
  client can fill the disk.
- systemd: `cuemix-web.service` adds `PrivateDevices`, `ProtectProc=invisible`, a system-call
  filter, `RestrictAddressFamilies`, `MemoryDenyWriteExecute`, `UMask=0077`, `MemoryMax=256M`,
  `TasksMax=256` and more (`systemd-analyze security`: 1.4, OK). The root update units may
  only write the binary, the two state directories and `/run` (`ProtectSystem=strict`, private
  `/tmp`); `/var/lib/cuemix-update` is now a systemd `StateDirectory` (root, 0700).
- `cuemix-update` refuses to publish the status files when `/var/lib/cuemix-update` and
  `/var/lib/cuemix-web` are on different filesystems (a move there would be a root copy into
  the web user's directory).
- CI: GitHub's own actions are pinned by commit SHA like the others, the gitleaks download is
  checked against its published sha256, the Docker base image is pinned by digest, and a
  manual run of the release workflow builds but never publishes.

- **Signed releases**: every release file (tarballs, `.deb`, `min-version`) is signed with
  minisign in a separate release job that only a protected `release` environment (required
  reviewer) can run, and that runs no third-party build code. `cuemix-update`, `linux-setup.sh`
  and the Proxmox provisioning refuse a download that is not signed by the key in
  `deploy/minisign.pub` under its exact file name (so a signed file cannot be passed off as
  another tag or architecture). An unsigned or altered `min-version` is ignored. minisign
  runs unprivileged.

### Fixed
- The update notice no longer offers an older release when the root updater is older than the
  web server (it reported "update available" for any different version).

### Compatibility
- If you reach the UI by a name that is not an IP address, `localhost`, `*.local` or the mDNS
  name (for example a reverse proxy's public name or a router DNS name), add it to
  `[web] allowed_hosts` in `config.toml` before updating, or the UI answers 421. The accepted
  names are logged at startup.
- The Version dialog no longer lists releases older than v0.7.2.
- `cuemix-update` now needs `minisign` (Debian 12 has it; `proxmox-setup.sh --update` and
  `linux-setup.sh` install it). Ubuntu 22.04 has no package: install it from upstream.
- Releases from v0.7.2 on are signed; nothing older can be installed anyway (floor v0.7.2).
- The new systemd sandbox was checked on a regular systemd host, not yet inside a Proxmox
  LXC container. If `cuemix-web` does not start after `proxmox-setup.sh --update`,
  `journalctl -u cuemix-web` names the directive; report it.
- The root-side changes (floor, unprivileged version check) arrive with
  `sudo deploy/proxmox-setup.sh --update` (or `deploy/linux-setup.sh`), like every change to
  `cuemix-update`. The in-app updater only replaces the binary.

## [0.7.1] - 2026-11-26

Stricter input checking everywhere the server accepts text or numbers, and one
Advanced settings dialog behind ⋯.

### Added
- `design/vectors/protocol.json`: one sample of every WebSocket message, written by the
  server's own types (`tests/protocol_vectors.rs`). vitest checks `protocol.ts` and the
  client reducer against it, so the web UI and the server can no longer drift apart
  silently. Regenerate with `UPDATE_VECTORS=1 cargo test --test protocol_vectors`.

### Security
- npm never runs package install scripts (`frontend/.npmrc`, and `npm ci --ignore-scripts`
  in CI): the usual way a compromised package runs code on a build machine. No package in
  the lock needs one.
- Every request body (update install, pre-release switch, `POST /api/v1/command`) is
  checked before it is read: size limit, `Content-Type: application/json` when
  there is a body, valid UTF-8 and no control characters. It is then read into a
  fixed set of fields; wrong types are refused with a clear error, and so are
  unknown fields in the update requests and in the speaker plug and OSC
  settings (commands are typed but tolerate extra fields).
  `POST /api/v1/command` now requires `Content-Type: application/json` (in
  Shortcuts, set the request body to JSON).
- WebSocket commands get the same checks: message size, control characters, and
  one shared validator for preset names, imported preset text (240 KiB at most),
  channel labels, speaker plug settings and OSC settings. Labels can no longer
  contain invisible or text-reversing characters.
- Speaker plug and custom URLs: the path and query accept only plain ASCII
  letters, digits, `- . _ ~ / ? & = : , + ; ! $ ' ( ) * @` and `%XX`, at most
  2048 characters; host names follow the usual label rules (253 characters at
  most), and a login before the host (`user@host`) is refused. Spaces, line
  breaks, `#`, backslashes and non-ASCII look-alikes are refused, so a URL can
  never add lines to the request sent to the plug.
- OSC: addresses and the OSC settings form only accept digits for ports and
  indices, bounded text and no control characters; incoming OSC messages are
  length-limited, and preset names received over OSC follow the preset rules.
- Saved channel labels that were edited by hand into something the UI would
  refuse (odd colours, control characters) are ignored when the server starts.
- The web forms check the same rules before sending (preset names, labels,
  speaker plug URLs); the server stays the authority.

### Changed
- **⋯** now opens one **Advanced settings** dialog with three tabs (Speaker
  plug, OSC, Version) instead of a menu that opens a dialog per item. It opens
  on the tab you used last (remembered in this browser), keeps unsaved edits
  when you switch tabs, and is also available in the Simple view.

### Compatibility
- `POST /api/v1/command` answers 415 without `Content-Type: application/json`
  (for example `curl -d` without `-H 'Content-Type: application/json'`).
  Add the header to scripts and shortcuts that call it.
- Speaker plug URLs saved by an earlier version that break the new rules
  (spaces, control or non-ASCII characters, over-long text) disable the plug at
  start-up, with the reason in the server log. Edit them in the plug panel and
  save again.
- Hand-edited `labels.json` entries that the UI would refuse (invalid colour,
  control characters, unknown channel) are ignored at start-up, and are gone for
  good after the next label change saves the file.
- No preset or `config.toml` change.

### Fixed
- `deploy/proxmox-setup.sh --update` waits (up to 60 s) for an update already
  running in the container, such as an install started from the web UI, instead
  of stopping with "another cuemix-update is already running".

## [0.7.0] - 2026-11-26

OSC and version settings in the Advanced menu, and open tabs follow updates.

### Added
- **⋯ → OSC…**: edit the controllers that receive OSC feedback (up to 8,
  e.g. TouchOSC on a tablet) and the server's OSC listen address and port.
  Changes apply live; a listen address that does not work keeps the old one
  and shows the error. Controllers must be on the home network. A controller
  name that does not resolve at start-up (device asleep) is retried every
  minute. Saved in `osc.json`, which overrides `[network]` in `config.toml`.
- **⋯ → Version…**: the published releases, with Install on each one.
  Installing an older release asks first (settings added later, like the
  speaker plug, stop working until you update again); nothing older than
  v0.5.0 is offered. A **Show pre-releases** switch decides what the UI
  offers and what the update notice announces. Installs keep the same checks
  (sha256, health check, automatic rollback); one install request per 30 s.
- An open tab reloads itself once when the server runs a different version
  (never while a settings panel is open), so the page always matches the
  server.

### Changed
- Updates: the root updater writes its files through a root-only directory
  (`/var/lib/cuemix-update`) and treats the web's install request as
  untrusted input (64 bytes, strict tag check, no symlinks or FIFOs). The
  web's pre-release switch never changes what a shell `cuemix-update` run
  installs.
- The ⋯ menu, Speaker plug panel and the new panels share one dialog:
  Escape and a click outside no longer discard unsaved edits; the rest of
  the page is inert while a panel is open.

### Compatibility
- No change to presets. New optional `[storage]` keys `osc_file` and
  `update_prefs_file` (defaults in `/var/lib/cuemix-web/`).
- **Per-version install needs the new root updater once.** The in-app
  updater only replaces the program. On Proxmox, run
  `sudo deploy/proxmox-setup.sh --update` on the host from a 0.7.0 checkout
  (or plain Linux: `sudo deploy/linux-setup.sh --refresh-updater`). Until
  then the Version panel offers only "Install latest" and says why.

## [0.6.0] - 2026-11-25

On/off moves to the server, and Monitor can switch a speaker plug.

### Added
- **Speaker plug.** Monitor (Main) on/off can switch a smart plug that powers the
  speakers, in a safe order: **Off** fades out, waits 0.3 s, then switches the
  plug off; **On** switches the plug on, waits a warm-up (2 s by default,
  adjustable), then fades in. If the plug does not answer, the volume fades in
  anyway and the app shows "Speaker plug did not respond". The plug is only
  switched while the MOTU is online. Set it up in the Advanced view: **⋯ →
  Speaker plug** (Shelly Gen2 / Plus / Pro / Gen3 / Gen4, Shelly Gen1, Tasmota, or
  two custom URLs), with **Test on / Test off**. The server only sends plain
  `http` GET requests to addresses on the home network (private, link-local or
  loopback); anything else is refused.
- On/off and the fade now run **on the server**, so every device shows the same
  state and the knob moves everywhere during a fade. New `power` command and
  event, OSC `/mix/main/power` and `/controlroom/phones/power`.
- Short HTTP API for the coming iOS app: `GET /api/v1/summary`,
  `POST /api/v1/command` (Monitor/Phones volume, power, preset recall only).
  `/api/health` reports `"api": 1`.

### Changed
- The level that Monitor/Phones return to after Off is remembered by the
  server (`power.json`), shared by every device and kept across restarts.

### Compatibility
- Presets and existing `config.toml` files keep working. New optional keys:
  `[storage] power_file` and `speaker_plug_file` (defaults in
  `/var/lib/cuemix-web/`), `[speaker_plug]` (off by default); see
  `config.example.toml`. Plug settings edited in the UI are saved in
  `speaker-plug.json` and override `config.toml`.
- The level each browser remembered for on/off (`localStorage`) is not carried
  over: the first On after the update fades to -70 dB if nothing was remembered.

## [0.5.2] - 2026-11-20

The web UI can be installed on the iPhone home screen as **Cue Desk**
(milestone 0 of the iOS plan, `docs/IOS-PLAN.md`), and the MOTU link on a
Proxmox host survives NetworkManager.

### Added
- **Home-screen app on iPhone.** The web UI ships a web app manifest (`Cue Desk`,
  standalone, `start_url` `./?simple`), an `apple-touch-icon` and new original
  icons (a rotary knob in the accent blue; generated by
  `frontend/scripts/gen-icons.mjs`). Safari **Share, Add to Home Screen** opens
  it full screen straight in the Simple view.
- `?simple` in the URL opens the Simple view for that page load without
  saving it. `?embed` (for the future native app's web view) opens the
  Advanced view for that load and hides the Simple/Advanced toggle and the
  connection dot; `?embed&simple` opens Simple.
- The page reconnects immediately when it returns from lock or background
  (`visibilitychange` / `pageshow`) instead of waiting for the backoff timer;
  an apparently open connection that stays silent for 2 s after unlock
  (online or demo) is dropped and reopened.

### Changed
- Safe areas: with `viewport-fit=cover` the top bar, bottom bars, drawer, toasts
  and landscape sides stay clear of the status bar, notch and home indicator
  (no change on desktop). The favicon uses the new icon.

### Fixed
- Proxmox host: the MOTU link no longer drops out of the isolated MOTU bridge
  (the app fell back to demo mode) when NetworkManager runs on the host. The
  link is now marked unmanaged for NetworkManager, and a systemd timer (every
  30 s) re-attaches it if anything detaches it (see
  `journalctl -u cuemix-motu-bridge`).

### Compatibility
- No change to presets or `config.toml`. Update as usual (Advanced view,
  **Install**, or `cuemix-update`). The home-screen app keeps its own
  storage, separate from Safari, so it starts with default settings.
- The MOTU link fix lives on the Proxmox host: run
  `sudo deploy/proxmox-setup.sh --update` from a checkout of this release.
  The in-app updater only updates the container.

## [0.5.1] - 2026-11-16

### Changed
- The release check runs every hour instead of once a day, so a new version
  shows up in the Advanced view (Control Room bar: **vX available**, then
  **Install**) within about an hour. The page re-reads the result every 10
  minutes and on reconnect.

### Compatibility
- The hourly timer is installed by `deploy/proxmox-setup.sh --update` (or
  `linux-setup.sh`); an install from the UI keeps the timer it already has.

## [0.5.0] - 2026-11-16

Phone view with rotary knobs and per-channel fades, room test with the phone
microphone, input and output EQ written to the card, and meters that follow
the card's output routing.

### Changed
- Phones meter: it now shows what the headphone jack plays. Output meters
  follow the card's output patch (property 5010): on the card the jack is fed
  by the Main mix, not by the Phones mix, so the meter read the (near-silent)
  Phones mix and stayed empty. An output with nothing patched reads silent.
- The accent colour is now blue (#3b93e0) instead of cyan, everywhere.
- Top bar: the status label shows only on hover, focus or a tap.
- Top bar: the connection status is now a small dot (green online, amber
  pulsing while connecting, red when the server has no MOTU, grey when the
  server is unreachable). Its text shows on hover, focus or tap, and for a few
  seconds whenever the state changes. The Presets button is hidden in the
  Simple (phone) view.
- Controls that are not sent to the MOTU yet (compressor, gate, clock, Dim,
  Speaker A/B, reverb) are now greyed out and disabled instead of staying
  interactive with a dashed outline, so they no longer look broken. The Reverb
  tab is hidden while a MOTU is connected and returns once reverb is written.

### Added
- The output patch table of the card (property 5010) is now read, never
  written, and sent to the web UI (`patch` message). The meter probe
  (`/?probe`) shows it as a "Patch (5010)" table (destination to source
  point, highlighted when the source has signal), and the output meters are
  ready to follow it once the destination of each output is known (the
  Phones meter stays empty meanwhile: the headphone jack is not fed by the
  Phones mix).
- Room test (optional, `[measure]` in `config.toml`, off by default): the
  server asks your MPD / myMPD to play pink noise (served by the new
  `GET /api/measure/noise.wav`, 90 s, 48 kHz, 16-bit, about -20 dBFS RMS,
  streamed), mutes every other input of the Main mix, clears solos, bypasses
  the Main output EQ and the source inputs' EQ, then restores all of it (and
  what MPD was playing) when the test ends, fails, is stopped or hits the
  `max_seconds` watchdog (20-80 s). The values to restore are kept in
  `measure-restore.json` until the card has them, so a link loss, unplug,
  restart or crash mid-test is repaired when the MOTU is back. New WebSocket messages `measure_start` /
  `measure_stop` and a `measure` event (`unavailable`, `idle`, `starting`,
  `quiet`, `playing`, `restoring`, `error`). The phone measures the room with its
  microphone and proposes up to three Main output EQ cuts below 500 Hz; the
  analysis runs in the browser. The card has no noise generator, hence MPD.
  The microphone needs the UI served over HTTPS.
  MPD is paused by the server for a 3.5 s quiet phase (background recording)
  before the noise; the test also ends when the page that started it
  disconnects.
- Output EQ writes: the five output EQ props (1022-1026: type, bypass,
  frequency, gain, Q) are now sent to the MOTU, coalesced per parameter by the
  existing throttle. A filter type is written only on a band where CueMix 5
  offers it (band 1: bell, low shelf, high-pass; band 2: bell; band 3: bell,
  high shelf). Compressor, gate, reverb and clock stay read-only until each
  is checked on a card.
- Input EQ writes: the five input EQ props (1002-1006, 8 analog inputs x 4
  bands, index `ch x 4 + band`) are sent to the MOTU like the output EQ. Filter
  types follow CueMix 5 (band 1: bell, low shelf, high-pass; bands 2-3: bell;
  band 4: bell, high shelf); the rule is shared with the output EQ (3 bands:
  first / middle / last).

### Removed
- The second "Advanced controls" link at the bottom of the phone view (the
  header's mode button remains), and Mono / Talkback from the phone view.

### Fixed
- Output EQ slot labelling. The device has 12 EQ slots, not 12 mono outputs:
  Main L-R, an unused slot (never shown or written), Line 3 … Line 10, Phones
  L-R and the Reverb. The Output tab now names them that way and a preset or
  edit applies to the selected slot only (it used to treat slots as L/R pairs,
  so "Phones R" edited the reverb EQ).
- Double-clicking or double-tapping a volume knob (Monitor, Phones) no longer
  resets it to 0 dB, which was a jump to full volume. Those knobs have no reset.
- Double-clicking or double-tapping an output trim knob (Output tab) no longer
  resets it to 0 dB (no attenuation, full level). Trim knobs have no reset.
- Proxmox: the MOTU USB link no longer drops off its isolated bridge after a
  host network reload. ifupdown2 (`ifreload -a`, or "Apply Configuration" in
  the Proxmox GUI) detaches every bridge port not listed in `bridge-ports`,
  and the MOTU link was attached only by the udev `add` rule, so the app fell
  back to demo mode until the helper was re-run by hand. The bridge stanza now
  has a `post-up` hook that runs `cuemix-motu-bridge --all`, which finds any
  MOTU link (USB vendor 07fd, `cdc_ether`) in sysfs and re-attaches it (no-op
  without one). `proxmox-setup.sh --update` now also refreshes the host helper,
  udev rule and bridge file, without a network reload.

### Changed
- Phone (Simple) view redesigned after the desktop Monitoring card: two large
  rotary knobs, Monitor (Main) and Phones, side by side, each with its dB value
  and a pair of vertical meters. The knobs resize with the screen width
  (stacked under 340 px), the drag travel scales with the knob size and a drag
  works up or right for more. Each card is one unit: its name, knob, value and
  meters, with an on/off button at the bottom (a speaker icon for Monitor, a
  headphones icon for Phones). Dragging anywhere on the card turns its knob. Off
  remembers the level (in the browser, separately for Monitor and Phones) and
  sweeps the knob all the way down to the minimum (-100 dB) in up to 1.2 s; On
  sweeps back up to that level and never above it (to -70 dB when none is
  remembered). Touching the card cancels a fade, and a fade stops by itself if
  the volume is changed elsewhere (another client, OSC, a preset) or the
  connection drops. The two units are centred vertically in the view.
- Output EQ defaults match CueMix 5 (low shelf 60 Hz, bell 800 Hz, high shelf
  14 kHz, 0 dB, Q 1); the *Flat* preset uses the same values. OSC
  `/output/{n}/eq/…` takes the 1-based EQ slot (1 = Main, 3-10 = Line 3-10,
  11 = Phones, 12 = Reverb; 2 is unused).
- Input EQ defaults match CueMix 5 (low shelf 60 Hz, bell 400 Hz, bell 2 kHz,
  high shelf 14 kHz, 0 dB, Q 1).
- Compatibility: preset recall now also applies the input and output EQ to the
  card. A preset saved before this version holds the EQ the app had at the
  time: the card's values if it was saved while the card was connected, or the
  app's old defaults (input: 45 Hz high-pass, flat bells; output: flat bells)
  if saved in demo mode. Recalling it replaces the EQ stored on the card. EQ values the card never gets (the unused output slot, a filter type
  the band does not offer) are rejected from OSC and repaired to the defaults
  when a preset is imported or recalled.
- Known limitation: on a stereo line pair CueMix only uses the left slot (the
  odd-numbered line), so edit Line 3/5/7/9 for stereo pairs. The UI does not
  hide the right slot yet. The same holds for the input EQ: CueMix shows one
  strip, with the left channel's EQ, for a stereo input pair; the right
  channel's EQ is not used by it (what the firmware does with the right
  channel's EQ is not verified on hardware).

### Verified on hardware
- UltraLite-mk5 (firmware 2.0.8): a Main EQ bell written at 1 kHz, −10 dB,
  Q 1 is audible, and the card reports it back after a service restart (from
  its own connect dump).
- Measured on the card's meters (music on Line 7-8, speakers on Main): all
  three Main output EQ bands at −20 dB lower the Main output meter by about
  26 dB, with Phones unchanged (it has its own slot); all four input EQ bands
  at −20 dB on Line 7 and 8 lower their post-EQ meters by about 29 dB. Both
  return to their previous level once the original values are written back.

### Compatibility
- Presets: format unchanged (v2); 0.4.0 presets load as they are.
- `config.toml`: new optional `[measure]` section (MPD address for the room
  test); without it the room test is hidden. Older configs work unchanged.
- Deployment (Proxmox): run `deploy/proxmox-setup.sh --update` once to install
  the bridge `post-up` hook that keeps the MOTU link attached after a network
  reload.

## [0.4.0] - 2026-11-15

First version used day to day on a real UltraLite-mk5 (firmware 2.0.8).

### Added
- The device is identified from its connect dump: model, firmware, channel
  counts, stereo pairs and channel/mix names in CueMix 5 order (Mic 1 … Line 8,
  S/PDIF 1-2, ADAT 1-8, USB returns per mix, Reverb). The UI sizes itself to
  the device, shows one strip per stereo pair (flagging pairs whose halves
  differ on the device), lists the mixes like CueMix (Main, Phones, Line 3-4…)
  and shows the model and firmware in the top bar.
- Input and Output tabs, as in CueMix 5. Input is the former Preamps view.
  Output groups the monitor level, A/B, mono and talkback, the phones level,
  and a trim per analog output (new `output_trim` param, prop 5000 indices
  0-9, OSC `/output/{n}/trim`). Output trims are calibration: like the other
  trims they are stored in presets but never recalled.
- Mix faders, pan, mute and solo are read from and written to the MOTU
  (layout `bus × 32 + input`, from a real dump). A stereo strip moves both
  halves of its pair. Pan direction is still to be confirmed on a card.
- DSP, reverb and clock are read from the MOTU (layouts from a real connect
  dump): input EQ on analog inputs 1-8 (4 bands: type, bypass, frequency,
  gain, bandwidth; props 1002-1006, index `ch × 4 + band`), compressor on
  inputs 1-8 (bypass, threshold, attack, release, ratio, make-up; 1011-1015,
  1021), gate on mic 1-2 (1007-1010), output EQ on 12 outputs (3 bands;
  1022-1026, index `out × 3 + band`), reverb (decay, damping, pre-delay,
  width, preset; 1030-1034), sample rate (10) and clock source (11). The UI
  shows them as read-only ("changes here are not sent yet"): the encoders are
  written and unit-tested but switched off, so nothing new is sent to the
  device until each feature is checked on a card.
- Meter probe (diagnostic view, open the UI with `?probe`): shows the
  MOTU's raw meter frame (property 6000, 100 points) live, highlights the
  points that change against a silent baseline and copies a JSON snapshot.
  Used to map the meter layout before real meters are drawn. The server
  forwards the frames as a new `meter_raw` event (≈ 15/s).
- Level meters from the MOTU on an UltraLite-mk5: every mixer strip (after
  the input EQ/dynamics; both halves of a stereo pair), the selected mix's
  output beside the Main fader, each preamp on the Input tab (after the
  preamp, for gain staging), and each output, Main and Phones on the Output
  tab (before trim/volume). Layout of the meter frame (prop 6000, one byte
  per patch-bus point, −0.5 dB per step) confirmed with the meter probe.
- Settings files: Presets → *Export to file* downloads every setting
  (preset format, with the device model and firmware); *Import file…*
  stores a settings file as a preset and loads it. New WebSocket command
  `import_preset`; incoming message limit raised to 256 KiB.
- Input tab: S/PDIF and Optical in sections with their level meters, and
  the optical input format (ADAT 8 channels / TOS stereo, prop 5006,
  new param `optical_mode`, OSC `/optical/{in|out}/mode`).
- Updates from the web UI, by release channel: `[update] channel =
  "stable" | "prerelease"` in `config.toml`. A root systemd timer
  (`cuemix-update-check.timer`, daily) runs `cuemix-update --check
  --status-file …`; the UI shows "vX available" next to the version, with
  the release notes and an **Install** button. Installing is always a
  person's decision: the button only creates a request file that the root
  `cuemix-update-request.path` unit turns into a normal `cuemix-update` run
  (sha256 check, health check, automatic rollback). The web service itself
  stays unprivileged and never downloads anything. New endpoints
  `GET /api/update` and `POST /api/update/install` (needs the
  `x-cuemix-update` header). The setup scripts install the units
  (`proxmox-setup.sh --update` adds them to an existing container).
- Each view opens directly from the URL: `#mixer`, `#input`, `#output`,
  `#reverb` (kept in the address bar when switching tabs).
- Phone remote: on a phone the UI opens in a **Simple** view with large
  Main and Phones volume controls (slider, −/+ 1 dB, meters), Mono,
  Talkback and preset recall. **Advanced** (button in the header) shows
  every control; the choice is remembered per browser. The full console
  also fits small screens: tabs on their own row, compact Control Room
  bar, Output and EQ sections that wrap.
- Browser tab icon.
- `Int16` payloads (`ValueKind::I16`) for the DSP attack/release times.
- OSC addresses for all of the above: `/eq/{ch}/band/{b}/{mode|bypass|freq|gain|bw}`,
  `/output/{n}/eq/band/{b}/…`, `/comp/{ch}/…`, `/gate/{ch}/…`,
  `/reverb/{decay|damping|predelay|width|preset}`, `/clock/rate`,
  `/clock/source`.
- Output tab: compact output EQ (pick an output, curve + band table), with
  presets for home listening (Flat, Voice clarity, Night / dialogue,
  Loudness, Bass boost, Tame harshness) applied to the selected output pair.
  Like the rest of the output EQ, they reach the MOTU once EQ writes are
  enabled.

### Changed
- The Routing tab is gone: it was never wired to the device, and its
  input × output grid did not match the hardware (each output takes one
  source, prop 5010). The mixes cover normal use; a per-output source menu
  can come later on the Output tab. The `route` param and the state's
  routing matrix stay (state only) for preset compatibility.
- Preset recall with an identified MOTU sends only the parameters that differ
  from the live state (which mirrors the hardware), instead of every mapped
  parameter.
- **Preset format v2** (`schema_version: 2`). The DSP and reverb part of the
  state now matches the hardware: `input_eq`, `compressors`, `gates`,
  `output_eq` and a `reverb` block with decay / damping / pre-delay / width /
  preset, replacing the per-mixer-input `channels` (EQ + HPF + compressor +
  gate on all 32 strips) and the old reverb (time, return, sends). One param
  per hardware property (e.g. `input_eq_freq`, `comp_makeup`, `sample_rate`,
  `clock_source`) replaces the block params `eq_band`, `hpf`, `comp`, `gate`,
  `reverb`, `reverb_send` and `clock`.
- Migration: v1 (and unversioned) presets still load. Mixes, preamps, trims,
  control room, routing, clock and labels are kept; the old DSP and reverb
  sections are dropped and replaced by the hardware defaults (they never
  matched a hardware setting). The file is rewritten as v2 on the next save.
- Reverb sends are the faders of the Reverb mix and the reverb return is the
  "Reverb" mixer input in every mix; the Reverb tab now shows those mix
  faders. Removed: the global EQ bypass (`eq_bypass`, OSC `/eq/{ch}/bypass`):
  the hardware only has a bypass per band.
- The mixer strip's DSP button only appears on strips that have DSP (analog
  inputs); the separate EQ toggle is gone. The gate section only shows on the
  mic inputs.

### Fixed
- EQ: the band's width value is a Q (0.45-10, as in CueMix 5), not a
  bandwidth in octaves; the curve now draws bells with the right width, the
  table shows "Q" and only for bells. Gain range ±20 dB. Each band offers
  the hardware's types: low shelf / HPF on the first band, high shelf on the
  last, bells in between. Output EQ presets use real shelves.
- Output trims (prop 5000) were written to the wrong outputs: the device
  indexes its DACs in hardware order (Line 5-10 first, Main 1-2 at 6-7).
  The trim of "output 1" actually changed Line 5. Now translated both ways.
- EQ band types: 1 is a low shelf and 2 a high shelf (was shown as
  "Type 1" / "Shelf"); the curve draws both.
- Fader thumbs no longer stick out past the end of the track and cover the
  pan readout (fader at the top) or the value (fader at the bottom).
- Reverb tab: the send faders overflowed their panel. Return and sends now
  sit side by side and fit.
- Mixer strips all have the same width (stereo strips were wider).
- More compact layout: the header and the view tabs share one bar (the
  Presets button shows the active preset), and the Output tab fits one
  screen (monitor, A/B, mono, talkback and phones in one card, trims as
  small knobs grouped by pair, EQ curve beside its bands).
- The Control Room bar is half as tall, and the version, model and firmware
  moved to its right end.
- Routing matrix: inputs and outputs carry their names (Mic 1 … ADAT 8,
  Main L … ADAT 8 on an UltraLite-mk5), grouped by connector (Analog,
  Phones, S/PDIF, ADAT) with separators, and the hovered route is spelled
  out ("Line 3 → Main L"). Routing is still not read from the device.
- Input tab: mic preamps and line inputs in two groups on one row, with
  smaller knobs and buttons.
- Output tab: the EQ curve grows to fill the free height.
- EQ editor (input DSP panel and output EQ) laid out like CueMix 5: a
  coloured column per band with an on/off button, Freq / Gain / Q and the
  band's filter-type icons. On the Output tab the curve stays on the left
  and the bands are compact rows on the right. Curve handles and labels
  keep a fixed on-screen size however wide the curve is.
- Writes to the MOTU had no effect: the device ignores frames without a
  payload length field. Writes are now `[id][index][len u16][payload]`
  (verified on an UltraLite-mk5: main volume follows the web UI). Reads are
  unchanged.
- `proxmox-setup.sh`: on hosts whose `/etc/network/interfaces` does not load
  `interfaces.d`, the MOTU bridge was never created and the container failed
  to start. The script now offers to source its bridge file (with a backup),
  stops otherwise, and checks the bridge exists after `ifreload`. The
  reverse-proxy note names the headers actually used (`X-Forwarded-Host`).
- udev rule never matched: `ATTRS{idVendor}` and `DRIVERS` must match the
  same parent device, but the vendor id and the `cdc_ether` driver live on
  different ones. It now matches `ENV{ID_VENDOR_ID}` / `ENV{ID_USB_DRIVER}`
  and passes the renamed interface (`$name`, not the pre-rename `%k`). Found
  on the first real hardware test (`udevadm test`).

### Verified on hardware
- An UltraLite-mk5 is discovered through its UDP announcement and accepts the
  WebSocket connection on port 1280 without a path.

### Compatibility
- Presets: format **v2**. v1 presets still load (mixes, preamps, trims,
  control room, labels kept; their old DSP section is dropped). Files are
  rewritten as v2 on the next save. A v2 preset is refused by 0.3.0.
- `config.toml`: new optional `[update]` section (channel and file paths);
  older configs work unchanged.
- Deployment: run `deploy/proxmox-setup.sh --update` (or `linux-setup.sh`)
  once to install the update units; without them the UI simply shows no
  update notice.

## [0.3.0] - 2026-11-14

First feature-complete version (Phases 1–4 of `docs/HANDOFF.md`), not yet
verified on a real UltraLite-mk5.

### Added
- Rust server: engine actor owning the device state, MOTU WebSocket client
  (discovery, hardware-priority sync, per-parameter 40 Hz throttle + 20 ms
  global pacing, reconnect, offline demo mode), bidirectional OSC bridge,
  JSON WebSocket API, presets and channel labels, mDNS announcement.
- Web UI (Svelte 5, dark theme): mixer with 7 buses × 32 strips, preamps,
  interactive 4-band EQ + HPF, compressor, gate, reverb, routing matrix,
  Control Room bar, presets manager, labels, meters, running version.
- Deployment: Proxmox VE LXC and Raspberry Pi / Linux setup scripts, systemd
  unit, udev rule, Docker image (amd64/arm64).
- Updates: `cuemix-update` with sha256 verification, atomic swap, health
  check and automatic rollback; `proxmox-setup.sh --update`.
- CI: tests, MSRV 1.85, secret scanning, shellcheck; security workflow (cargo
  audit, npm audit, dependency review, CodeQL) and Dependabot.
- Security: third-party GitHub Actions pinned to commit SHAs; the web
  client only accepts in-range integer indices from server payloads.

### Changed
- Minimum supported Rust version raised to **1.85** (dependency upgrades:
  tokio 1.54, toml 1.x, tokio-tungstenite 0.30, rosc 0.11, thiserror 2,
  if-addrs 0.15, mdns-sd 0.21).

### Compatibility
- Presets: format v1 (first versioned format).
- Hardware: mapped and unverified — preamp gain/phase, pad, +48V, main and
  headphone trims, mono, talkback. Mix faders, DSP, routing and meters are
  state-only until the protocol is confirmed on a device.
