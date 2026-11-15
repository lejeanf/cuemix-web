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
- The device is identified from its connect dump: model, firmware, channel
  counts, stereo pairs and channel/mix names in CueMix 5 order (Mic 1 … Line 8,
  S/PDIF 1-2, ADAT 1-8, USB returns per mix, Reverb). The UI sizes itself to
  the device, shows one strip per stereo pair (flagging pairs whose halves
  differ on the device), lists the mixes like CueMix (Main, Phones, Line 3-4…)
  and shows the model and firmware in the top bar.
- Mix faders, pan, mute and solo are read from the MOTU (layout
  `bus × 32 + input`, from a real dump). Read-only: changes made in the UI are
  not written to the device yet, and the controls say so.

### Fixed
- Writes to the MOTU had no effect: the device ignores frames without a
  payload length field. Writes are now `[id][index][len u16][payload]`
  (verified on an UltraLite-mk5: main volume follows the web UI). Reads are
  unchanged.
- `proxmox-setup.sh`: on hosts whose `/etc/network/interfaces` does not load
  `interfaces.d`, the MOTU bridge was never created and the container failed
  to start. The script now offers to source its bridge file (with a backup),
  stops otherwise, and checks the bridge exists after `ifreload`. The
  reverse-proxy note names the headers actually used (`X-Forwarded-Host`).

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
