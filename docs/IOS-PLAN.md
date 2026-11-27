# Cue Desk (iOS app for cuemix-web) — plan

Status: **approved by the maintainer on 2026-11-16** (draft v7, reviewed twice: once against the code, once for iOS feasibility and
process). Nothing here is built yet. It adds to
cuemix-web; it does not replace it.

## Decisions so far (2026-11-16)

| # | Question | Decision |
|---|---|---|
| Q1 | Away from home | **Not needed.** Home network only: no public access, no app tokens, no pairing (§5.3) |
| Q2 | UI approach | **Hybrid**: native Simple view, controls, widgets, Siri; Advanced view = the server's web UI inside the app (§3) |
| Q3 | Minimum iOS | **iOS 27** |
| Q4 | Advanced tabs | **Native tabs at the bottom**, each opening the matching web page |
| Q5 | Widget push | **Later, optional** (phase 9); decide after living with the widgets |
| Q6 | On/off | **On the server** (`Power` command, server-side fade, shared remembered level, §5.2) |
| Q7 | Build path | **GitHub Actions + TestFlight**, Apple Developer Program; your Mac for one-time setup and device logs |
| Q8 | Name | **Cue Desk**. "CueMix", "MOTU" and "UltraLite" are MOTU's names: not in the app name or icon; the description may say "for the MOTU UltraLite-mk5" |
| Q9 | Extras | **iPad layout** and **Apple Watch** after 1.0 (phase 11). No Live Activity for now |
| Q10 | Room test | **In the app**, with the microphone permission, through the web view (phase 8) |
| Q11 | Phone-width web | **Bottom tab bar** like the app; desktop keeps the top tabs |
| Q13 | Look | **iOS system style for the frame** (tab bar, sheets, settings), **shared tokens for the content** (cards, knobs, meters, colours) |
| Q14 | Browsers' remembered levels | **Dropped** at server 0.6.0, no import (worst case: one On fades to -70 dB) |
| Q12 | Quick toggles | **In milestone 1**: Control Center / Lock Screen toggles ship with the Simple view app |
| — | HTTPS | **Verified 2026-11-16**: the home address is served over HTTPS by the proxy with a publicly trusted certificate, TLS 1.3; the WebSocket upgrade works for a native client. Meets App Transport Security with no exception. The name resolves to a LAN address through the home DNS, so the phone must use the home Wi-Fi's DNS (no custom DNS profile) |

All questions are answered.

---

## 1. Goal

An iPhone app for quick, reliable access to the MOTU from home, plus everything else cuemix-web
does (the deep editing through the server's own web UI inside the app, §3):

- **First and most important**: the Simple view (Monitor and Phones knobs, meters, on/off with
  fades, presets), opening instantly.
- **Control Center and Lock Screen controls** (the panel that slides down from the top right):
  Monitor on/off, Phones on/off, recall a preset, open the app.
- **Lock Screen and Home Screen widgets** that show the levels.
- **Shortcuts and Siri**: set a volume, toggle a channel, recall a preset, read the levels.
- Fast to open, fast to respond, small.

It keeps what you already have: open the web UI on any device at home, see the same live state,
change it there, and every other device follows.

Target: iPhone 14, iOS 27.2. Minimum iOS: 27.

## 2. The key choice: the server stays the single source of truth

The MOTU is plugged into the Proxmox container by USB. An iPhone cannot talk to it directly,
and should not try: two masters would fight. So the app is **one more client of the cuemix-web
server**, like a browser or TouchOSC.

```text
 Cue Desk app ──┐             ┌─ browsers (any device at home)
 controls ──────┤  HTTPS/WSS  │  TouchOSC / QLab (OSC)
 widgets ───────┤  (home LAN) │
 Siri/Shortcuts ┘             │
                ▼             ▼
        cuemix-web server (Proxmox CT) ── USB ── MOTU UltraLite-mk5
        single owner of the state, broadcasts every change to every client
```

What this gives for free:

- A change made on the phone shows up live in every open browser, and the other way round.
  The engine already broadcasts every effective change to all clients (ARCHITECTURE §3.4).
- No sync logic and no conflicts. The phone keeps only a small cache for controls and widgets.

What it costs:

- Nothing works when the server is unreachable: off home Wi-Fi, server down, or the MOTU
  unplugged. The app, controls and widgets then show the last known values greyed and say why
  (§4.5).

## 3. How the app is built: hybrid (decided)

cuemix-web's UI is about 7,600 lines of Svelte (19 components). The app is:

- **Native SwiftUI** for what you touch daily: the Simple view, controls, widgets, Siri.
- **The server's own web UI inside the app** (`WKWebView`) for the Advanced view: mixer, input,
  output, EQ, dynamics, probe and room test. It always matches the server's version, and a
  server update needs no app update. Any Advanced screen can later move to native, one at a time,
  if it feels too "web" (EQ is the likely first).

Limits of the web view, handled in phase 8:

- **Room test**: the analysis runs in the browser code (ARCHITECTURE D28), so it runs in the web
  view, not natively. It needs HTTPS (in place), the iOS microphone permission, and the app kept
  open: the server stops a test when the client that started it disconnects.
- **Two connections**: the web view opens its own `/ws`. The app pauses it (and its meters) when
  the Advanced tab is not visible, so the phone never streams meters twice.

## 4. What the app contains

### 4.1 App

- **Simple view (native)**: the two knobs (Monitor, Phones) with their meters, the two on/off
  buttons (the fade runs on the server, §5.2), the preset picker. Same content and look as the
  web Simple view (§6b). The room test button appears in phase 8, when the room test is
  available in the app.
  - Haptic tick every 1 dB while turning, not every frame.
  - VoiceOver: each knob is "adjustable", 1 dB per swipe.
  - Dynamic Type: at the largest accessibility sizes the two knobs stack vertically, like the
    web does on narrow screens.
  - No double-tap reset on the volume knobs (the web Simple view has none; a volume must never
    jump up).
- **Advanced view**: the server's web UI in a `WKWebView`, opened with `?embed` (its own
  Simple/Advanced toggle and status dot hidden, safe areas respected). Native tabs at the bottom
  open each page (Mixer, Input, Output, …).
- **Settings**: the server address (typed once, `https://` only), a connection test, the server
  version, and the update badge when a server update is available (as in the web UI).
- **Meters** at about 30 per second, which is what the server already sends for this card
  (`meter_raw`), drawn with SwiftUI `Canvas`, decoded with the same patch-aware table as the web
  (port of `meterMap.ts`).
- **Lifecycle**: the WebSocket closes when the app goes to the background and reconnects with
  backoff when it comes back. Cached values show instantly in the meantime.

### 4.2 Widgets (WidgetKit) — milestone 2

- **Lock Screen**: circular gauge (Monitor or Phones level), rectangular (both levels and on/off
  state), inline (one line above the clock).
- **Home Screen**: small (one channel with its on/off button) and medium (both channels, on/off,
  ±3 dB, preset).
- Buttons in widgets run App Intents without opening the app.
- Tinted and clear Lock/Home Screen styles supported.
- **Freshness, honestly**: iOS decides when widgets refresh (a budget of roughly 40–70 per day,
  so often every 15–60 minutes, never guaranteed). A widget refreshes right away after you press
  one of its buttons, and the app refreshes them whenever it sees a change. After a change made on
  another device while the app is closed, a widget can be stale until the next refresh. The
  optional push of §5.5 improves that, but iOS also budgets pushes ("usually soon", not instant).

### 4.3 Controls (Control Center and Lock Screen) — milestone 1

- `Monitor` toggle and `Phones` toggle; `Recall preset` (you choose the preset when adding it);
  `Open Cue Desk`.
- **How a toggle behaves**:
  - Tap: the control shows the new state at once. The intent sends `Power` to the server and
    returns as soon as the server accepts it; the fade (250–1200 ms) then runs on the server, so
    the phone never waits for it.
  - If the server refuses or cannot be reached within 2 s, the control flips back and iOS shows
    the error ("Not on home network", "Server unreachable", "MOTU offline").
  - Displayed state: asked from the server when iOS draws the control (2 s limit), otherwise the
    last cached value. iOS may cache it, so after a change on another device a control can be
    briefly stale.
- The same controls can replace the flashlight/camera buttons at the bottom of the Lock Screen.

### 4.4 Shortcuts and Siri (App Intents) — milestone 2

`Set Monitor volume`, `Set Phones volume`, `Change volume by` (± dB), `Turn Monitor on/off`,
`Turn Phones on/off`, `Recall preset`, `Get levels`. Suggested phrases such as "Monitor off in
Cue Desk". They appear in Spotlight and the Shortcuts app. (The iPhone 14 has no Action button.)

### 4.5 When things go wrong (applies to app, controls, widgets, Siri)

| Situation | What you see |
|---|---|
| App never configured | Controls, widgets and Siri say "Open Cue Desk to set it up" |
| Local Network permission not granted yet, or refused | "Open Cue Desk once" (the permission can only be asked by the app itself, §5.3), or a link to the iOS setting |
| Off home Wi-Fi (cellular, other network) | "Not on home network", last values greyed |
| Server unreachable | "Server unreachable", last values greyed, the app retries |
| Server reachable, MOTU offline | "MOTU offline" (the server's own status), knobs disabled |
| Certificate problem | "Certificate problem", never silently ignored |
| Server too old / app too old | "Update the server" / "Update Cue Desk" (§5.1 `api` range) |

All requests from controls, widgets and Siri give up after 2 s, so nothing hangs.

## 5. Changes to the server (Rust)

The browser UI keeps working. One visible change on the web: on/off becomes shared (§5.2).
Everything ships as **cuemix-web 0.6.0** (phase 1), with the usual release steps (version in
`Cargo.toml` = tag, CHANGELOG with a Compatibility section, README, ARCHITECTURE).

### 5.1 Short HTTP API for controls, widgets and Siri

Controls and intents run for a few seconds in the background; opening a WebSocket and waiting for
the full snapshot is too slow for that. New routes:

- `GET /api/v1/summary`: Monitor and Phones level, their on/off and fading state, active preset,
  preset list, MOTU status, server version.
- `POST /api/v1/command`: only the commands quick access needs: set `main_vol` / `phones_vol`,
  `power`, `load_preset`. Everything else is refused (400). In particular the room test can never
  start from it: a test is tied to the WebSocket client that started it. Clients must send
  `Content-Type: application/json` (415 otherwise) and a body of at most 16 KiB (413 otherwise).
  - It answers with the real result (200, or 4xx with the reason). Today the engine sends errors
    only over the WebSocket of the client that caused them, so phase 1 adds an engine request that
    returns the result to the HTTP route.
  - The change is broadcast to every client as usual.
- Both routes use the same cross-site `Origin` check as `/ws` (no `Origin` header, as from the
  app, is accepted).
- `GET /api/health` gains `api` (an integer). The app declares the range it supports; outside it,
  the app says "Update the server" or "Update Cue Desk". The app's decoders ignore unknown events
  and fields, so a newer server inside the range never breaks it.

### 5.2 On/off moves to the server (decided)

Today on/off and the "remembered level" live in each browser's `localStorage`, and the fade runs
in the browser. Turn Monitor off from the phone and the laptop does not know the level to come back
to; a Control Center toggle could not run a fade at all. So:

- **New command** `power { channel: main | phones, on: bool }`.
- **The engine runs the fade.** The engine is an actor that only reacts to messages today, so it
  gets one `Option<Fade>` per channel and a timer branch in its main loop (`sleep_until` the next
  step). Same curve as now: smoothstep, 12 ms per dB, 250–1200 ms, a step every 28 ms (under the
  40 Hz per-parameter write limit). Fade steps are written as server-originated changes, so every
  client, including the one that pressed the button, sees the knob move.
- **What cancels a fade**: a `set` of that volume from any client or OSC, or a value from the
  hardware more than 0.5 dB away from the last step (the same tolerance as the web today, so the
  hardware's own echo of each step never cancels it). Presets never touch these volumes (they are
  excluded from recall, D13), so a preset does not cancel a fade.
- **Remembered level**, the same rules as the web today:
  - Off remembers the current level, unless a fade is already running.
  - On fades to the remembered level, then forgets it (the next Off remembers afresh).
  - A knob move to a real level (not the minimum) forgets it.
  - With nothing remembered, On fades to the safe level (-70 dB, `FADE_SAFE_DB`).
  - It is saved in a small server file next to the labels file, survives restarts, and is never
    part of presets. New config key, Docker volume and `.gitignore` entry as for the other data
    files.
  - No migration from browsers: their `localStorage` values are dropped (noted in the CHANGELOG
    Compatibility section). The worst case is one On that fades to -70 dB instead of the old level.
- **On/off is derived from the level** (at the minimum = off), so raising the volume from anywhere
  turns it back on naturally. Fading state comes from the engine.
- **New event** `power { main: {on, fading}, phones: {on, fading} }`, sent with the initial
  events on connect and on every change. It is not part of the device state, so it never sets the
  "Modified" indicator and never lands in a preset file.
- **OSC**: `/mix/main/power` and `/controlroom/phones/power` (following the existing
  `/mix/main/vol` and `/controlroom/phones/vol`), with feedback and dump support.
- **Web**: the Simple view's buttons send `power` and show the server's state; `channelFade` and
  the browser storage keys are removed.
- **Docs**: a new ARCHITECTURE decision entry, §3.5 (JSON), §3.6 (OSC, also fixing the stale
  meter rate there), §3.7 (config); HANDOFF §3.1 OSC table; README ("Main on/off" wording).

### 5.2b Speaker plug (decided 2026-11-25, ships with 0.6.0)

Monitor (Main) on/off can also switch a smart plug that powers the speakers (for example a
Shelly), in the right order so the speakers never pop and never stay on at full volume.

- **Sequence (run by the engine, so it works from any client, OSC or `/api/v1/command`):**
  - **Off:** fade out (as §5.2) → when the fade ends at the minimum, wait `off_delay_ms`
    (default 300) → plug off. A fade cancelled by a knob move leaves the plug on.
  - **On:** plug on → if it answered, wait `warmup_ms` (default 2000, adjustable) → fade in.
    If it did not answer within the timeout: fade in right away and send a warning
    ("Speaker plug did not respond"). An Off pressed during the warm-up cancels it and
    switches the plug off.
  - Only the `power` command drives the plug. A knob move never switches it (the Simple view
    shows the plug state, so a raised knob with the plug off is visible).
  - Phones never use the plug.
- **Plug types (presets), host + switch id `N`:**
  - `shelly_rpc`: Shelly Gen2, Plus, Pro, Gen3 and **Gen4** — `http://HOST/rpc/Switch.Set?id=N&on=true|false`.
  - `shelly_gen1`: `http://HOST/relay/N?turn=on|off`.
  - `tasmota`: `http://HOST/cm?cmnd=Power%20On|Off` (`Power<N+1>` when N > 0).
  - `custom`: two full URLs (on, off).
- **Request rules (the server is told where to send requests, so it is fenced):** plain
  `http://` GET only (local smart plugs do not do HTTPS), no redirects followed, timeout
  `timeout_ms` (default 2000), response body ignored apart from the status line (2xx = OK).
  The target must resolve to a private, link-local or loopback address (RFC 1918, 169.254/16,
  127/8, IPv6 ULA/link-local); anything else is refused, at save time and again at send time.
  No credentials (plugs with a password are out of scope for now).
- **Where the settings live:** `[speaker_plug]` in `config.toml` gives the defaults
  (`enabled = false` in `config.example.toml`, example host `192.0.2.10`). The Advanced view's
  menu edits them; edits are saved in a server file next to the labels file
  (`speaker-plug.json`), which overrides `config.toml`. **Real addresses never go in Git.**
- **Protocol:**
  - Commands: `set_speaker_plug { settings }` (validated, saved, broadcast) and
    `test_speaker_plug { on }` (sends one request, answers with a result).
  - Event `speaker_plug { settings, state }` on connect and on every change, where `state` is
    `off | on | warming | error | unknown` (last known result of our own requests; the plug is
    not polled).
  - The `power` event's `main` gains `plug` (the same state, or `null` when no plug is set).
- **UI:** an Advanced-menu entry "Speaker plug": enable switch, type, host, switch id (or the
  two URLs for custom), warm-up, the resulting URLs shown read-only, **Test on / Test off**,
  Save. The Simple view's Monitor button shows a small plug indicator and the warming state.

### 5.3 Home network only, HTTPS, Local Network permission (decided)

The app is used at home only: no public access, no tokens, no pairing; the server's security model
(ARCHITECTURE §6) is unchanged.

- **HTTPS**: the home address is served over HTTPS with a trusted certificate (checked
  2026-11-16), so App Transport Security needs no exception and the room test microphone works.
  The app accepts only `https://` addresses.
- **Local Network permission**: the address resolves to a LAN address, so iOS asks for the Local
  Network permission. Only the app can trigger that prompt; controls and widgets inherit the app's
  answer. So the app connects on first launch (triggering the prompt) before you add any control.
  There are reports of extensions failing on a hostname that resolves to a private address even
  after the app was allowed. **This is the biggest technical risk** (§12), so phase 4's first
  TestFlight build includes one control that calls `/api/v1/summary`, to prove it on your
  iPhone before anything else is built on it.
- **No Bonjour discovery**: the server's announcement is plain `http` on the container's port,
  which iOS blocks and which does not match the certificate. You type the `https://` address once
  in Settings.

### 5.4 Widget push (optional, phase 9)

If chosen later: the server sends an Apple push when Monitor/Phones/preset change, so widgets and
controls refresh soon after a change from another device (iOS budgets these). Needs the
container to reach Apple's push servers, and a push key from your developer account, stored only
in the container's `config.toml`.

## 6. Code layout

```text
ios/
  project.yml            XcodeGen spec: the Xcode project is generated, never hand-edited
  CuemixKit/             Swift package, Foundation only (no UIKit)
    Protocol/            Codable models mirroring src/state.rs; unknown events/fields ignored
    Client/              connection logic (reconnect, backoff) behind a small transport
                         interface; HTTP client for /api/v1. The real WebSocket transport is
                         Apple-only (Linux Foundation lacks it); Linux tests use a fake one
    Meters/              meter_raw decoding + patch-aware table (port of meterMap.ts)
    Knob/                knob drag/step maths and dB formatting
  App/                   SwiftUI app: Simple view, Advanced web view, settings
  Widgets/               widget extension: controls + widgets
  Intents/               App Intents shared by the app, controls, widgets and Siri
  Tests/                 unit, snapshot and UI tests
tests/fixtures/protocol/ JSON samples shared by the Rust, TypeScript and Swift tests
design/                  tokens.json, strings.json, behaviour vectors (§6b) + generator
docs/UI-PARITY.md        the web ↔ iOS parity map (§6b)
```

- No third-party Swift packages: Apple frameworks only.
- App, controls, widgets and intents share the server address and the last known summary through
  an **App Group** (shared `UserDefaults`). The address is not a secret, so no Keychain is needed.
- Identifiers: `io.github.lejeanf.cuedesk` (app), `.widgets` (extension), App Group
  `group.io.github.lejeanf.cuedesk`. No real hostname or address in any tracked file; fixtures and
  defaults use `.example` names and 192.0.2.x.
- Required files: `PrivacyInfo.xcprivacy` (the app uses `UserDefaults`, an API Apple requires a
  reason for), `ITSAppUsesNonExemptEncryption = NO` (only standard HTTPS), usage texts for the
  Local Network and the microphone.

## 6b. Web and iOS stay in step (one product, two surfaces)

The Advanced view is the web UI itself, so it cannot drift. What can drift is the **native Simple
view** (and the look of controls and widgets) against the **web Simple view**. Five mechanisms
keep them together.

**1. One design source, generated for both.** `design/tokens.json` holds the colours (the blue
`#3b93e0`, greys, meter green/yellow/red), radii, spacing, font sizes and knob geometry (270° arc,
track and arc widths, drag travel). A script generates the `:root` block of
`frontend/src/app.css`, a `tokens.ts`, and `Tokens.swift`. It keeps the existing CSS names
(`--cyan` stays the name of the blue, `--card`, `--meter-gradient`…), so no web component changes.
Nobody edits generated parts; CI fails if they are out of date. HANDOFF §3.2 (which still lists
the old cyan) is updated to point at `tokens.json`. The fade timings are **not** in the tokens:
after phase 1 the fade runs only on the server, so its constants live in Rust.

**2. One text catalogue.** Plain labels ("Monitor", "Phones", "On", "Off", "Fading out…", error
messages of §4.5) come from `design/strings.json`, generated into the web's strings and the app's
string catalogue. Text built by functions (such as EQ band rows) stays hand-written on each side,
checked by the vectors below.

**3. One behaviour, checked by shared test vectors.** JSON test cases run by vitest on the web and
by Swift tests in the app: knob drag distance → value, wheel and arrow steps, "no reset on volume
knobs", dB text formatting, meter decoding and colours. The fade curve vectors run in `cargo test`
(the only place the fade runs). If one side changes a behaviour without the other, its tests fail.

**4. A parity map and a CI rule.** `docs/UI-PARITY.md` lists each mirrored pair:

| Web | iOS | Mirrored |
|---|---|---|
| `Remote.svelte` (Simple view) | `SimpleView.swift` | layout, knobs, meters, on/off, presets (room test button from phase 8) |
| `Knob.svelte` | `KnobView.swift` | look, drag, accessibility |
| `Meter.svelte` | `MeterView.swift` | colours, scale, peak hold |
| `TopBar.svelte` status dot | app status dot | states and colours |
| web tabs (phone width) | native bottom tabs | order, names, icons |

The CI rule starts in phase 5, when both sides exist. It pairs **paths** (not whole stores): a PR
that changes one side of a pair without the other fails unless it carries the label `parity:web-only`
or `parity:ios-only` and says why in its description (for example haptics, which iPhone browsers
lack). It re-runs when the PR is opened, updated or its description edited. Known gaps are listed
in the same file. Controls, widgets and Siri are iOS-only by nature.

**5. Side-by-side screenshots in every UI PR.** The macOS CI job runs the server in demo mode with
a fixed state, takes the web Simple view in WebKit at iPhone 14 size (390 × 844), and the native
Simple view in an iPhone simulator of the same size, side by side in one image. The phase's audit
agent reviews it, I check it, and you see it in the PR. The goal is "looks like the same app", not
pixel equality.

**Look: iOS system style for the frame, shared tokens for the content (decided, Q13).** Built with Xcode
27, the app's tab bar, sheets and navigation automatically get iOS 27's glass look. Copying that
in the browser is not possible, and fighting it in the app makes it feel foreign. Proposal: the
**frame** (tab bar, sheets, settings) uses the iOS system style, and the **content** (cards, knobs,
meters, buttons, colours) uses the shared tokens, identical on both. The web's phone-width bottom
tab bar matches the app's in order, names and icons, not in glass effect.

**What this changes on the web side:**

- On phone-width screens, a bottom tab bar replaces the top tabs (Q11). Desktop keeps the top tabs.
- `?embed` (for the app's web view): own top bar parts hidden, safe areas respected.
- The rule for agents: **one feature, one PR, both surfaces.**

## 7. Building without switching to your Mac

**GitHub Actions builds, TestFlight installs.**

- The repo is public, so GitHub's macOS runners are free. Their Xcode 27 image is available
  (exact runner label and simulator model confirmed in phase 4; if no iPhone 14 simulator ships
  with Xcode 27, the nearest 390 × 844 model is used).
- Every push to an iOS branch: build, run all tests in the simulator, save screenshots of each
  screen as build artifacts (how I check the UI without a Mac). Expect 20–40 minutes per run.
- A tag `ios-vX.Y.Z` (separate from the server's `vX.Y.Z` tags, which keep their own release
  workflow): build, sign, upload to TestFlight. You install from the TestFlight app; no Mac, no
  cable. Build number = the CI run number, so it always increases.
- TestFlight builds expire after 90 days, so a scheduled monthly job uploads a fresh build of
  the latest tag.
- Signing: Xcode's cloud-managed signing with an App Store Connect API key (role: Admin, needed
  for that). Secrets are used only in the tag and scheduled workflows, never in pull-request runs,
  masked in logs; `pull_request_target` is never used; `ios-v*` tags are protected.
- **Tested on your PC as well**: CuemixKit's logic builds and tests with Swift on Linux in WSL, in
  seconds. The real WebSocket is tested only on macOS/iOS.

**Your Mac's role**: one-time steps that are easier in Xcode, and live logs from the phone when
something only goes wrong on the device. A macOS VM on Proxmox is dropped: Apple's licence allows
macOS only on Apple hardware, and the simulator would be very slow.

**One-time setup only you can do** (passwords, payment and keys are yours; I never handle them):

1. Join the Apple Developer Program (can take a few days; phases 1–3 do not need it, §9).
2. Register the identifiers of §6 and the App Group, and create the app record in App Store Connect
   (I give you the exact values and clicks).
3. Create the App Store Connect API key (Admin) and add it to the repo's GitHub secrets yourself.
4. In App Store Connect, add yourself as an internal tester (no Apple review needed for that).
5. Install TestFlight on the iPhone.

## 8. Speed and size targets

| What | Target | How it is checked |
|---|---|---|
| Launch to usable Simple view (cached values) | under 0.4 s | launch-time UI test in CI, then on your iPhone |
| Live values after launch, home Wi-Fi | under 0.3 s more | timing log in the app's debug build |
| Control or widget tap to fade start, home Wi-Fi | under 0.3 s | timing log; the fade itself then takes 0.25–1.2 s |
| Meters | 30 per second, hitch rate under 5 ms/s on an iPhone 14 | Instruments on your Mac, once in phase 9 |
| App download size | under 5 MB | App Store Connect size report |
| Battery | nothing streams in the background | WebSocket closed on background (UI test) |

## 9. Phases

Each phase: implementation agents (Sonnet) on a dedicated branch, audit agents (Opus) checking
quality and conformity, my own check, then a PR you review. Nothing is pushed to `main` directly.
Estimates are in **working sessions with me** (one session ≈ one evening like today), not
guaranteed.

**Priority: the Simple view and quick access first.** Each milestone is usable on its own.

### Milestone 0 — the web Simple view on your home screen (web only, one PR, ~1 session)

"Add to Home Screen" in Safari opens the Simple view full screen like an app. No Apple account
needed, so it is also the fallback if enrollment is slow.

Delivers:

- `frontend/public/` with a web app manifest (`display: standalone`, relative `start_url`
  `./?simple` inside its `scope`, so it also works behind a proxy subpath) and 180 px
  `apple-touch-icon` PNG (the current icon is an SVG, which the home screen does not use).
- `viewport-fit=cover` and safe-area padding (today `env(safe-area-*)` is 0 and the page could
  sit under the clock and home bar); `theme-color` aligned with the background.
- `?simple` wins over the remembered view without being saved (today the remembered view in
  storage always wins); `?embed` defined for phase 8.
- Reconnect immediately when the page comes back from lock or background (`visibilitychange` /
  `pageshow`).
- Note: a home-screen web app has its own storage, separate from Safari: its view choice and
  on/off remembered level start empty. On/off becomes shared in phase 1 anyway.

Done when (on your iPhone): it installs with the icon; opens in the Simple view full screen in
under a second; nothing sits under the clock or home bar; after locking the phone for a minute and
unlocking, live values return within 2 s.

### Milestone 1 — the native Simple view + quick toggles (Cue Desk 0.1)

| # | Phase | Delivers | Done when | Est. |
|---|---|---|---|---|
| 0 | Accounts (you) | Developer account, identifiers, API key in GitHub secrets, internal tester | You confirm; runs **in parallel** with phases 1–3 | — |
| 1 | Server 0.6.0 | `power` + server-side fade + remembered level (§5.2), `/api/v1/*` (§5.1), `api` in health, OSC, web Simple view switched, docs | Rust/TS tests green; on your setup: Off on the phone fades and the laptop shows Off; restart keeps the remembered level; released as 0.6.0 | 2–3 |
| 2 | Shared design (Simple view scope) | tokens + strings generator, behaviour vectors, protocol fixtures for the events the Simple view uses, parity map | Web screenshots identical before/after; CI fails on a hand-edited generated part | 1–2 |
| 3 | CuemixKit | Swift models, clients, meter decoding, knob maths | `swift test` green on Linux and macOS, including the shared fixtures | 1–2 |
| 4 | Pipeline + risk check | iOS CI, simulator tests, screenshots, TestFlight upload; "hello" build with **one control calling `/api/v1/summary`** | You install it from TestFlight; the control shows the live Monitor level on the Lock Screen (§5.3 risk proven) | 1–2 |
| 5 | Simple view | Native Simple view; side-by-side screenshot job; parity CI rule on | Checklist on your iPhone: both knobs move the volume; meters live; on/off fades; preset recall; a change in a browser shows in the app within 1 s and the other way round; screenshots reviewed side by side | 2–3 |
| 6 | Quick toggles | Control Center and Lock Screen controls (§4.3), error states (§4.5) | Checklist: toggles fade Monitor and Phones; off Wi-Fi they say "Not on home network" within 2 s; preset control recalls | 1–2 |

### Milestone 2 — widgets and Siri (Cue Desk 0.2)

| # | Phase | Delivers | Done when | Est. |
|---|---|---|---|---|
| 7 | Widgets, Siri | §4.2, §4.4 | Checklist: each widget size shows levels and its buttons work; three Siri phrases work | 2 |

### Milestone 3 — everything else (Cue Desk 1.0)

| # | Phase | Delivers | Done when | Est. |
|---|---|---|---|---|
| 8 | Advanced view | Native bottom tabs opening the web pages (`?embed`), phone-width web bottom tabs, room test with the microphone, web view `/ws` paused when hidden | Every Advanced feature works in the app; a room test completes on your iPhone | 2–3 |
| 9 | Polish (+ widget push if chosen) | §8 checks, accessibility pass, icon (two or three proposals for you to pick) | Targets met; audit clean | 1–2 |
| 10 | Release | `ios-v1.0.0`; README, ARCHITECTURE, CHANGELOG | You accept | 1 |
| 11 | After 1.0 | iPad layout; Apple Watch app | Planned separately | — |

## 10. Versions, rollback, approvals

- **Versions**: the server keeps `vX.Y.Z` tags; the app uses `ios-vX.Y.Z`. The app supports a
  range of the server's `api` (§5.1) and says which side to update.
- **Rollback**: if server 0.6.0 misbehaves, the previous release is reinstalled with the existing
  updater (`cuemix-update` with a version) or the hotfix procedure; the server's remembered-level
  file is simply ignored by older versions. A bad app build: TestFlight lets you install the
  previous build.
- **You approve**: this plan; every PR before merge; the App Store Connect values; the icon; each
  milestone's device checklist.

## 11. Tests and quality control

- **Rust**: new routes (allowed and refused commands, results, `Origin` rule); the fade (timing,
  curve vectors, cancel rules including the hardware echo tolerance, remembered-level rules,
  persistence across restart); OSC power addresses. Clippy `-D warnings`, no `unwrap`/`expect`.
- **TypeScript**: existing suite; the Simple view on `power`; `?simple` / `?embed`; reconnect on
  page show; shared vectors.
- **Swift (Linux + macOS)**: CuemixKit, the shared protocol fixtures, meter decoding against the
  real patch table from your card, shared vectors.
- **iOS simulator**: unit, snapshot (each screen, each widget size, large Dynamic Type) and UI tests.
- **End to end in CI** (macOS job, Rust build cached): cuemix-web in demo mode; the app's UI tests
  turn a knob and check a second WebSocket client sees it, toggle Monitor and watch the fade
  arrive, recall a preset; the WebKit web screenshot for §6b.
- **Audits per phase** (Opus): protocol conformity, security (no secrets or real addresses in Git,
  secrets only in tag workflows, HTTPS-only), accessibility, §8 targets, parity, docs updated.
- **Device checklists by you**: milestone 0 and phases 4, 5, 6, 7, 8, 10.

## 12. Risks

| Risk | Impact | What we do |
|---|---|---|
| Controls/widgets refused local network access for a name resolving to a private address | Quick toggles would not work | Proven first in phase 4; fallbacks: clear "Open Cue Desk once" state; if iOS still refuses, ask your DNS/proxy setup for an alternative name, decided with you |
| Apple Developer enrollment delayed | Milestone 1 waits at phase 4 | Phases 1–3 and milestone 0 do not need it |
| Server-side fade timing in the engine loop | Jerky fades | Rust tests on timing; device check in phase 1 |
| Xcode 27 runner image or simulator model changes | CI breaks | Label and model pinned, checked in phase 4 |
| Widget staleness disappoints | Wrong level on the Lock Screen | Honest freshness (§4.2); optional push in phase 9 |
| Web view feels foreign | Advanced view less pleasant | `?embed` trimming; move screens to native one at a time later |
| iOS 27 system look vs web look | The two drift visually | Q13 rule: system frame, shared content tokens |

## 13. Questions for you

All answered (see the decisions table).

Still open, not blocking: your Mac's macOS and Xcode versions (for the one-time setup only).
