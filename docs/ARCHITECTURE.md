# Architecture — `cuemix-web`

> Document **vivant** et source de vérité technique. Toute modification structurelle
> doit mettre à jour ce fichier **et** `README.md` dans la même PR (gouvernance
> HANDOFF §8.1). Spécification d'origine : [`HANDOFF.md`](HANDOFF.md) (v12.0.0).
> Protocole matériel : [`MOTU-PROTOCOL.md`](MOTU-PROTOCOL.md).

| | |
|---|---|
| Version du contrat | `0.3.0` (Phase 3 — interface ; params DSP/réverbe/horloge éditables) |
| MSRV | Rust `1.85`, édition 2021 (relevée depuis 1.80, décision D18) |
| Frontend | Svelte 5 + Vite + TypeScript → `frontend/dist/` (embarqué via `rust-embed`) |

---

## 1. Vue d'ensemble

```text
 Navigateurs (Web UI)                    Contrôleurs OSC (TouchOSC, QLab…)
        │  JSON / WebSocket /ws  (unités natives)   │  UDP :8000 (1-based, faders 0..1)
        ▼                                           ▼
 ┌──────────────────┐                     ┌────────────────────┐
 │ web::server      │                     │ osc::bridge        │
 │ axum + rust-embed│                     │ osc::address (0..1 ⇄ natif) │
 └────────┬─────────┘                     └─────────┬──────────┘
          │ EngineHandle::command(Origin, Command)  │
          │ ◄──────── broadcast<Envelope{origin,target,event}>
          ▼                                         ▼
 ┌──────────────────────────────────────────────────────────────┐
 │ engine (acteur, seul propriétaire de MotuState)              │
 │  apply(Param) → Param effectif → broadcast + DeviceLink      │
 │  presets (JSON) · labels.json · indicateur « ● Modifié »     │
 └──────────────────────────┬───────────────────────────────────┘
          DeviceLink.commands │ ▲ DeviceLink.events (Synced/Changed/Meters/Status)
                              ▼ │
 ┌──────────────────────────────────────────────────────────────┐
 │ motu::client  — reconnexion, sync Option A, mode démo        │
 │ motu::throttle (40 Hz/param) · motu::props (table prop_id)   │
 │ motu::protocol (BE, 8.24) · motu::discovery (IP, mDNS, ARP)  │
 └──────────────────────────┬───────────────────────────────────┘
                            │ ws://<169.254.x.x>:1280 (binaire)
                            ▼
                MOTU UltraLite-mk5 (USB CDC-Ethernet)
```

Principe : **un seul type de changement, [`Param`](../src/state.rs)**, circule dans les deux
sens. Un client demande un `Param` ; l'`engine` l'applique, le borne et diffuse le `Param`
**effectif** à tous les clients (feedback bidirectionnel) et l'envoie au matériel. Les
changements venant du matériel sont appliqués et diffusés, **jamais renvoyés** au matériel.

## 2. Carte des modules

| Fichier | Rôle | Statut |
|---|---|---|
| `src/lib.rs` | Racine, lints `deny(unwrap_used, expect_used)` hors tests | ✅ |
| `src/main.rs` | CLI, logs, orchestration (`JoinSet`), arrêt propre Ctrl-C/SIGTERM (3 s de grâce) | ✅ |
| `src/config.rs` | `config.toml` + surcharges `CUEMIX_*` | ✅ |
| `src/state.rs` | `MotuState`, `Topology`, `Param`, `Command`, `ServerEvent`, `diff`, `all_params` | ✅ |
| `src/engine.rs` | Acteur : `EngineHandle`, `Envelope{Origin,Target}`, `DeviceLink`, `Shutdown` | ✅ |
| `src/presets.rs` | Présets (`<presets_dir>/<nom>.json`), `labels.json`, écritures atomiques | ✅ |
| `src/motu/fader.rs` | Lois de fader `0..1` (mix et trims) — utilisées **uniquement aux bords** | ✅ |
| `src/motu/protocol.rs` | Trames BE, `8.24`, `Value`/`ValueKind` | ✅ |
| `src/motu/props.rs` | Table `prop_id` (65 entrées, `verified: false`), `encode`/`decode` | 🟡 partiel (§8) |
| `src/motu/throttle.rs` | Limiteur par paramètre, dernière valeur garantie | ✅ |
| `src/motu/client.rs` | Client WS, `PropCodec`, sync Option A, backoff 1→10 s, mode démo | ✅ |
| `src/motu/discovery.rs` | IP explicite → mDNS (provisoire) → ARP + sonde TCP | 🟡 (§8) |
| `src/osc/address.rs` | Table OSC ⇄ `Command`, conversions 0..1 ⇄ unités natives | ✅ |
| `src/osc/bridge.rs` | UDP `rosc`, pairs (TTL 10 min, max 64), bundles ≤ 8 KiB | ✅ |
| `src/web/server.rs` | Axum : `/ws`, `/api/health`, SPA embarquée, `X-Forwarded-For` | ✅ |
| `src/zeroconf.rs` | Annonce mDNS `http://<mdns_hostname>.local:<port>` | ✅ |
| `tests/` | Mock MOTU + intégration `mock_motu`, `motu_client`, `osc_bridge`, `web_server` | ✅ |

Le code Rust vit à la **racine** (`Cargo.toml`, `src/`), conformément à HANDOFF §7.

## 3. Contrats

### 3.1 Unités : natives au cœur, `0..1` aux bords (décision D8)

L'état, les présets et le JSON WebSocket portent les **unités natives de la MOTU** :

| Contrôle | Champ | Unité / plage | Fil MOTU |
|---|---|---|---|
| Fader de mix | `gain` | gain linéaire `0.0..=MIX_GAIN_MAX` (3.98107 = +12 dB), `1.0` = 0 dB | `8.24` (Q4 : linéaire, à confirmer) |
| Main / casque | `db` | dB entiers `-100..=0` | `U8` atténuation (0 = 0 dB, 100 = −100 dB) |
| Gain préampli | `db` | dB entiers `0..=74` | `U8` |
| Pan | `value` | `-1.0..=1.0` | non mappé (Q3) |

La position de fader normalisée `0..1` n'existe **qu'aux bords** : OSC (`osc::address`) et le
widget fader de l'UI (Phase 3, même loi portée en TypeScript).

```text
Fader de mix :  gain(p) = (p / 0.75)^K      K = (12/20) / log10(1/0.75) ≈ 4.8024
                p = 0 → −inf · 0.5 → −16.9 dB · 0.75 → 0 dB · 1.0 → +12 dB
Trims       :  dB(p) = dB_mix(p · 0.75)    même courbe, 1.0 → 0 dB, plancher −100 dB
```

Fonctions totales (NaN → silence, hors plage → borné) ; **l'OSC rejette NaN/inf** avant
conversion (une valeur invalide ne coupe jamais le son).

### 3.2 Virgule fixe signée `8.24` et trames

* `raw = round(v · 2^24)` en `i32` BE ; plage `[-128, 128)` ; hors plage/non fini → `ProtocolError`.
* Trame : `[prop_id: u16 BE][index: u16 BE][payload]`, payload selon `ValueKind`
  (`Fixed` 4 o, `U32` 4 o, `U8` 1 o, `Str32` 32 o). Toute trame malformée → erreur, jamais de
  panique (balayage exhaustif 0..64 octets testé).
* ⚠ Les **fréquences d'EQ ne sont pas en `8.24`** mais en `Int32` Hz (Q1 résolu) ; `Int16` et
  tableaux ne sont pas encore représentables par `ValueKind` (voir §8).

### 3.3 État central (`state::MotuState`)

```text
MotuState
├── topology: { mixes: 7, mixer_inputs: 32, preamps: 8, mic_inputs: 2, routing: 18 × 22 }  (démo ; Q2)
├── mixes[7]: { name, channels[32]: { gain, pan, mute, solo } }
├── main: { db }
├── preamps[8]: { gain_db, phantom, pad, phase }   (+48V / Pad : seulement les 2 entrées micro)
├── channels[32]: { eq: { bypass, hpf, bands[4] }, comp, gate }
├── reverb: { enabled, return_vol, time_s, predelay_ms, sends[32] }   (non mappé)
├── routing: { matrix[input][output]: bool }                          (non mappé)
├── control_room: { dim, mono, speaker_b, talkback, phones_db }
└── clock: { sample_rate, source }
```

* Indices **0-based** partout sauf OSC (1-based).
* `apply(&Param)` : rejette non-fini / index hors bornes **sans muter** ; borne et arrondit
  (dB entiers) ; renvoie `None` si rien ne change.
* `diff(&target)` : `Param`s pour passer d'un état à l'autre (rappel de préset) ;
  `all_params()` : tous les `Param` adressables (dump OSC après snapshot).

### 3.4 Moteur (`engine`)

* Acteur unique ; files : requêtes 1024, matériel 1024 (`try_send`, ne bloque jamais),
  broadcast 4096 (`Lagged` → le client se resynchronise via `initial_events()`).
* `Envelope { origin: Web(id) | Osc(addr) | Hardware | System, target: All | Only(origin), event }`.
  Les réponses (`Presets` sur `ListPresets`) et les `Error` ne vont qu'à l'émetteur.
* Seuls les `Param` **mappés matériellement** (`motu::props::is_mapped`) entrent dans la file
  matériel ; si elle est pleine, le param est **abandonné** (log `warn`).
* Rappel de préset : refusé si la topologie diffère ; fichier **normalisé** (forme vérifiée,
  valeurs bornées via `apply`) ; **trims (main/casque) et +48V jamais rappelés** (arbitrage
  mainteneur, D13) ; tous les params mappés du préset sont envoyés (pas un diff contre un état
  matériel inconnu), puis `Snapshot`. `● Modifié` ignore trims et +48V.
* `Synced(Vec<Param>)` : appliqué **par-dessus** l'état courant ; les champs non rapportés par
  la MOTU (non mappés) sont conservés.
* Liste des présets en cache mémoire (aucune I/O disque par connexion client).
* Labels : nom 1–64 caractères imprimables, couleur `#RRGGBB` ; écriture `fsync` + renommage ;
  retour arrière et erreur si l'écriture échoue.

### 3.5 Contrat JSON WebSocket (`/ws`)

Client → serveur (`Command`, tag `cmd`) :

```json
{"cmd":"set","param":"mix_vol","bus":0,"ch":3,"gain":1.0}
{"cmd":"set","param":"main_vol","db":-12}
{"cmd":"set","param":"preamp_gain","ch":0,"db":30}
{"cmd":"set","param":"phantom","ch":0,"on":true}
{"cmd":"load_preset","name":"live"}
{"cmd":"save_preset","name":"live"}
{"cmd":"list_presets"}
{"cmd":"set_label","ch":2,"label":{"name":"Vox","color":"#06B6D4"}}
```

Serveur → client (`ServerEvent`, tag `event`) — à la connexion : `snapshot`, `labels`, `presets`.

```json
{"event":"snapshot","state":{…},"session":{"active_preset":null,"modified":false,"connection":"demo"}}
{"event":"changed","params":[{"param":"dim","on":true}]}
{"event":"status","connection":"online"}
{"event":"session","active_preset":"live","modified":true,"connection":"online"}
{"event":"presets","names":["default","live"]}
{"event":"labels","channels":{"2":{"name":"Vox","color":"#06B6D4"}}}
{"event":"meters","levels":[0.0,0.12,…]}
{"event":"error","message":"…"}
```

`param` : `mix_vol` (`gain`), `mix_pan` (`value`), `mix_mute`, `mix_solo`, `main_vol` (`db`),
`preamp_gain` (`db`), `phantom`, `pad`, `phase`, `eq_bypass`, `route`, `dim`, `mono`,
`speaker_b`, `talkback`, `phones_vol` (`db`), et (Phase 3, état/présets/UI seulement, sans
adresse OSC ni mappage matériel) : `eq_band` (`ch`, `band` 0–3, `enabled`, `freq_hz`, `gain_db`,
`q`), `hpf` (`enabled`, `freq_hz`), `comp` (`enabled`, `threshold_db`, `ratio`, `attack_ms`,
`release_ms`, `gain_db`), `gate` (`enabled`, `threshold_db`, `attack_ms`, `release_ms`),
`reverb` (`enabled`, `return_gain` linéaire, `time_s`, `predelay_ms`), `reverb_send` (`ch`,
`gain` linéaire), `clock` (`sample_rate`, `source`). Chaque message porte le **bloc entier**
(idempotent) ; plages dans `state::dsp_limits` (miroir TS : `frontend/src/lib/protocol.ts`,
loi de fader : `frontend/src/lib/fader.ts`, portage exact de `motu::fader`). Le serveur ne renvoie pas un `changed` au client
qui l'a provoqué ; `meters` est limité à 60/s par client. Messages entrants ≤ 64 KiB.

### 3.6 Table OSC (UDP `:8000`, indices **1-based**)

| Adresse | Arg. | `Param` (conversion) |
|---|---|---|
| `/mix/{bus}/ch/{ch}/vol` | float `0..1` | `mix_vol` (`gain = (p/0.75)^K`, 0.75 = 0 dB) |
| `/mix/{bus}/ch/{ch}/pan` | float `-1..1` | `mix_pan` |
| `/mix/{bus}/ch/{ch}/mute` · `/solo` | int/float/bool | `mix_mute` · `mix_solo` |
| `/mix/main/vol` | float `0..1` | `main_vol` (trim : 1.0 = 0 dB, 0 = −100 dB) |
| `/preamp/{ch}/gain` | float dB `0..74` | `preamp_gain` |
| `/preamp/{ch}/48v` · `/pad` · `/phase` | int | `phantom` · `pad` · `phase` |
| `/eq/{ch}/bypass` | int | `eq_bypass` |
| `/routing/in/{in}/out/{out}` | int | `route` |
| `/controlroom/dim` · `/mono` · `/ab` · `/talkback` | int | `dim` · `mono` · `speaker_b` · `talkback` |
| `/controlroom/phones/vol` | float `0..1` | `phones_vol` (trim) — extension à HANDOFF §3.1 |
| `/preset/load` | string | `Command::LoadPreset` |

Pont OSC : un pair est enregistré (TTL 10 min, 64 max, **8 nouveaux / 10 s**) dès qu'un message
est accepté, et reçoit un **dump complet** — dumps limités à **1 / s au total** (anti-amplification
UDP) ; `/sync` (sans argument) en redemande un ; bundles imbriqués > 8 niveaux rejetés avant
décodage ; le feedback `changed` va aux cibles
configurées + pairs, **sauf l'émetteur** ; bundles découpés ≤ 8 KiB. Non transmis en OSC :
`presets`, `labels`, `meters`, `status`, `session`, `error`.

### 3.7 Configuration (`config.rs`)

Fichier : `--config` → `$CUEMIX_CONFIG` → `./config.toml` → `/etc/cuemix-web/config.toml` ;
absent = défauts, invalide = erreur, clés inconnues refusées. Surcharges `CUEMIX_HTTP_BIND`,
`CUEMIX_OSC_LISTEN`, `CUEMIX_MOTU_IP`. `throttle_hz = 40` → 25 ms.

## 4. Comportements moteur

* **Découverte** : `[motu].ip` explicite (seul) ; sinon liste ordonnée de candidats — annonce
  UDP JSON sur `:1280` (provisoire, 1,5 s, groupe multicast non documenté) → mDNS (nom
  contenant motu/ultralite **obligatoire**) → voisins ARP `169.254.x.x` sondés en TCP `:1280` ;
  **nos propres adresses sont exclues** ; le client essaie chaque candidat dans l'ordre.
* **Option A — matériel prioritaire** : la MOTU pousse **spontanément tout son état** à la
  connexion puis chaque changement. Le client applique chaque trame jusqu'à **500 ms sans trame
  non-mètre** (les mètres, prop `6000`, défilent en continu), plafond **5 s**, puis
  `Synced` → `Snapshot` à tous. On ne réécrit jamais l'état au matériel à la reconnexion.
* **Mode démo** : MOTU absente → `Demo`, état en mémoire ; les commandes sont drainées.
* **Throttling** : par paramètre, 1re valeur immédiate puis ≤ 1 / 25 ms, valeurs intermédiaires
  fusionnées, dernière garantie ; puis **cadenceur global ≥ 20 ms entre trames** (y compris les
  params multi-trames et les rafales de rappel) — la MOTU ferme la socket si on écrit trop vite.
* **Statut** : `Connecting` seulement au 1er essai et après une perte de connexion ; `Demo`
  pendant les nouvelles tentatives.

## 5. Chaîne de build & CI

* Ordre **non négociable** : `frontend` (`npm ci && npm run build`) **puis** `cargo build`
  (`rust-embed` sans `allow_missing` : pas de binaire sans UI).
* `.cargo/config.toml` : résolution compatible MSRV 1.85 (`cargo update` choisit des versions compatibles).
* `ci.yml` : frontend → rust (fmt, clippy `-D warnings`, tests) · msrv · `cargo audit` · gitleaks + RFC1918.
* `release.yml` : musl x86_64/aarch64/armv7 (`cargo-zigbuild`), `.deb`, GitHub Release, image
  `ghcr.io` amd64/arm64 (distroless `nonroot`).

## 6. Sécurité

* Aucun `unwrap`/`expect` en production ; trames et JSON malformés → erreurs, jamais de panique.
* Noms de présets validés (`[A-Za-z0-9 _.-]{1,64}`, pas de `.` initial) : pas de traversée.
* Serveur web : seuls les fichiers embarqués sont servis (`..`, `\`, NUL → `index.html`).
* `/ws` : en-tête `Origin` vérifié (même hôte que `Host`, ou `web.allowed_origins`), sinon 403
  (anti-détournement cross-site) ; envois bornés à 2 s (un client bloqué est déconnecté) ;
  `X-Forwarded-For` : entrée la plus à droite, seulement si `trust_forwarded_headers` (défaut `false`).
* **Pas d'authentification** : n'exposer que sur un LAN de confiance ou derrière un proxy authentifiant.
* Fichiers locaux git-ignorés : `config.toml`, `*.local.toml`, `deploy/*.env`, `.env`, `*.pem`,
  `*.key`, `data/labels.json`, `data/presets/*.json`, `.claude/`.
* `.gitleaks.toml` : règles par défaut + RFC1918 ; exemples en `192.0.2.x`/`127.0.0.1`/`169.254.x.x`.

## 7. Journal des décisions

| # | Décision | Raison |
|---|---|---|
| D1 | Loi de fader = loi de puissance `(p/0.75)^K` | Exacte aux 3 ancrages HANDOFF, inversible |
| D2 | `8.24` : arrondi + erreur hors plage | Pas de valeur fausse silencieuse vers le matériel |
| D3 | Un seul type `Param`, bidirectionnel, idempotent | Feedback Web/OSC trivial |
| D4 | 0-based interne/JSON, 1-based OSC | Tableaux JS ; convention TouchOSC/QLab |
| D5 | Clamp des valeurs finies, rejet des non finies / index invalides | Tolérance aux contrôleurs |
| D6 | Code Rust à la racine (HANDOFF §7) | HANDOFF fait foi |
| D7 | Dépendances ajoutées au premier usage | Manifeste propre |
| D8 | **Unités natives MOTU** dans l'état/présets/JSON ; `0..1` seulement en OSC et widget UI (arbitrage mainteneur 2026-11-14) | Codec quasi direct, présets = valeurs matérielles ; JSON ne peut pas porter `-inf` dB → gain linéaire pour les mix |
| D9 | `engine` = acteur unique, `Envelope{origin,target}` | Pas de verrou, pas d'écho, réponses ciblées |
| D10 | Sync Option A par fenêtre de silence non-mètre (500 ms, max 5 s) | La MOTU pousse son état sans requête (MOTU-PROTOCOL.md) |
| D11 | Topologie démo 32 entrées mixer / 8 préamplis / 2 micros | Sources motu-mk5-linux (Q2) |
| D12 | Pas d'écho `changed` vers l'émetteur (Web et OSC) | Évite le « tremblement » des faders pendant le geste |
| D13 | Rappel de préset sans trims ni +48V (arbitrage mainteneur, audit n°9) | Sécurité des écoutes et du matériel ; motu-mk5-linux le déconseille |
| D14 | `rust-embed` verrouillé en 8.7.2 dans `Cargo.lock` | ≥ 8.8 tire `cpufeatures 0.3` (édition 2024) et casse la MSRV 1.80 |
| D15 | Compatibilité des données : présets versionnés (`schema_version`, v1), lecture tolérante (`serde(default)`, champs inconnus ignorés), format plus récent refusé clairement ; clés inconnues de `config.toml` = avertissement | Mises à jour et retours arrière sans perte de présets ni démarrage bloqué ; fixture figée `tests/fixtures/preset-v1.json` |
| D16 | Mises à jour **manuelles** via `cuemix-update` (sha256, remplacement atomique, contrôle `/api/health`, retour arrière automatique) ; ni minuterie, ni bouton web, ni requête sortante vers GitHub (arbitrage mainteneur 2026-11-14) | Pas de redémarrage surprise en séance ; le serveur web non-root et sans authentification ne doit pas pouvoir déclencher une action root |
| D17 | Version de crate = tag de release (vérifié par `release.yml`) ; tags avec `-` = pré-release (pas de `latest` Docker) | Le contrôle de santé de `cuemix-update` compare la version annoncée par `/api/health` |
| D18 | MSRV relevée de 1.80 à **1.85** (arbitrage mainteneur 2026-11-14) ; job CI MSRV via `dtolnay/rust-toolchain@master` + `toolchain: "1.85"`, ignoré par Dependabot | Les mises à jour Dependabot (tokio 1.54, toml 1.x) exigent 1.85 ; binaires statiques : les utilisateurs ne compilent pas ; reste conforme à « MSRV 1.80+ » (HANDOFF) |

## 8. Questions matériel — arbitrages et état (2026-11-14)

| # | Sujet | Arbitrage | État |
|---|---|---|---|
| Q1 | Encodage des fréquences EQ | Vérifier dans les sources | ✅ `Int32` Hz (props 1004/1024) ; gain/largeur EQ en `8.24` ; HPF = mode de bande 3. `ValueKind::I32` à ajouter avec l'EQ |
| Q2 | Dimensions | Défauts démo, topologie réelle à la connexion | 🟡 défauts mis à jour (D11) ; lecture de la topologie à la connexion à faire |
| Q3 | Table `prop_id` | Depuis motu-mk5-linux | 🟡 65 propriétés ; mappés : gain/phase préamplis (8), pad/48V (2), main, casque, mono, talkback. **Non mappés** : matrice de mix (ordre d'index 1016–1019 inconnu), routing (5010 = table de 80 o), bypass EQ global (par bande), DIM (absent du matériel), A/B (5015–5017 inconnus) |
| Q4 | Volumes en gain linéaire `8.24` | Gain linéaire | 🟡 non confirmé ; seul indice : seuil de gate « 1 = 0 dB » |

À vérifier **sur matériel** par le mainteneur (bloquant pour les faders de mix) :
1. Écriture avec ou sans champ longueur `u16` (`[id][index][len][payload]` selon deux sources).
2. Ordre d'index des props 1016–1019 (32 entrées × 14 bus).
3. Chemin WebSocket selon firmware (`/` ou `/<serial>`, port 1280 ou 1281).
4. Format des trames de mètres (prop 6000) ; valeur trim 100 = −100 dB ou −inf.
