# CueMix 5 capture session (protocol discovery)

Goal: record what CueMix 5 sends to the UltraLite-mk5 when one setting changes, to decode
the properties nobody documents (routing, loopback/phones source, A/B, monitor group,
"To USB host") and to confirm the write layout of the mix matrix (pan direction, stereo
pairs). About 20 minutes. Nothing here changes cuemix-web; it only produces capture files.

The captures contain your device's serial number and settings: keep them private, never
commit them. Share them with the maintainer's tooling only.

## Before you start

1. Move the MOTU's USB cable to the PC that runs CueMix 5, and check that CueMix shows the
   device (not an offline snapshot: the values must move when you turn the MAIN VOL knob).
2. Install Wireshark yourself from its official site (with Npcap).
3. In Wireshark, pick the network adapter of the MOTU. It is the one with a link-local
   `169.254.x.x` address (Windows: "Ethernet N", description mentioning USB / Remote NDIS / MOTU).
4. Capture filter: `tcp port 1280`.
5. Turn your speakers down. Some steps change levels.

## How to capture

One file per step, so each change is isolated:

- Start the capture, wait 3 s, make **exactly one** change in CueMix, wait 3 s, stop.
- Save as `NN-name.pcapng` (names below).
- If a step has several values (e.g. a source menu), go through them **in the listed order**,
  waiting 2 s between each, in the same file.

## Steps

| # | File | In CueMix | Change |
|---|------|-----------|--------|
| 00 | `00-baseline` | – | Nothing: just open CueMix and wait 10 s (full state dump). |
| 01 | `01-mix-fader` | Main 1-2 Mix, strip Line 3 | Fader from -inf to 0 dB, then back to -inf. |
| 02 | `02-mix-pan` | Main 1-2 Mix, Line 3 | Pan fully left, then fully right, then centre. |
| 03 | `03-mix-mute-solo` | Main 1-2 Mix, Line 3 | Mute on, off; Solo on, off. |
| 04 | `04-stereo-fader` | Main 1-2 Mix, Computer USB Main 1-2 | Fader from 0 dB to -12 dB and back. |
| 05 | `05-stereo-link` | Input page (or strip settings) | Link Line 3-4 as a stereo pair, then unlink. |
| 06 | `06-phones-source` | Output page, Phones Source | Each entry of the menu, top to bottom, then back to the original. |
| 07 | `07-loopback-source` | Output page, Loopback Source | Each entry, top to bottom, then back. |
| 08 | `08-ab` | Output page | AB ON off/on; A; B; BOTH. |
| 09 | `09-monitor-group` | Output page, Monitor Group | Toggle 3, then 4, then back. |
| 10 | `10-usb-host` | Input page, Line 3 "To USB host" | Each option, then back to Pre Fx. |
| 11 | `11-reverb` | Reverb settings | Change decay, then damping, predelay, width, preset (one at a time). |
| 12 | `12-input-eq` | Line 3 EQ | Band 2 gain +3 dB, freq 1 kHz, width, then bypass on/off. |
| 13 | `13-dynamics` | Mic 1 | Gate on/off and threshold; Compressor on/off, ratio, makeup. |
| 14 | `14-output-eq` | Output EQ (Main 1-2) | One band's gain and frequency. |
| 15 | `15-sample-rate` | Device page | 48 kHz → 44.1 kHz → 48 kHz (audio stops briefly). |
| 16 | `16-routing` | Device / routing page, if CueMix has one | One routing change, then undo. |

Put the files in a folder outside the repository (or in `target/`, which git ignores) and
tell the maintainer where they are.
