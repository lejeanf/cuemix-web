# Architecture — `cuemix-web`

> Document **vivant** et source de vérité technique. Toute modification structurelle
> doit mettre à jour ce fichier **et** `README.md` dans la même PR (gouvernance
> HANDOFF §8.1). Spécification d'origine : [`HANDOFF.md`](HANDOFF.md) (v12.0.0).
> Protocole matériel : [`MOTU-PROTOCOL.md`](MOTU-PROTOCOL.md).

| | |
|---|---|
| Version du contrat | `0.3.0` + non publié : DSP / réverbe / horloge à la forme du matériel, lus sur la MOTU (présets schema v2, D19–D23) |
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
          DeviceLink.commands │ ▲ DeviceLink.events (Identified/Synced/Changed/Meters/Status)
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
| `src/state.rs` | `MotuState`, `Topology`, `DeviceInfo`, `Param`, `Command`, `ServerEvent`, `diff`, `all_params` | ✅ |
| `src/motu/identify.rs` | Identification de l'appareil (modèle, firmware, dimensions, paires stéréo, noms) et décodage de la matrice de mix depuis le dump de connexion | ✅ |
| `src/engine.rs` | Acteur : `EngineHandle`, `Envelope{Origin,Target}`, `DeviceLink`, `Shutdown` | ✅ |
| `src/presets.rs` | Présets (`<presets_dir>/<nom>.json`), `labels.json`, écritures atomiques | ✅ |
| `src/motu/fader.rs` | Lois de fader `0..1` (mix et trims) — utilisées **uniquement aux bords** | ✅ |
| `src/motu/protocol.rs` | Trames BE, `8.24`, `Int16`, `Value`/`ValueKind` | ✅ |
| `src/motu/props.rs` | Table `prop_id` (69 entrées, `verified: false`), `encode`/`decode` ; DSP/réverbe/horloge décodés ; **EQ d'entrée et de sortie actifs** (écrits) ; encodeurs `encode_unverified` (dynamique, réverbe, horloge) **non actifs** | 🟡 partiel (§8) |
| `src/motu/throttle.rs` | Limiteur par paramètre, dernière valeur garantie | ✅ |
| `src/motu/client.rs` | Client WS, `PropCodec`, sync Option A, backoff 1→10 s, mode démo | ✅ |
| `src/motu/discovery.rs` | IP explicite → mDNS (provisoire) → ARP + sonde TCP | 🟡 (§8) |
| `src/osc/address.rs` | Table OSC ⇄ `Command`, conversions 0..1 ⇄ unités natives | ✅ |
| `src/osc/bridge.rs` | UDP `rosc`, pairs (TTL 10 min, max 64), bundles ≤ 8 KiB | ✅ |
| `src/web/server.rs` | Axum : `/ws`, `/api/health`, SPA embarquée, `X-Forwarded-For` | ✅ |
| `src/web/update.rs` | `GET /api/update` (lit le statut écrit par le timer root), `POST /api/update/install` (crée seulement le fichier de demande ; en-tête `x-cuemix-update` exigé) | ✅ |
| `src/measure/` | Test de pièce (D28) : `mod.rs` (tâche pilotée par l'engine : instantané → silence des autres entrées → MPD → attente → restauration exacte, chien de garde), `mpd.rs` (client MPD TCP texte, délais 2 s), `noise.rs` (`GET /api/measure/noise.wav`, bruit rose 90 s en flux) | ✅ |
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
  (`Fixed` 4 o, `U32` 4 o, `U8` 1 o, `I16` 2 o signé, `Str32` 32 o). Toute trame malformée → erreur, jamais de
  panique (balayage exhaustif 0..64 octets testé).
* ⚠ Les **fréquences d'EQ ne sont pas en `8.24`** mais en `Int32` Hz (Q1 résolu, lues via
  `U32` interprété signé) ; les temps DSP sont en `Int16` ms (`ValueKind::I16`) ; les tableaux
  ne sont pas représentables par `ValueKind` (voir §8).

### 3.3 État central (`state::MotuState`)

```text
MotuState
├── topology: { mixes: 7, mixer_inputs: 32, preamps: 8, mic_inputs: 2, routing: 18 × 22 }  (démo ; lue sur la MOTU à la connexion)
├── mixes[7]: { name, channels[32]: { gain, pan, mute, solo } }
├── main: { db }
├── preamps[8]: { gain_db, phantom, pad, phase }   (+48V / Pad : seulement les 2 entrées micro)
├── input_eq[8][4]: { mode, bypass, freq_hz, gain_db, bw_oct }   (préamplis × INPUT_EQ_BANDS ; lu et écrit)
├── compressors[8]: { bypass, threshold_db, attack_ms, release_ms, ratio, makeup }   (lu, non écrit)
├── gates[2]: { bypass, threshold_db, attack_ms, release_ms }     (entrées micro ; lu, non écrit)
├── output_eq[12][3]: { mode, bypass, freq_hz, gain_db, bw_oct }  (OUTPUT_EQ_OUTPUTS slots × OUTPUT_EQ_BANDS ; lu et écrit, slot 1 jamais écrit)
├── reverb: { decay, damping, predelay_ms, width, preset }        (lu, non écrit ; départs/retour = mixes)
├── routing: { matrix[input][output]: bool }                          (non mappé)
├── control_room: { dim, mono, speaker_b, talkback, phones_db }
└── clock: { sample_rate, source }                                (lu, non écrit)
```

* Dimensions DSP : `input_eq` et `compressors` = `topology.preamps`, `gates` =
  `topology.mic_inputs`, `output_eq` = constantes `OUTPUT_EQ_OUTPUTS` × `OUTPUT_EQ_BANDS`
  (mk5 : 12 × 3). `MotuState::reset_dsp` reconstruit ces sections aux défauts du dump.
* Réverbe : ses **départs** sont les faders du mix « Reverb » (mix 6) et son **retour** est
  l'entrée de mixage « Reverb » (30-31) de chaque mix : ils vivent dans `mixes` (`mix_vol`).
* `mode` d'EQ = octet brut (0 cloche, 2 shelf ?, 3 passe-haut) ; les valeurs inconnues sont
  conservées telles quelles.

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
  mainteneur, D13) ; MOTU identifiée (état relu à la connexion) → seul le **diff** est envoyé
  (le mix complet ferait ~900 écritures, ~22 s au rythme du pacer) ; sinon (démo, pas encore
  synchronisé) tous les params mappés du préset, puis `Snapshot`. `● Modifié` ignore trims et +48V.
* `Identified(DeviceInfo)` (juste avant `Synced`) : modèle, firmware, dimensions comptées dans
  le dump, paires stéréo et noms d'affichage. Topologie différente → l'état est reconstruit aux
  dimensions de l'appareil. `DeviceInfo` va dans `SessionInfo.device` (pas dans les présets) ;
  remis à `null` en mode démo.
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
{"cmd":"import_preset","name":"from file","text":"{\"schema_version\":2,\"state\":{…}}"}
{"cmd":"list_presets"}
{"cmd":"set_label","ch":2,"label":{"name":"Vox","color":"#06B6D4"}}
{"cmd":"measure_start"}
{"cmd":"measure_stop"}
```

Serveur → client (`ServerEvent`, tag `event`) — à la connexion : `snapshot`, `labels`, `presets`, `measure`.

```json
{"event":"snapshot","state":{…},"session":{"active_preset":null,"modified":false,"connection":"demo"}}
{"event":"changed","params":[{"param":"dim","on":true}]}
{"event":"status","connection":"online"}
{"event":"session","active_preset":"live","modified":true,"connection":"online","device":{"model":"UL5E","firmware":"2.0.8+2568","topology":{…},"input_names":["Mic 1",…],"mix_names":["Main 1-2",…],"stereo_inputs":[…],"stereo_buses":[…]}}
{"event":"presets","names":["default","live"]}
{"event":"labels","channels":{"2":{"name":"Vox","color":"#06B6D4"}}}
{"event":"meters","levels":[0.0,0.12,…]}
{"event":"measure","state":"playing","source":[6,7],"until":"2026-11-16T20:15:30Z"}
{"event":"error","message":"…"}
```

`measure` : `state` ∈ `unavailable` (pas de `[measure] mpd`, aucune MOTU identifiée ou liaison pas `online`), `idle`,
`starting`, `quiet` (la musique de l'utilisateur est mise en pause et la pièce reste silencieuse 3,5 s : le téléphone enregistre le bruit de fond ; `until` = fin de cette phase), `playing` (MPD a confirmé la lecture du bruit), `restoring`, `error` (avec `message` ;
la restauration est déjà faite, `idle` suit). `source` = entrées de mixage portant le signal MPD ;
`until` (RFC 3339 UTC) = fin de la phase `quiet`, ou heure d'arrêt du chien de garde en `playing`. Séquence : `starting` → `quiet` → `playing` → `restoring` → `idle`. `measure_start`
hors `idle` est refusé par un `error` au seul client demandeur. Le test s'arrête (comme `measure_stop`) quand le client WebSocket qui l'a démarré se déconnecte ; la déconnexion d'un autre client ne change rien (le chien de garde reste le filet de sécurité). L'`engine` mémorise le dernier état
(rejoué à chaque connexion) et transmet `measure_start/stop` à la tâche `measure` ; celle-ci écrit
ses changements de mixage par `Command::Set` d'origine `System` : l'état, la carte et tous les
clients restent cohérents.

`param` : `mix_vol` (`gain`), `mix_pan` (`value`), `mix_mute`, `mix_solo`, `main_vol` (`db`),
`preamp_gain` (`db`), `phantom`, `pad`, `phase`, `route`, `dim`, `mono`, `speaker_b`,
`talkback`, `phones_vol` (`db`), `output_trim` (`ch`, `db`), et **un param par propriété
matérielle** pour le DSP (lus sur la MOTU ; seuls les EQ d'entrée et de sortie sont écrits) : `input_eq_mode` (`ch`, `band`,
`mode` octet brut), `input_eq_bypass` (`on`), `input_eq_freq` (`hz` entiers), `input_eq_gain`
(`db`), `input_eq_bw` (`octaves` = Q), idem `output_eq_*` (`ch` = **slot** d'EQ 0–11 : 0 Main L-R, 1 inutilisé, 2–9 Line 3–10, 10 Phones L-R, 11 Reverb ; `band` 0–2),
`gate_bypass` / `comp_bypass` (`on` = bypass), `gate_threshold` / `comp_threshold` (`db`),
`gate_attack` / `gate_release` / `comp_attack` / `comp_release` (`ms` entiers), `comp_ratio`
(`ratio`), `comp_makeup` (`gain` linéaire), `reverb_decay` / `reverb_damping` / `reverb_width`
(`value` 0–100), `reverb_predelay` (`ms` 0–100), `reverb_preset` (`preset`), `sample_rate`
(`rate` : `"48000"`…), `clock_source` (`source` : `internal`/`spdif`/`optical`). Chaque message
est idempotent ; plages dans `state::dsp_limits` (provisoires, larges ; miroir TS :
`frontend/src/lib/protocol.ts`,
loi de fader : `frontend/src/lib/fader.ts`, portage exact de `motu::fader`). Le serveur ne renvoie pas un `changed` au client
qui l'a provoqué ; `meters` est limité à 60/s par client. Messages entrants ≤ 256 KiB (un fichier de réglages importé fait ≈ 20 Ko). `import_preset` enregistre le JSON reçu (même format qu'un fichier de preset, toute version lisible) sous `<name>.json` puis le rappelle comme `load_preset`.

### 3.6 Table OSC (UDP `:8000`, indices **1-based**)

| Adresse | Arg. | `Param` (conversion) |
|---|---|---|
| `/mix/{bus}/ch/{ch}/vol` | float `0..1` | `mix_vol` (`gain = (p/0.75)^K`, 0.75 = 0 dB) |
| `/mix/{bus}/ch/{ch}/pan` | float `-1..1` | `mix_pan` |
| `/mix/{bus}/ch/{ch}/mute` · `/solo` | int/float/bool | `mix_mute` · `mix_solo` |
| `/mix/main/vol` | float `0..1` | `main_vol` (trim : 1.0 = 0 dB, 0 = −100 dB) |
| `/preamp/{ch}/gain` | float dB `0..74` | `preamp_gain` |
| `/preamp/{ch}/48v` · `/pad` · `/phase` | int | `phantom` · `pad` · `phase` |
| `/eq/{ch}/band/{b}/mode` · `/bypass` · `/freq` · `/gain` · `/bw` | int / float | `input_eq_*` (`ch` 1–8, `b` 1–4 ; Hz, dB, Q) |
| `/output/{n}/eq/band/{b}/mode` · `/bypass` · `/freq` · `/gain` · `/bw` | int / float | `output_eq_*` (`n` 1–12, `b` 1–3) |
| `/comp/{ch}/bypass` · `/threshold` · `/attack` · `/release` · `/ratio` · `/makeup` | int / float | `comp_*` (dB, ms, `n:1`, make-up linéaire) |
| `/gate/{ch}/bypass` · `/threshold` · `/attack` · `/release` | int / float | `gate_*` (`ch` 1–2) |
| `/reverb/decay` · `/damping` · `/predelay` · `/width` · `/preset` | float / int | `reverb_*` (unités natives 0–100, ms, n° brut) |
| `/clock/rate` · `/clock/source` | int Hz · string | `sample_rate` · `clock_source` (`internal`/`spdif`/`optical`) |
| `/routing/in/{in}/out/{out}` | int | `route` |
| `/controlroom/dim` · `/mono` · `/ab` · `/talkback` | int | `dim` · `mono` · `speaker_b` · `talkback` |
| `/controlroom/phones/vol` | float `0..1` | `phones_vol` (trim) — extension à HANDOFF §3.1 |
| `/output/{n}/trim` | float `0..1` | `output_trim` (sortie analogique `n`, 1–10) — onglet Output |
| `/preset/load` | string | `Command::LoadPreset` |

Pont OSC : un pair est enregistré (TTL 10 min, 64 max, **8 nouveaux / 10 s**) dès qu'un message
est accepté, et reçoit un **dump complet** — dumps limités à **1 / s au total** (anti-amplification
UDP) ; `/sync` (sans argument) en redemande un ; bundles imbriqués > 8 niveaux rejetés avant
décodage ; le feedback `changed` va aux cibles
configurées + pairs, **sauf l'émetteur** ; bundles découpés ≤ 8 KiB. Non transmis en OSC :
`presets`, `labels`, `meters`, `meter_raw`, `status`, `session`, `error`. `meter_raw` (`{"event":"meter_raw","raw":[255,13,…]}`) porte la trame de meters brute du MOTU (propriété 6000, non décodée), ≈ 15/s, pour la vue de diagnostic *Meter probe* (`/?probe`) qui sert à établir la table des meters.

`patch` (`{"event":"patch","sources":[46,47,…]}`) porte la table de patch 5010 lue sur la carte (index = destination, valeur = point source ; liste vide = inconnue). Le client MOTU l'accumule (`apply_patch_frame`), le moteur la garde (`Engine.patch`, hors `MotuState` et hors présets), la diffuse à chaque changement, la rejoue après les événements initiaux quand elle est connue et la vide en mode démo. Lecture seule : aucun `Command`, aucun encodeur, pas de `check_writable`. Côté web : `store.patch`, tableau « Patch (5010) » du *Meter probe*, et `outputSourcePoint` (`meterMap.ts`) qui fait suivre aux meters de sortie la source réellement patchée dès que `MK5_PATCH_DEST` (indice de destination par sortie, vide tant qu'il n'est pas lu sur la carte) est renseigné.

### 3.7 Configuration (`config.rs`)

Fichier : `--config` → `$CUEMIX_CONFIG` → `./config.toml` → `/etc/cuemix-web/config.toml` ;
absent = défauts, invalide = erreur, clés inconnues signalées par un avertissement (D15). Surcharges `CUEMIX_HTTP_BIND`,
`CUEMIX_OSC_LISTEN`, `CUEMIX_MOTU_IP`. `throttle_hz = 40` → 25 ms.

## 4. Comportements moteur

* **Découverte** : `[motu].ip` explicite (seul) ; sinon liste ordonnée de candidats — annonce
  UDP JSON sur `:1280` (provisoire, 1,5 s, groupe multicast non documenté) → mDNS (nom
  contenant motu/ultralite **obligatoire**) → voisins ARP `169.254.x.x` sondés en TCP `:1280` ;
  **nos propres adresses sont exclues** ; le client essaie chaque candidat dans l'ordre.
* **Option A — matériel prioritaire** : la MOTU pousse **spontanément tout son état** à la
  connexion puis chaque changement. Le client collecte chaque trame jusqu'à **500 ms sans trame
  non-mètre** (les mètres, prop `6000`, défilent en continu), plafond **5 s**, identifie
  l'appareil (`Identified`), décode le dump avec ce layout, puis `Synced` → `Snapshot` à tous. On ne réécrit jamais l'état au matériel à la reconnexion.
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
* `ci.yml` : frontend → rust (fmt, clippy `-D warnings`, tests) · msrv · shell (shellcheck) · gitleaks + RFC1918.
* `security.yml` : `cargo audit` · `npm audit` · dependency-review (PR) · CodeQL ; actions tierces épinglées par SHA.
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
| D14 | `rust-embed` verrouillé en 8.7.2 dans `Cargo.lock` | ≥ 8.8 tire `cpufeatures 0.3` (édition 2024) et cassait l'ancienne MSRV 1.80 ; à réévaluer depuis D18 (MSRV 1.85) |
| D15 | Compatibilité des données : présets versionnés (`schema_version`, v2 depuis D20), lecture tolérante (`serde(default)`, champs inconnus ignorés), format plus récent refusé clairement ; clés inconnues de `config.toml` = avertissement | Mises à jour et retours arrière sans perte de présets ni démarrage bloqué ; fixtures figées `tests/fixtures/preset-v1.json` et `preset-v2.json` |
| D16 | ~~Mises à jour **manuelles**~~ (remplacée par D24) via `cuemix-update` (sha256, remplacement atomique, contrôle `/api/health`, retour arrière automatique) ; ni minuterie, ni bouton web, ni requête sortante vers GitHub (arbitrage mainteneur 2026-11-14) | Pas de redémarrage surprise en séance ; le serveur web non-root et sans authentification ne doit pas pouvoir déclencher une action root |
| D17 | Version de crate = tag de release (vérifié par `release.yml`) ; tags avec `-` = pré-release (pas de `latest` Docker) | Le contrôle de santé de `cuemix-update` compare la version annoncée par `/api/health` |
| D18 | MSRV relevée de 1.80 à **1.85** (arbitrage mainteneur 2026-11-14) ; job CI MSRV via `dtolnay/rust-toolchain@master` + `toolchain: "1.85"`, ignoré par Dependabot | Les mises à jour Dependabot (tokio 1.54, toml 1.x) exigent 1.85 ; binaires statiques : les utilisateurs ne compilent pas ; reste conforme à « MSRV 1.80+ » (HANDOFF) |
| D19 | DSP / réverbe / horloge à la forme du matériel : un `Param` **par propriété** (`input_eq_freq`, `comp_makeup`…) au lieu de blocs entiers (refonte approuvée par le mainteneur le 2026-11-15 ; la forme « un param par propriété » est un choix d'implémentation) | Une trame lue = un param ; pas d'état fantôme côté client pour recomposer un bloc ; codec 1:1 |
| D20 | Présets **schema v2** : migration v0/v1 → v2 dans `presets::parse_preset` (suppression de `channels` et de l'ancien `reverb`, puis `reset_dsp` aux dimensions du préset) ; fixtures figées `preset-v1.json` et `preset-v2.json` | L'ancien DSP (32 tranches, HPF séparé, Q) ne correspond à aucun réglage matériel : rien à convertir honnêtement |
| D21 | Départs/retour de réverbe = paramètres de mix (mix « Reverb », entrée « Reverb ») ; plus de `reverb_send` ni de `return_gain` | C'est ainsi que la MOTU les stocke (dump) |
| D22 | Mode d'EQ = octet brut (`u8`) + noms côté UI ; pas de bypass global d'EQ (`eq_bypass`, `/eq/{ch}/bypass` retirés) | Valeurs inconnues conservées ; le matériel n'a qu'un bypass par bande |
| D23 | Encodeurs DSP écrits et testés ; **les EQ d'entrée et de sortie sont actifs** (`props::encode` → `encode_eq_param` ; le slot 1 inutilisé de la sortie n'est jamais écrit ; une règle unique `state::eq_mode_allowed` limite les types de filtre par bande). Compresseur, gate, réverbe et horloge restent dans `props::encode_unverified`, **non branchés** : `is_mapped` faux, rien n'est envoyé | Activation une fonction à la fois après un essai sur la carte |
| D24 | Mises à jour **déclenchées par l'utilisateur depuis l'UI**, par canal de release (`[update] channel` = `stable` ou `prerelease`, dans `config.toml`, pas dans l'UI) ; jamais automatiques (arbitrage mainteneur 2026-11-15, remplace D16). Un timer root horaire (quotidien avant 0.5.1 ; `cuemix-update --check --status-file`) écrit le statut ; le bouton *Install* ne fait que créer un fichier de demande vide que l'unité `.path` root transforme en `cuemix-update` | Le service web reste non-root et sans accès réseau sortant ; seul le dernier release officiel du canal (sha256, santé, retour arrière) peut être installé, et seulement à la demande d'une personne |
| D25 | Meters : trame 6000 brute relayée (`meter_raw`, ≈ 30/s) et décodée **côté client** avec la table du bus de patch du mk5 (`frontend/src/lib/meterMap.ts`), confirmée au *Meter probe* et par la table de CueMix 5 | Le serveur reste agnostique ; d'autres modèles = une autre table |
| D26 | Onglet *Routing* supprimé ; `route` et la matrice restent dans l'état (inertes) pour la compatibilité des présets | La matrice booléenne ne correspond pas au matériel (5010 = une source par sortie) ; les mixes couvrent l'usage |
| D27 | Valeur 1006/1026 = **Q** (0,45-10) et non une largeur en octaves ; le champ garde le nom `bw_oct` pour les présets | Table et UI de CueMix 5 |
| D28 | Bruit de test de pièce fourni par le **MPD de l'utilisateur** (`[measure] mpd`, sortie analogique reliée à Line 7-8) et servi par `GET /api/measure/noise.wav` ; la carte n'a **aucun générateur** (table des propriétés de CueMix 5 vérifiée). Pendant le test : instantané puis silence des autres entrées du Main (bus 0), solos coupés, EQ de sortie Main (slot 0) et EQ d'entrée des sources en bypass, via les `Set` normaux ; restauration exacte des seules valeurs changées (une entrée déjà coupée reste coupée), puis reprise de MPD (morceau, position, pause ; MPD est mis en pause par le serveur pendant la phase `quiet`, un arrêt à ce stade fait simplement `pause 0`) ; chien de garde `max_seconds` (20-80 s) ; écritures « silence d'abord » (mutes/solos qui coupent, puis le reste, puis les démutes). L'analyse (micro du téléphone) se fait dans le navigateur | Pas de nouvelle fonction matériel ni de dépendance ; le bruit passe par la chaîne réelle (entrée → mixage → sortie) ; toute erreur réseau MPD aboutit à une restauration propre puis `error` → `idle`. La liste de restauration est écrite dans `measure-restore.json` (à côté de `labels.json`) avant tout changement et gardée jusqu'à ce que la liaison soit restée `online` 3 s après l'envoi : perte de liaison, débranchement, arrêt ou crash en cours de test → restauration rejouée dès que la MOTU est de nouveau `online` (y compris au redémarrage). Limite : après un crash, MPD n'est pas restauré |

### Application écran d'accueil (iOS)

- `frontend/public/` (copié tel quel dans `dist/`, donc embarqué par `rust-embed`) : `manifest.json` (`display: standalone`, `start_url: "./?simple"`, `scope: "./"`, couleurs `#121214`), icônes 192/512/maskable, `apple-touch-icon.png`, `icon.svg` (favicon). Tout est en URL **relative** (compatible sous-chemin de reverse proxy). Extension `.json` pour un `Content-Type` sûr côté `rust-embed`. Les icônes sont produites par `frontend/scripts/gen-icons.mjs` (Node seul, sans dépendance), qui régénère aussi `icon.svg`.
- Zones sûres : `viewport-fit=cover` + barre d'état `black-translucent` ; `env(safe-area-inset-*)` (repli `0px`, donc bureau inchangé) sur TopBar (haut, côtés), vue Simple et ControlRoom (bas), bandeau hors-ligne, toasts, tiroir DSP, RoomTest.
- `?simple` force la vue Simple pour ce chargement sans l'écrire dans `localStorage` ; priorité : `?simple` > `localStorage` > largeur d'écran. `?embed` seul (réservé à la future vue web de l'app native, où Simple est natif) ouvre la vue Avancée pour ce chargement (non sauvegardée) et masque le bouton Simple/Avancé et le point d'état ; `?embed&simple` ouvre Simple. Logique pure dans `frontend/src/lib/launch.ts`.
- Au retour de veille (`visibilitychange` visible, `pageshow` persisté), `store.reconnectNow()` rouvre la WebSocket immédiatement si elle n'est ni ouverte ni en cours de connexion (une seule tentative à la fois, backoff remis à zéro pour afficher « connexion » et non « hors ligne »). Si la socket paraît ouverte mais que la session est `online`/`demo` (mètres ≈ 30/s) et qu'aucun message n'arrive dans les 2 s suivant le retour au premier plan, elle est détachée sans attendre la fermeture et une nouvelle connexion est ouverte aussitôt. `manifest.json` porte `id: "./"` ; `theme_color` = couleur de la TopBar (`#16181d`), `background_color` = `#121214`.

## 8. Questions matériel — arbitrages et état (2026-11-14)

| # | Sujet | Arbitrage | État |
|---|---|---|---|
| Q1 | Encodage des fréquences EQ | Vérifier dans les sources | ✅ `Int32` Hz (props 1004/1024) ; gain/largeur EQ en `8.24` ; HPF = mode de bande 3 ; **lu** depuis le dump (index `ch × 4 + bande` en entrée, `slot × 3 + bande` en sortie) ; **écrit** (EQ d'entrée et de sortie actifs, D23) |
| Q2 | Dimensions | Défauts démo, topologie réelle à la connexion | ✅ comptées dans le dump de connexion (`motu::identify`) ; défauts D11 confirmés sur un mk5 |
| Q3 | Table `prop_id` | Depuis motu-mk5-linux, puis la table de CueMix 5 (`dev.js`) | ✅ table complète connue (id, type, sens ; voir MOTU-PROTOCOL.md). **Lus et écrits** : gain/phase préamplis (8), pad/48V (2), main (vérifié sur carte), casque, trims de sortie (10, ordre des DAC), mono, talkback, matrice de mix 1016–1019 (`bus × 32 + entrée`, `motu::identify::encode_mix`), format optique 5006. **Mesurés** : meters 6000 (D25). **Lus et écrits** : EQ d'entrée 1002–1006 (8 × 4), EQ de sortie 1022–1026 (12 × 3). **Lus seulement** (encodeurs non actifs, D23) : gate 1007–1010, compresseur 1011–1015 + 1021, réverbe 1030–1034, horloge 10/11. **Connus, non implémentés** : routage 5010 (D26), A/B 5015–5017, talkback 5026–5031, loopback 5014 ; DIM absent du matériel |
| Q4 | Volumes en gain linéaire `8.24` | Gain linéaire | 🟡 non confirmé ; seul indice : seuil de gate « 1 = 0 dB » |

À vérifier **sur matériel** par le mainteneur (bloquant pour les faders de mix) :
1. ~~Écriture avec ou sans champ longueur~~ — tranché le 2026-11-15 : champ longueur `u16` requis en écriture (`Frame::encode_write`).
2. Ordre d'index des props 1016–1019 : `bus × 32 + entrée` déduit du dump (lecture) ; à confirmer par une écriture avant d'activer l'écriture des faders.
3. Chemin WebSocket selon firmware (`/` ou `/<serial>`, port 1280 ou 1281).
4. Format des trames de mètres (prop 6000) ; valeur trim 100 = −100 dB ou −inf.
