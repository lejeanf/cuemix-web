# Architecture — `cuemix-web`

> Document **vivant** et source de vérité technique. Toute modification structurelle
> doit mettre à jour ce fichier **et** `README.md` dans la même PR (gouvernance
> HANDOFF §8.1). Spécification d'origine : [`HANDOFF.md`](HANDOFF.md) (v12.0.0).

| | |
|---|---|
| Version du contrat | `0.1.0` (Phase 1 — Fondations & Contrats) |
| MSRV | Rust `1.80`, édition 2021 |
| Frontend | Svelte 5 + Vite + TypeScript → `frontend/dist/` |

---

## 1. Vue d'ensemble

```text
 Navigateurs (Web UI)          Contrôleurs OSC (TouchOSC, QLab…)
        │  JSON / WebSocket /ws          │  UDP :8000 (1-based)
        ▼                                ▼
 ┌──────────────┐               ┌────────────────┐
 │ web::server  │               │  osc::bridge   │
 │ (axum)       │               │  osc::address  │
 └──────┬───────┘               └───────┬────────┘
        │   Command ──────────────►     │
        │   ◄────────────── ServerEvent │
        ▼                               ▼
 ┌─────────────────────────────────────────────┐
 │ state::MotuState  (cœur, source de vérité)  │
 │  apply(Param) → Option<Param effectif>      │
 └──────────────────────┬──────────────────────┘
                        │ Param → Frame (throttle 40 Hz / param)
                        ▼
 ┌─────────────────────────────────────────────┐
 │ motu::client  (tokio-tungstenite)           │
 │ motu::protocol (BE, 8.24) · motu::fader     │
 │ motu::discovery (if-addrs, mDNS, ARP+probe) │
 └──────────────────────┬──────────────────────┘
                        │ ws://<169.254.x.x>:1280 (binaire)
                        ▼
              MOTU UltraLite-mk5 (USB CDC-Ethernet)
```

Principe : **un seul type de changement, [`Param`](../src/state.rs)**, circule dans les deux
sens. Un client demande un `Param` ; le cœur l'applique, le borne (clamp) et diffuse le
`Param` **effectif** à *tous* les clients Web et OSC (feedback bidirectionnel). JSON et OSC
ne sont que deux encodages de `Param`.

## 2. Carte des modules

| Fichier | Rôle | Statut | Agent |
|---|---|---|---|
| `src/lib.rs` | Racine de la bibliothèque, lints `deny(unwrap_used, expect_used)` hors tests | ✅ P1 | supervisor |
| `src/main.rs` | CLI (`--config`), logs `tracing`, orchestration Tokio, arrêt propre | ✅ P1 (squelette) | bridges |
| `src/config.rs` | `config.toml` + surcharges `CUEMIX_*` | ✅ P1 | bridges |
| `src/state.rs` | `MotuState`, `Topology`, `Param`, `Command`, `ServerEvent`, `Preset`, `Labels` | ✅ P1 (contrat) | motu-protocol |
| `src/motu/fader.rs` | Loi logarithmique unifiée `0..1` ⇄ gain/dB | ✅ P1 | motu-protocol |
| `src/motu/protocol.rs` | Trames Big-Endian, virgule fixe `8.24`, `Value`/`ValueKind` | ✅ P1 (codec) — table `prop_id` en P2 | motu-protocol |
| `src/motu/discovery.rs` | Interfaces lien-local (`if-addrs`) ✅ ; mDNS + ARP/sonde en P2 | 🟡 | motu-protocol |
| `src/motu/client.rs` | `ConnectionStatus` ✅ ; client WS, reconnexion, throttling, mode démo en P2 | 🟡 | motu-protocol |
| `src/osc/address.rs` | Table OSC ⇄ `Command`/`Param` (1-based) | ✅ P1 | bridges |
| `src/osc/bridge.rs` | Serveur/client UDP `rosc` | ⏳ P2 | bridges |
| `src/web/server.rs` | Axum, `/ws` JSON, assets `rust-embed` | ⏳ P2 | bridges |
| `tests/support/` + `tests/mock_motu.rs` | Serveur Mock WebSocket MOTU + tests d'intégration | ✅ P1 | qa-security |

> Écart assumé vs le prompt de lancement : le code Rust vit à la **racine** (`Cargo.toml`, `src/`)
> conformément à HANDOFF §7 (qui fait foi), et non dans un dossier `backend/`.

Dépendances ajoutées **au premier usage** (pas de dépendance morte dans le manifeste) :
`axum` (ws), `rust-embed` et `mdns-sd` arrivent en Phase 2 ; `tokio-tungstenite` et
`futures-util` sont en `[dev-dependencies]` (mock) et passeront en `[dependencies]` avec
`motu::client`.

## 3. Contrats

### 3.1 Loi de fader unifiée (`motu::fader`)

Position normalisée `p ∈ [0, 1]`, partagée par le Web et l'OSC :

```text
gain(p) = (p / 0.75)^K          K = (12 / 20) / log10(1 / 0.75) ≈ 4.8024
dB(p)   = 20 · K · log10(p / 0.75)
p(gain) = 0.75 · gain^(1/K)
```

| p | dB | gain linéaire |
|---|---|---|
| `0.0` | `-inf` | `0.0` |
| `0.25` | ≈ `-45.8` | ≈ `0.005` |
| `0.5` | ≈ `-16.9` | ≈ `0.143` |
| `0.75` | `0.0` | `1.0` |
| `1.0` | `+12.0` | `3.98107` |

Loi de puissance pure : continue, strictement monotone, exactement inversible, les trois
points d'ancrage HANDOFF sont exacts. Fonctions **totales** (NaN → silence, hors plage →
borné) ; le rejet des valeurs non finies se fait à la frontière (`MotuState::apply`).

### 3.2 Virgule fixe signée `8.24` (`motu::protocol`)

* `raw = round(v · 2^24)` en `i32` Big-Endian ; `v = raw / 2^24` (exact).
* Plage : `[-128.0, 127.99999994]`. **Hors plage ou non fini → `ProtocolError`**, jamais de
  saturation silencieuse. (HANDOFF écrit `as i32` — troncature/saturation ; on arrondit au plus
  proche, écart ≤ 1 LSB ≈ 6·10⁻⁸.)

### 3.3 Trame binaire MOTU

`[prop_id: u16 BE][index: u16 BE][payload]`, payload selon `ValueKind` :

| `ValueKind` | Octets | Encodage |
|---|---|---|
| `Fixed` | 4 | `8.24` signé BE |
| `U32` | 4 | `u32` BE |
| `U8` | 1 | `u8` |
| `Str32` | 32 | UTF-8, terminé/complété par NUL (≤ 31 octets utiles) |

`Frame::decode(bytes, kind_of)` reçoit la table `prop_id → ValueKind` (Phase 2) ; toute trame
tronquée, de longueur incorrecte ou non UTF-8 renvoie une erreur (testé par balayage
exhaustif des longueurs 0..64) — **une trame malformée ne fait jamais paniquer le serveur**.

### 3.4 État central (`state::MotuState`)

```text
MotuState
├── topology: Topology { mixes: 7, mixer_inputs: 18, preamps: 2, routing_inputs: 18, routing_outputs: 22 }  ⚠ provisoire (Q2)
├── mixes[7]: Mix { name, channels[mixer_inputs]: { vol (0..1), pan (-1..1), mute, solo } }
├── main: { vol }
├── preamps[2]: { gain_db (0..60), phantom, pad, phase }
├── channels[mixer_inputs]: ChannelDsp { eq: Equalizer { bypass, hpf, bands[4] }, comp, gate }
├── reverb: { enabled, return_vol, time_s, predelay_ms, sends[mixer_inputs] }
├── routing: { matrix[input][output]: bool }
├── control_room: { dim, mono, speaker_b, talkback, phones_vol }
└── clock: { sample_rate: "44100".."192000", source: internal|spdif|optical }
```

* **Indices 0-based** dans tout le serveur et en JSON. Seule la couche OSC convertit en 1-based.
* `apply(&Param) -> Result<Option<Param>, StateError>` : rejette non-fini / index hors bornes
  **sans muter l'état** ; borne les valeurs finies ; renvoie `None` si rien n'a changé
  (pas de feedback inutile — idempotence).
* `Preset { name, state }` ; `Preset::is_modified(&live)` alimente l'indicateur `● Modifié`.
* `Labels { channels: { <index>: { name, color } } }` → `labels.json` (git-ignoré).

### 3.5 Contrat JSON WebSocket (`/ws`)

Client → serveur (`Command`, tag `cmd`) :

```json
{"cmd":"set","param":"mix_vol","bus":0,"ch":3,"value":0.75}
{"cmd":"set","param":"phantom","ch":0,"on":true}
{"cmd":"set","param":"route","input":2,"output":5,"on":true}
{"cmd":"load_preset","name":"live"}
```

Serveur → client (`ServerEvent`, tag `event`) :

```json
{"event":"snapshot","state":{…MotuState…},"session":{"active_preset":"default","modified":false,"connection":"demo"}}
{"event":"changed","params":[{"param":"dim","on":true}]}
{"event":"status","connection":"online"}
{"event":"session","active_preset":"live","modified":true,"connection":"online"}
```

Valeurs de `param` : `mix_vol`, `mix_pan`, `mix_mute`, `mix_solo`, `main_vol`, `preamp_gain` (`db`),
`phantom`, `pad`, `phase`, `eq_bypass`, `route`, `dim`, `mono`, `speaker_b`, `talkback`, `phones_vol`.
`connection` : `online` | `connecting` | `demo`. Un `snapshot` est toujours envoyé en premier.
Les VU-mètres (60 FPS) seront un événement `meters` dédié, défini en Phase 2.

### 3.6 Table OSC (UDP `:8000`, indices **1-based**)

| Adresse | Arg. | `Param` |
|---|---|---|
| `/mix/{bus}/ch/{ch}/vol` | float `0..1` | `mix_vol` |
| `/mix/{bus}/ch/{ch}/pan` | float `-1..1` | `mix_pan` |
| `/mix/{bus}/ch/{ch}/mute` · `/solo` | int/float/bool | `mix_mute` · `mix_solo` |
| `/mix/main/vol` | float | `main_vol` |
| `/preamp/{ch}/gain` | float `0..60` dB | `preamp_gain` |
| `/preamp/{ch}/48v` · `/pad` · `/phase` | int | `phantom` · `pad` · `phase` |
| `/eq/{ch}/bypass` | int | `eq_bypass` |
| `/routing/in/{in}/out/{out}` | int | `route` |
| `/controlroom/dim` · `/mono` · `/ab` · `/talkback` | int | `dim` · `mono` · `speaker_b` · `talkback` |
| `/controlroom/phones/vol` | float | `phones_vol` — **extension** (absente de HANDOFF §3.1, requise par la barre Control Room §3.2) |
| `/preset/load` | string | `Command::LoadPreset` |

Entrées booléennes : `int` (≠0), `float` (≥0.5), `T`/`F`. Feedback : booléens en `int 0/1`,
continus en `float`. Index `0`, non numérique ou > `u16`/`u8` → `OscError::BadIndex`.

### 3.7 Configuration (`config.rs`)

Fichier : `--config` → `$CUEMIX_CONFIG` → `./config.toml` → `/etc/cuemix-web/config.toml` ;
absent = valeurs par défaut (démarrage zéro-config), invalide = erreur. Clés inconnues
**refusées** (`deny_unknown_fields`) pour détecter les fautes de frappe. Surcharges :
`CUEMIX_HTTP_BIND`, `CUEMIX_OSC_LISTEN`, `CUEMIX_MOTU_IP`. `config.example.toml` est
parsé par un test unitaire pour rester valide. `throttle_hz = 40` → intervalle 25 ms.

## 4. Comportements moteur (spécifiés, implémentés en Phase 2)

* **Découverte** : `[motu].ip` explicite → interface `169.254.0.0/16` (`if-addrs`) → mDNS →
  table ARP + sonde TCP `:1280`.
* **Option A — matériel prioritaire** : à chaque (re)connexion, lecture complète de l'état
  MOTU puis `snapshot` diffusé à tous les clients.
* **Mode démo** : MOTU absente → `ConnectionStatus::Demo`, état en mémoire, UI/OSC/présets
  utilisables.
* **Throttling** : ≤ 40 msg/s **par paramètre** vers la MOTU, dernière valeur toujours envoyée.

## 5. Chaîne de build & CI

* Ordre **non négociable** : `frontend` (`npm ci && npm run build` → `frontend/dist/`) **puis**
  `cargo build` (`rust-embed`). Le `Makefile` et tous les jobs CI le respectent.
* `.cargo/config.toml` : `resolver.incompatible-rust-versions = "fallback"` → `Cargo.lock`
  compatible MSRV 1.80 même généré avec une toolchain récente.
* `ci.yml` : frontend (check+build) → rust (fmt, clippy `-D warnings`, tests) · msrv (1.80) ·
  `cargo audit` · secrets (gitleaks 8.21.2 + regex RFC1918).
* `release.yml` (tags `v*.*.*`) : musl `x86_64`/`aarch64`/`armv7` via `cargo-zigbuild`,
  `.deb` (`cargo-deb`), archives + SHA-256, GitHub Release, image `ghcr.io` multi-arch
  (`linux/amd64`, `linux/arm64`) à partir des binaires musl (distroless `nonroot`).

## 6. Sécurité

* Aucun `unwrap`/`expect` en production (lint `deny` dans `lib.rs`, vérifié par clippy en CI).
* Fichiers locaux git-ignorés : `config.toml`, `*.local.toml`, `deploy/*.env`, `.env`,
  `*.pem`, `*.key`, `data/labels.json`, `data/presets/*.json`.
* `.gitleaks.toml` : règles par défaut (`useDefault`) + règle RFC1918 ; seul `docs/HANDOFF.md`
  est en liste blanche. Les fichiers d'exemple n'utilisent que `192.0.2.x`, `127.0.0.1`,
  `169.254.x.x`.
* Exécution non-root (`cuemix` / distroless `nonroot`).

## 7. Journal des décisions

| # | Décision | Raison |
|---|---|---|
| D1 | Loi de fader = loi de puissance unique `(p/0.75)^K` | Exacte aux 3 ancrages, inversible, sans morceaux |
| D2 | `8.24` : arrondi + erreur hors plage (pas de saturation) | Pas de valeur fausse silencieuse envoyée au matériel |
| D3 | Un seul type `Param`, bidirectionnel, *set-style* idempotent | Feedback Web/OSC trivial, pas de dérive entre encodages |
| D4 | 0-based interne/JSON, 1-based OSC | Tableaux JS natifs ; convention humaine TouchOSC/QLab |
| D5 | Clamp des valeurs finies, rejet des non finies/index invalides | Contrôleurs OSC qui « débordent » légèrement tolérés |
| D6 | Code Rust à la racine (HANDOFF §7), pas de `backend/` | HANDOFF fait foi ; Makefile existant cohérent |
| D7 | Dépendances ajoutées au premier usage | Manifeste sans dépendance morte (lint cargo) |

## 8. Questions matériel — arbitrages du mainteneur (2026-11-14)

| # | Question | Arbitrage | Conséquence Phase 2 |
|---|---|---|---|
| Q1 | HANDOFF §2.2 dit que les **fréquences** sont en `8.24`, or `8.24` plafonne à ~128 : 20 kHz est irreprésentable | **Vérifier dans les sources** (`motu-mk5-linux`, `app.asar` CueMix 5) avant d'implémenter l'EQ | Les fréquences EQ/HPF restent **hors protocole binaire** tant que l'encodage réel n'est pas établi ; `to_fixed_8_24(20000.0)` continue de renvoyer une erreur |
| Q2 | Dimensions réelles UltraLite-mk5 | **Garder les valeurs par défaut** pour le mode démo, **lire la topologie réelle à la connexion** (Option A) | `Topology::ULTRALITE_MK5` = valeurs démo ; `motu::client` remplace la topologie par celle du matériel puis diffuse un `snapshot` |
| Q3 | Table `prop_id` → `ValueKind` | **Extraire depuis `motu-mk5-linux`** (open source) | Chaque entrée porte un marqueur « non vérifié sur matériel » jusqu'au test par le mainteneur |
| Q4 | Grandeur envoyée pour les volumes | **Gain linéaire `8.24`** (`fader::pos_to_gain`, 0 dB = `0x01000000`) | Hypothèse maintenue tant que l'extraction Q3 ou un test matériel ne la contredit pas |
