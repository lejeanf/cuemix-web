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

## [0.3.0] - unreleased

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

### Changed
- Minimum supported Rust version raised to **1.85** (dependency upgrades:
  tokio 1.54, toml 1.x, tokio-tungstenite 0.29, rosc 0.11, thiserror 2,
  if-addrs 0.15).

### Compatibility
- Presets: format v1 (first versioned format).
- Hardware: mapped and unverified — preamp gain/phase, pad, +48V, main and
  headphone trims, mono, talkback. Mix faders, DSP, routing and meters are
  state-only until the protocol is confirmed on a device.
