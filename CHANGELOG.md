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

### Added
- Output EQ writes: the five output EQ props (1022-1026: type, bypass,
  frequency, gain, Q) are now sent to the MOTU, coalesced per parameter by the
  existing throttle. A filter type is written only on a band where CueMix 5
  offers it (band 1: bell, low shelf, high-pass; band 2: bell; band 3: bell,
  high shelf). Input EQ, compressor, gate, reverb and clock stay
  read-only until each is checked on a card.

### Fixed
- Output EQ slot labelling. The device has 12 EQ slots, not 12 mono outputs:
  Main L-R, an unused slot (never shown or written), Line 3 … Line 10, Phones
  L-R and the Reverb. The Output tab now names them that way and a preset or
  edit applies to the selected slot only (it used to treat slots as L/R pairs,
  so "Phones R" edited the reverb EQ).

### Changed
- Output EQ defaults match CueMix 5 (low shelf 60 Hz, bell 800 Hz, high shelf
  14 kHz, 0 dB, Q 1); the *Flat* preset uses the same values. OSC
  `/output/{n}/eq/…` takes the 1-based EQ slot (1 = Main, 3-10 = Line 3-10,
  11 = Phones, 12 = Reverb; 2 is unused).
- Compatibility: preset recall now also applies the output EQ to the card.
  Recalling a preset saved before this version (it holds placeholder EQ
  values) replaces the EQ stored on the card. Output EQ values the card never
  gets (the unused slot, a filter type the band does not offer) are rejected
  from OSC and repaired to the defaults when a preset is imported or recalled.
- Known limitation: on a stereo line pair CueMix only uses the left slot (the
  odd-numbered line), so edit Line 3/5/7/9 for stereo pairs. The UI does not
  hide the right slot yet.

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
