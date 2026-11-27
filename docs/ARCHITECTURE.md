# Architecture — `cuemix-web`

> Document **vivant** et source de vérité technique. Toute modification structurelle
> doit mettre à jour ce fichier **et** `README.md` dans la même PR (gouvernance
> HANDOFF §8.1). Spécification d'origine : [`HANDOFF.md`](HANDOFF.md) (v12.0.0).
> Protocole matériel : [`MOTU-PROTOCOL.md`](MOTU-PROTOCOL.md).

| | |
|---|---|
| Version du contrat | `0.3.0` + non publié (0.6.0 : `power`, prise enceintes, `/api/v1`, D29 ; 0.7.0 : réglages OSC en direct `set_osc_settings` / `osc_settings`, choix de version à installer, D30) : DSP / réverbe / horloge à la forme du matériel, lus sur la MOTU (présets schema v2, D19–D23) |
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
| `src/engine.rs` | Acteur : `EngineHandle`, `Envelope{Origin,Target}`, `DeviceLink`, `Shutdown`, requêtes `Summary` / `Api` (HTTP court) | ✅ |
| `src/engine/sequence.rs` | Marche/arrêt serveur (D29) : un `Option<Fade>` par canal et branche `sleep_until` de la boucle, niveau mémorisé, séquence de la prise enceintes (Main), événements `power` / `speaker_plug` | ✅ |
| `src/power.rs` | Courbe du fondu (smoothstep, 12 ms/dB, 250–1200 ms, pas de 28 ms), `Fade`, niveau mémorisé (`power.json`) — pur, testé à horloge virtuelle | ✅ |
| `src/plug.rs` | Prise enceintes : réglages, URL (vecteurs `design/vectors/speaker-plug-urls.json`), barrière (adresses locales), client HTTP GET écrit à la main sur `TcpStream` | ✅ |
| `src/presets.rs` | Présets (`<presets_dir>/<nom>.json`), `labels.json`, écritures atomiques | ✅ |
| `src/motu/fader.rs` | Lois de fader `0..1` (mix et trims) — utilisées **uniquement aux bords** | ✅ |
| `src/motu/protocol.rs` | Trames BE, `8.24`, `Int16`, `Value`/`ValueKind` | ✅ |
| `src/motu/props.rs` | Table `prop_id` (69 entrées, `verified: false`), `encode`/`decode` ; DSP/réverbe/horloge décodés ; **EQ d'entrée et de sortie actifs** (écrits) ; encodeurs `encode_unverified` (dynamique, réverbe, horloge) **non actifs** | 🟡 partiel (§8) |
| `src/motu/throttle.rs` | Limiteur par paramètre, dernière valeur garantie | ✅ |
| `src/motu/client.rs` | Client WS, `PropCodec`, sync Option A, backoff 1→10 s, mode démo | ✅ |
| `src/motu/discovery.rs` | IP explicite → mDNS (provisoire) → ARP + sonde TCP | 🟡 (§8) |
| `src/osc/address.rs` | Table OSC ⇄ `Command`, conversions 0..1 ⇄ unités natives | ✅ |
| `src/osc/bridge.rs` | UDP `rosc`, pairs (TTL 10 min, max 64), bundles ≤ 8 KiB ; **changement de réglages en direct** (`OscApply` : nouveau socket lié d'abord, l'ancien gardé en cas d'échec ; dump complet aux cibles ajoutées) | ✅ |
| `src/osc/settings.rs` | Réglages OSC enregistrés (`osc.json`) : `OscSettings { listen, targets }`, validation (syntaxe, ports, 8 cibles au plus, barrière sur chaque adresse résolue), lecture/écriture atomique, `start_bridge` (osc.json > `config.toml`) | ✅ |
| `src/engine/osc.rs` | Réglages OSC côté moteur : état courant, `listening`, `error`, un seul changement à la fois ; événement `osc_settings` | ✅ |
| `src/web/server.rs` | Axum : `/ws`, `/api/health` (`api`), SPA embarquée, `X-Forwarded-For` | ✅ |
| `src/web/api.rs` | HTTP court `GET /api/v1/summary`, `POST /api/v1/command` (contrôles, widgets, Raccourcis ; même règle `Origin` que `/ws`) | ✅ |
| `src/web/admin.rs` | Jeton admin facultatif (`[web] admin_token = "auto"`) : création du fichier (0600, 32 octets aléatoires), lien d'appairage (fragment `#pair=`), `GET /api/admin`, `POST /api/admin/pair` (cookie `HttpOnly`, `SameSite=Strict`), `POST /api/admin/unpair` ; `is_admin` (cookie ou `Authorization: Bearer`, comparaison en temps constant) | ✅ |
| `src/web/update.rs` | `GET /api/update` (statut du timer root, liste des versions, `version_pinning`, `show_prerelease`), `POST /api/update` (alias `/api/update/install`) : corps facultatif `{"version":"vX.Y.Z"}` validé (syntaxe **et** présence dans la liste du statut), écrit dans le fichier de demande de façon atomique ; `PUT /api/update/prefs` (`update-prefs.json`) ; en-tête `x-cuemix-update` exigé en écriture | ✅ |
| `src/measure/` | Test de pièce (D28) : `mod.rs` (tâche pilotée par l'engine : instantané → silence des autres entrées → MPD → attente → restauration exacte, chien de garde), `mpd.rs` (client MPD TCP texte, délais 2 s), `noise.rs` (`GET /api/measure/noise.wav`, bruit rose 90 s en flux) | ✅ |
| `src/zeroconf.rs` | Annonce mDNS `http://<mdns_hostname>.local:<port>` | ✅ |
| `tests/` | Mock MOTU + intégration `mock_motu`, `motu_client`, `osc_bridge`, `web_server`, `speaker_plug` (séquence complète contre une vraie prise HTTP sur 127.0.0.1), `osc_settings` (changement de réglages OSC en direct sur de vrais sockets UDP) | ✅ |

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
* **Marche/arrêt de Monitor (Main) et Phones** (`power`, D29). Le moteur possède le fondu :
  `Option<Fade>` par canal, une branche `sleep_until(prochaine échéance)` dans le `select!`.
  Même courbe que l'ancien fondu navigateur (smoothstep, linéaire en dB, 12 ms/dB, 250–1200 ms,
  un pas toutes les 28 ms sur une grille fixe, valeurs en dB entiers, dernier pas exactement à
  l'heure). Chaque pas est un `Changed` d'origine `System` (tous les clients voient le bouton
  bouger, y compris celui qui a appuyé). **Ce qui annule un fondu** : un `set` de ce volume
  (Web, OSC, API) ; une valeur du matériel à plus de 0,5 dB de tous les derniers pas écrits
  (l'écho d'un pas écrit ne l'annule pas : on retient les 8 derniers pas, conservés lors d'une inversion et pendant 2 s après la fin du fondu ; une valeur matérielle sans rapport avec eux est un geste). Les présets ne touchent pas ces volumes (D13) :
  ils n'annulent rien. **Niveau mémorisé** (mêmes règles que l'ancien navigateur) : Arrêt le
  mémorise sauf si un fondu tourne déjà ; Marche remonte jusqu'à lui puis l'oublie ; un `set`
  (ou un geste matériel) vers un vrai niveau l'oublie ; sans mémoire, Marche va à −70 dB. Fichier
  `power.json` (`[storage] power_file`), écrit après chaque changement, jamais dans un préset.
  « Marche » = niveau au-dessus du minimum ; `fading` vient du moteur.
* **Prise enceintes** (Main seulement, `[speaker_plug]`, D29) : Arrêt = fondu → fin au minimum →
  `off_delay_ms` → prise éteinte (pas si un geste a annulé le fondu : la prise reste allumée) ;
  Marche = prise allumée → réponse OK → `warmup_ms` → fondu (pas de réponse dans `timeout_ms` :
  fondu tout de suite + `notice` « Speaker plug did not respond ») ; Arrêt pendant l'attente annule
  la montée et éteint la prise. Seul `power` pilote la prise (ni un geste sur le bouton, ni
  Phones). La requête part dans une tâche, jamais dans la boucle : sa réponse revient comme
  message interne (`Internal`), numérotée — une réponse périmée (annulée entre-temps) est ignorée.
  Un `set_speaker_plug` est validé, **contrôlé par la barrière** (tâche), écrit dans
  `speaker-plug.json` puis diffusé ; l'état de la prise (`off|on|warming|error|unknown`) est celui
  de nos propres requêtes (la prise n'est pas interrogée).
* **Liaison MOTU** (D29) : le fondu tourne dans tous les modes (la démo reste utilisable), mais la prise
  n'est commandée, et un niveau n'est mémorisé (et écrit dans `power.json`), que si la liaison est `Online`
  **et** l'état lu sur la carte (`Synced`). Quand la liaison quitte `Online`, les fondus et la séquence de la
  prise s'arrêtent sans aucune requête à la prise (son état est laissé tel quel, `warming` redevient
  `unknown`). Un `power` qui aurait eu besoin de la prise hors de ce cas lance le fondu seul et diffuse
  `notice` « MOTU not connected: speaker plug left as is ».
* **Requêtes à la prise** : un seul agent (tâche + file) ; jamais deux requêtes en vol ; de la séquence on ne
  garde que le dernier souhait, qui n'est pas renvoyé si la prise est déjà connue dans cet état (même
  version des réglages) ; un changement de réglages invalide les réponses en route, et un `set_speaker_plug`
  plus ancien ne remplace jamais un plus récent (numéro d'ordre). `test_speaker_plug` passe par le même agent
  et est refusé pendant une séquence, et en « On » tant que Main n'est pas au minimum (« Turn Monitor off
  first »). Délai avant l'extinction : au moins 50 ms. Marche alors que Main est audible mais que la prise
  n'est pas connue allumée : le bouton est mis au minimum (enceintes hors tension, donc silence), puis
  prise → échauffement → remontée au niveau d'origine ; Marche pendant le délai d'extinction (prise encore
  allumée) remonte directement, sans échauffement. L'écriture de `power.json` se fait en tâche de fond
  (atomique, dernière valeur gagnante).
* `power` et `speaker_plug` sont émis à la connexion (après `measure`) et à chaque changement
  réel (comparés au dernier envoyé, à la fin de chaque tour de boucle) ; ils ne touchent jamais
  `● Modifié`.
* **Réglages OSC** (0.7.0, D30) : `set_osc_settings` est validé (syntaxe : `listen` `ip:port` avec port 1024–65535 sans multicast, 0 à 8 cibles `ip:port` / `[v6]:port` / `nom:port`),
  puis une tâche résout les noms et applique la **même barrière que la prise** (`plug::resolve_fenced` : privé, lien-local, bouclage) ;
  le pont reçoit alors `OscApply` : il lie le **nouveau** socket d'abord (échec : l'ancien continue d'écouter et le demandeur reçoit
  un `error`) — sauf quand le nouveau est sur le même port et que le système refuse tant que l'ancien est ouvert (`0.0.0.0:8000` → `127.0.0.1:8000`) :
  l'ancien est alors libéré le temps de l'essai et repris en cas d'échec. Les cibles changent sans nouveau socket ; un dump complet part vers les cibles **ajoutées**
  (pas vers celles qui restent). Après un nouveau socket, les pairs appris sont oubliés (ils s'inscrivent de nouveau en écrivant au nouveau port). Puis le moteur écrit `osc.json`
  (atomique) **avant** de diffuser `osc_settings` (qui est donc enregistré quand un client le voit) ; un seul changement à la fois (« another OSC change is in progress »).
  Les noms de cibles sont résolus à l'application et au démarrage, pas à chaque envoi. Familles : un socket IPv4 n'atteint que des cibles IPv4 ; `[::]` atteint les deux (IPv4 sous forme IPv4-mappée) ; une adresse IPv6 précise n'atteint que de l'IPv6.
  **Démarrage** : un nom de cible qui ne se résout pas (réseau pas encore monté) n'écarte pas `osc.json` : l'adresse d'écoute et les autres cibles sont gardées, la cause va dans `osc_settings.error`
  et le nom est réessayé **toutes les 60 s** (tick interne) par le même chemin ; une fois résolu, la cible reçoit un dump et l'erreur disparaît. Une adresse hors barrière n'est jamais réessayée.
  **Échec en direct** (port pris, adresse refusée, barrière) : le demandeur reçoit `error`, `osc_settings` est rediffusé avec la cause dans `error` ; le prochain changement réussi l'efface.
  `listening` est faux si le pont n'a plus de vrai socket (l'ancien n'a pu être repris après l'échec d'un déplacement sur le même port, `ApplyOutcome.listening`) ou si sa tâche s'est arrêtée.
* **Requêtes HTTP courtes** : `Request::Summary` et `Request::Api { command }` ; la seconde exécute
  la commande comme un client mais renvoie le **vrai résultat** (rejet capturé par `reply_error`)
  au lieu d'un événement `error` vers un WebSocket.

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
{"cmd":"power","channel":"main","on":false}
{"cmd":"set_speaker_plug","settings":{"enabled":true,"kind":"shelly_rpc","host":"plug.example","switch_id":0,"on_url":"","off_url":"","warmup_ms":2000,"off_delay_ms":300,"timeout_ms":2000}}
{"cmd":"test_speaker_plug","on":true}
{"cmd":"set_osc_settings","settings":{"listen":"0.0.0.0:8000","targets":["192.0.2.20:9000","ipad.local:9000"]}}
```

Serveur → client (`ServerEvent`, tag `event`) — à la connexion : `snapshot`, `labels`, `presets`, `measure`, `power`, `speaker_plug`, `osc_settings` (puis `patch` s'il est connu).

```json
{"event":"snapshot","state":{…},"session":{"active_preset":null,"modified":false,"connection":"demo"}}
{"event":"changed","params":[{"param":"dim","on":true}]}
{"event":"status","connection":"online"}
{"event":"session","active_preset":"live","modified":true,"connection":"online","device":{"model":"UL5E","firmware":"2.0.8+2568","topology":{…},"input_names":["Mic 1",…],"mix_names":["Main 1-2",…],"stereo_inputs":[…],"stereo_buses":[…]}}
{"event":"presets","names":["default","live"]}
{"event":"labels","channels":{"2":{"name":"Vox","color":"#06B6D4"}}}
{"event":"meters","levels":[0.0,0.12,…]}
{"event":"measure","state":"playing","source":[6,7],"until":"2026-11-16T20:15:30Z"}
{"event":"power","main":{"on":true,"fading":"out","plug":"on","return_db":-18.0},"phones":{"on":true,"fading":null,"plug":null,"return_db":null}}
{"event":"speaker_plug","settings":{…},"urls":{"on":"http://plug.example/rpc/Switch.Set?id=0&on=true","off":"…&on=false"},"state":"unknown"}
{"event":"speaker_plug_result","on":true,"ok":false,"message":"no answer (timeout)"}
{"event":"osc_settings","settings":{"listen":"0.0.0.0:8000","targets":["192.0.2.20:9000"]},"listening":true,"error":null}
{"event":"notice","level":"warning","message":"Speaker plug did not respond"}
{"event":"error","message":"…"}
```

`power` : `on` = niveau au-dessus du minimum (`TRIM_MIN_DB`), `fading` ∈ `null`/`"in"`/`"out"`
(pendant l'attente de la prise : Main = `"in"`, `plug` = `"warming"`), `plug` ∈ `null` (aucune prise
active, toujours `null` pour `phones`) ou l'état de la prise. Il n'appartient pas à l'état de la carte :
ni `● Modifié`, ni préset. `speaker_plug` : `settings` (voir ci-dessous), `urls` (`""` si les réglages ne
permettent pas de les construire), `state` ∈ `off|on|warming|error|unknown`. `speaker_plug_result`
ne va qu'au client qui a demandé le test (`test_speaker_plug` s'exécute sur les réglages **enregistrés**,
même désactivés ; un seul test à la fois). `notice` est diffusé à tous. Un `set_speaker_plug` refusé
(plage, URL, barrière) répond par un `error` au demandeur seul.

`osc_settings` (0.7.0) : `settings` = ce qui est en vigueur ; `listening` = le socket UDP est lié (faux seulement avant que le pont ne s'annonce, par exemple dans les tests) ;
`error` = `null`, ou le dernier problème : réglages enregistrés non (tous) utilisés au démarrage (fichier illisible, port pris : retour à `config.toml` ; cible non résolue : le reste est gardé), ou échec d'un changement en direct ; effacé par le changement suivant qui réussit. Un
`set_osc_settings` refusé (syntaxe, barrière, port occupé, « OSC is not running ») répond par un `error` au demandeur seul et ne change rien. Il n'est pas
disponible par `POST /api/v1/command`. Non transmis en OSC.

Réglages OSC (`OscSettings`) : `listen` (`ip:port`, `0.0.0.0` / `::` / bouclage / une adresse de la machine — le `bind` tranche —, port 1024–65535, ni multicast ni diffusion) et `targets`
(0 à 8, `ip:port`, `[v6]:port` ou `nom:port`, port 1–65535 ; chaque adresse résolue doit passer la barrière ci-dessous ; les doublons après résolution sont fusionnés). Les IP sont renvoyées
sous forme canonique, les espaces retirés.

Réglages de la prise (`SpeakerPlugSettings`) :

```json
{"enabled":true,"kind":"shelly_rpc","host":"plug.example","switch_id":0,"on_url":"","off_url":"",
 "warmup_ms":2000,"off_delay_ms":300,"timeout_ms":2000}
```

`kind` ∈ `shelly_rpc` (Gen2/Plus/Pro/Gen3/Gen4 : `http://HOST/rpc/Switch.Set?id=N&on=true|false`),
`shelly_gen1` (`http://HOST/relay/N?turn=on|off`), `tasmota` (`http://HOST/cm?cmnd=Power%20On|Off`,
`Power<N+1>` si N > 0), `custom` (`on_url` / `off_url` tels quels, `http://` obligatoire ; `host` vide).
`host` : IP ou nom, sans schéma, chemin ni identifiants ni espaces, IPv6 entre crochets, `:port`
optionnel. Plages : `switch_id` 0–15, `warmup_ms` 0–10000, `off_delay_ms` 0–5000, `timeout_ms`
500–10000 ; les URL ne sont exigées que si `enabled`. Les mêmes vecteurs valides/invalides
(`design/vectors/speaker-plug-urls.json`) servent à `cargo test` et à vitest.

**Barrière** (à l'enregistrement **et** avant chaque envoi) : `GET http://` seulement, aucune
redirection, aucun identifiant ; toute adresse résolue doit être privée (`10/8`, `172.16/12`,
`192.168/16`), lien-local (`169.254/16`, `fe80::/10`), de bouclage (`127/8`, `::1`) ou ULA IPv6
(`fc00::/7`) — une adresse IPv4 « mappée » suit la règle IPv4 ; la connexion se fait sur l'adresse
**déjà vérifiée** (pas de seconde résolution). Délai `timeout_ms` pour l'ensemble (résolution
comprise) ; seule la ligne d'état et les en-têtes sont lus (8 KiB max), 2xx = succès, le corps est ignoré.

#### API HTTP courte (`/api/v1`, version `1`, D29)

Pensée pour des appels de quelques secondes en arrière-plan (contrôles, widgets, Raccourcis) : pas de
WebSocket ni de snapshot. `GET /api/health` annonce `"api": 1` (entier, incrémenté si une route
change de façon incompatible ; les clients ignorent les champs inconnus). Les deux routes appliquent
la même règle `Origin` que `/ws` (403 `{"ok":false,"error":"origin not allowed"}` ; pas d'`Origin` =
accepté) et répondent avec `Cache-Control: no-store`.

`GET /api/v1/summary` → 200 :

```json
{"api":1,"version":"0.6.0",
 "main":{"db":-12.0,"on":true,"fading":null,"plug":"on","return_db":null},
 "phones":{"db":-100.0,"on":false,"fading":null,"plug":null,"return_db":-30.0},
 "preset":{"active":"live","modified":false},
 "presets":["default","live"],
 "motu":{"status":"online","model":"UL5E","firmware":"2.0.8+2568"}}
```

`db` = dB entiers `-100..0` ; `on`, `fading`, `plug`, `return_db` comme l'événement `power` (`return_db` : dB entiers vers lesquels un « Marche » ramène le canal tant qu'il est éteint ou en fondu de sortie — le niveau mémorisé, sinon −70 dB ; `null` quand il est allumé ou en fondu d'entrée ; absent des serveurs antérieurs) ; `motu.status` ∈
`online`/`connecting`/`demo` ; `model` / `firmware` = `null` sans appareil identifié.

`POST /api/v1/command` — corps JSON = **la même `Command` que `/ws`**, limitée à
`{"cmd":"set","param":"main_vol","db":-12}`, `{"cmd":"set","param":"phones_vol","db":-30}`,
`{"cmd":"power","channel":"main","on":true}` et `{"cmd":"load_preset","name":"live"}` (corps ≤ 16 KiB,
sinon 413 ; `Content-Type: application/json` obligatoire, sinon 415). Réponses : `200 {"ok":true}` ; sinon `{"ok":false,"error":"…"}` avec **400** (JSON invalide,
commande non permise — dont `measure_start`, qui reste lié au client WebSocket qui l'a lancé —,
valeur ou nom refusé par le moteur), **404** (préset inexistant), **403** (`Origin`), **503**
(moteur arrêté). Le changement est diffusé à tous les clients comme d'habitude (origine `System`).

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
qui l'a provoqué ; `meters` est limité à 60/s par client (la source en produit ≈ 30/s). Messages entrants ≤ 256 KiB (un fichier de réglages importé fait ≈ 20 Ko). `import_preset` enregistre le JSON reçu (même format qu'un fichier de preset, toute version lisible) sous `<name>.json` puis le rappelle comme `load_preset`.

**Vecteurs de protocole** (`design/vectors/protocol.json`, généré, jamais édité à la main) : un
exemple de chaque `Param`, `Command` et `ServerEvent`, sérialisé par les types Rust eux-mêmes
(`tests/protocol_vectors.rs`, qui refuse de compiler tant qu'une nouvelle variante n'a pas son
exemple), plus l'état obtenu en appliquant tous les `Param` (`after_params`). Côté web,
`frontend/src/lib/protocol.test.ts` vérifie que les commandes et événements typés par
`protocol.ts` sont identiques à ces JSON, que le réducteur traite chaque `Param` et chaque événement,
et qu'il aboutit au même état que le serveur. Un changement de protocole échoue donc d'un côté ou de
l'autre au lieu de dériver. Régénérer : `UPDATE_VECTORS=1 cargo test --test protocol_vectors`, puis
relire le diff. Le futur client iOS peut s'appuyer sur le même fichier.

### 3.6 Table OSC (UDP `:8000`, indices **1-based**)

| Adresse | Arg. | `Param` (conversion) |
|---|---|---|
| `/mix/{bus}/ch/{ch}/vol` | float `0..1` | `mix_vol` (`gain = (p/0.75)^K`, 0.75 = 0 dB) |
| `/mix/{bus}/ch/{ch}/pan` | float `-1..1` | `mix_pan` |
| `/mix/{bus}/ch/{ch}/mute` · `/solo` | int/float/bool | `mix_mute` · `mix_solo` |
| `/mix/main/vol` | float `0..1` | `main_vol` (trim : 1.0 = 0 dB, 0 = −100 dB) |
| `/mix/main/power` · `/controlroom/phones/power` | int/float/bool | `Command::Power` (fondu serveur, D29) ; feedback : int `0/1` = **où va** le canal (démarre à 1 dès qu'un fondu d'allumage commence, à 0 dès qu'un fondu d'extinction commence) |
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
configurées + pairs, **sauf l'émetteur** ; bundles découpés ≤ 8 KiB. Le feedback `power`
(`/mix/main/power`, `/controlroom/phones/power`) va à **tous** les pairs, émetteur compris (le fondu est
l'œuvre du serveur, pas un geste du pair), seulement pour les canaux dont l'état a changé, et fait
partie du dump complet. Non transmis en OSC :
`presets`, `labels`, `meters`, `meter_raw`, `status`, `session`, `speaker_plug`, `speaker_plug_result`, `osc_settings`, `notice`, `error`. `meter_raw` (`{"event":"meter_raw","raw":[255,13,…]}`) porte la trame de meters brute du MOTU (propriété 6000, non décodée), ≈ 30/s (`METER_RAW_INTERVAL` = 33 ms), pour la vue de diagnostic *Meter probe* (`/?probe`) qui sert à établir la table des meters.

`patch` (`{"event":"patch","sources":[46,47,…]}`) porte la table de patch 5010 lue sur la carte (index = destination, valeur = point source ; liste vide = inconnue). Le client MOTU l'accumule (`apply_patch_frame`), le moteur la garde (`Engine.patch`, hors `MotuState` et hors présets), la diffuse à chaque changement, la rejoue après les événements initiaux quand elle est connue et la vide en mode démo. Lecture seule : aucun `Command`, aucun encodeur, pas de `check_writable`. Côté web : `store.patch`, tableau « Patch (5010) » du *Meter probe*, et `outputSourcePoint` (`meterMap.ts`) qui fait suivre aux meters de sortie la source réellement patchée dès que `MK5_PATCH_DEST` (indice de destination par sortie, vide tant qu'il n'est pas lu sur la carte) est renseigné.

### 3.7 Configuration (`config.rs`)

Fichier : `--config` → `$CUEMIX_CONFIG` → `./config.toml` → `/etc/cuemix-web/config.toml` ;
absent = défauts, invalide = erreur, clés inconnues signalées par un avertissement (D15). Surcharges `CUEMIX_HTTP_BIND`,
`CUEMIX_OSC_LISTEN`, `CUEMIX_MOTU_IP`. `throttle_hz = 40` → 25 ms.

`[storage]` : `presets_dir`, `labels_file`, et (0.6.0) `power_file` (niveaux mémorisés, défaut
`/var/lib/cuemix-web/power.json`) et `speaker_plug_file` (réglages enregistrés depuis l'interface, défaut
`/var/lib/cuemix-web/speaker-plug.json`) — dans le volume Docker `cuemix-data` comme `labels.json`,
git-ignorés (`data/power.json`, `data/speaker-plug.json`). 0.7.0 : `osc_file` (réglages OSC enregistrés depuis l'interface, défaut `/var/lib/cuemix-web/osc.json`,
`{"listen":"0.0.0.0:8000","targets":[…]}`) et `update_prefs_file` (interrupteur « pré-versions », défaut `/var/lib/cuemix-web/update-prefs.json`,
`{"show_prerelease":bool}`), git-ignorés aussi (`data/osc.json`, `data/update-prefs.json`).

`osc.json` **remplace** `[network] osc_listen` / `osc_feedback_targets` (et donc `CUEMIX_OSC_LISTEN`) tant qu'il existe ; le supprimer rend la main à `config.toml`.
Illisible, invalide, hors barrière ou impossible à lier au démarrage : avertissement dans le journal, valeurs de `config.toml` utilisées, raison dans `osc_settings.error`.
Seul un `config.toml` dont l'adresse ne se lie pas arrête le serveur (comme avant 0.7.0). `update-prefs.json` remplace `[update] channel` pour l'interface seulement (jamais lu par root) ; sans fichier,
`channel` (`prerelease`, `pre`, `alpha` ou `beta`) donne la valeur par défaut de l'interrupteur.

`[speaker_plug]` (0.6.0) : mêmes champs que `SpeakerPlugSettings` (§3.5) ; défauts `enabled = false`,
`kind = "shelly_rpc"`, `host = ""`, `switch_id = 0`, `warmup_ms = 2000`, `off_delay_ms = 300`,
`timeout_ms = 2000`. Si `speaker_plug_file` existe, son contenu **remplace entièrement** cette section.
Des réglages invalides au démarrage (fichier ou section) **ou un `speaker-plug.json` illisible** donnent une
prise **désactivée** (avertissement dans le journal) : on ne revient jamais silencieusement à une autre prise. `config.example.toml` donne un exemple commenté en `192.0.2.10` ; les vraies adresses ne
vont jamais dans Git.

### 3.8 Mises à jour et choix de version (0.7.0, D30)

Trois acteurs, chacun avec ses droits :

* **Racine, minuterie** : `cuemix-update --check --status-file` (toutes les heures) écrit `update-status.json` — champs historiques (`installed`, `latest`, `tag`, `channel`,
  `update_available`, `checked_at`, `release_url`) plus `features` (`["pin"]` : cet outil sait installer la version demandée), `releases` (au plus 20, plus récentes d'abord, brouillons exclus :
  `{"tag","version","prerelease","published_at"}`), `latest_stable` et `latest_pre` (tag ou `null`). La liste vient de `GET /repos/…/releases?per_page=30`, lue **sans `jq`** (awk sur les clés
  de premier niveau de la sortie indentée de GitHub) ; chaque champ est revalidé avant d'être écrit. Format de liste non reconnu ou API en échec : liste vide, le contrôle n'échoue pas.
  La racine **ne lit pas** `update-prefs.json` (écrit par l'utilisateur web) : une demande vide installe la plus récente du canal de `config.toml`, comme avant ; l'interrupteur ne change que ce que propose l'interface (le serveur web calcule le badge à partir de `latest_stable` / `latest_pre`, et l'interface envoie toujours un tag explicite).
  Seules les versions ≥ le **plancher** (`v0.7.2` : `MIN_PIN_VERSION` dans `src/web/update.rs`, `MIN_VERSION` dans `deploy/cuemix-update.sh`, `deploy/min-version` ; la CI vérifie qu'ils sont égaux) figurent dans `releases` côté racine et web :
  une version plus ancienne réintroduirait une faille ou un bug grave corrigé depuis. La dernière release publie son plancher
  (asset `min-version`, lu par `cuemix-update` via `releases/latest/download/min-version`, 64 octets au plus, `X.Y.Z` strict) et le statut le
  recopie dans `min_version` : il ne peut que **relever** le plancher intégré, jamais l'abaisser (le serveur web garde le maximum des deux,
  car le statut est dans un répertoire où l'utilisateur web peut écrire). **Règle** : toute release qui corrige une faille ou un bug grave
  relève le plancher dans les trois fichiers.
* **Serveur web (non privilégié)** : `GET /api/update` renvoie `installed`, `channel`, `enabled`, `version_pinning` (le statut annonce `features` ∋ `pin`), `show_prerelease`, `update_available`,
  `latest` (`{tag,version,prerelease}` : le plus récent de *stable*, ou de *stable + pré-version* si `show_prerelease`, comparé par la précédence semver), `latest_stable`, `latest_pre`, `releases`,
  `status` (brut), `pending`, `pending_version`, `last_result` (`{code,outcome,at,version}`). `POST /api/update` accepte `{"version":"vX.Y.Z"}` : la syntaxe (regex ci-dessous) **et** la présence du tag dans `releases`
  sont vérifiées, la version installée est refusée, et `version_pinning` doit être vrai (sinon **422**, contrôlé avant la liste : on ne retombe jamais en silence sur « la plus récente ») ; une version sous le plancher est refusée (400, « releases older than v0.7.2 cannot be installed… »), pré-versions du plancher comprises. Deux installations acceptées à moins de **30 s** : la seconde reçoit **429**. La demande est écrite dans un fichier temporaire (nom unique) puis
  liée en dur (`link`) sur `update-request` : atomique, et refusée (409) s'il en existe déjà une. Sans corps : fichier vide, ancien comportement. `PUT /api/update/prefs` enregistre l'interrupteur.
  Les écritures exigent l'en-tête `x-cuemix-update` ; corps ≤ 1 KiB. Les trois routes appliquent aussi la règle `Origin` de `/ws` (403).
* **Racine, demande** : l'unité `.path` lance `cuemix-update --run-request FILE --result-file FILE --status-file FILE`. Le fichier de demande est une **entrée non fiable** (le service web peut y écrire n'importe quoi,
  y compris un lien symbolique) : fichier ordinaire de 64 octets au plus, lu **une seule fois** par `timeout 5 dd iflag=nofollow,nonblock bs=65 count=1` (un lien symbolique ou un FIFO échangé entre le contrôle et la lecture ne peut ni être suivi ni bloquer root), vide (la plus récente du canal) ou **un seul tag** (≥ plancher) `^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$` ; le contenu n'est jamais passé à un
  shell (un seul argument après `--version`) ni recopié dans les journaux ; tout autre contenu est refusé (code 2) sans rien installer, y compris un répertoire posé à la place du fichier (la suppression échoue sans arrêter l'enveloppe : le résultat est enregistré, et `TriggerLimitIntervalSec` / `TriggerLimitBurst` de l'unité `.path` empêchent une boucle). Le fichier est supprimé dès qu'il est lu. Une version plus ancienne est
  autorisée (rétrogradation) avec les mêmes garde-fous : sha256, binaire qui annonce la bonne version, contrôle de santé, retour arrière automatique. Le code de sortie va dans `update-result` (`CODE HORODATAGE [TAG]`),
  puis le statut est rafraîchi. `update-status.json` et `update-result` sont **construits dans `/var/lib/cuemix-update`** (répertoire réservé à root, 0700, créé par le script ; refusé s'il est un lien symbolique, n'appartient pas à root ou est inscriptible par d'autres), puis déplacés par `mv -fT` : root ne crée, n'écrit ni ne `chmod` jamais rien dans `/var/lib/cuemix-web` (écrivable par l'utilisateur web, sans bit collant), et un lien symbolique posé sur la destination est **remplacé**, pas suivi (`-T` empêche d'entrer dans un répertoire posé là). Une installation en cours est attendue (`--wait-lock`, 60 s) au lieu d'échouer « occupé » ; les deux services ont un `TimeoutStartSec` (contrôle 2 min, installation 15 min).
  *Piste, non faite* : durcissement `systemd` des deux services root (`ProtectSystem`, `PrivateTmp`, `NoNewPrivileges`…) ; il peut échouer avec 226/NAMESPACE dans un CT Proxmox non privilégié et n'a pas pu être essayé.

**Installations existantes** : le programme de mise à jour intégré ne remplace que le **binaire**. `cuemix-update` (script) et les unités `systemd` ne sont poussés que par les scripts d'installation
(`deploy/proxmox-setup.sh --update` côté hôte Proxmox, `deploy/linux-setup.sh --refresh-updater` sur Linux). Tant que ce n'est pas fait, le côté racine est l'ancien : son statut n'a pas `features`, donc
`version_pinning` est `false` et l'interface masque le choix de version (le bouton *Install* habituel marche comme avant).

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
  Sur un hôte Proxmox, le lien MOTU doit rester sur le pont isolé : règle `udev` + hook `post-up` + minuteur `systemd` de 30 s (`cuemix-motu-bridge --all`), et NetworkManager (s'il tourne) l'ignore (`NM_UNMANAGED`, `nmcli device set … managed no`) ; sinon ses réactivations détachent le lien et l'application retombe en démo. Voir `docs/HANDOFF.md` §6.1.
* **Throttling** : par paramètre, 1re valeur immédiate puis ≤ 1 / 25 ms, valeurs intermédiaires
  fusionnées, dernière garantie ; puis **cadenceur global ≥ 20 ms entre trames** (y compris les
  params multi-trames et les rafales de rappel) — la MOTU ferme la socket si on écrit trop vite.
* **Statut** : `Connecting` seulement au 1er essai et après une perte de connexion ; `Demo`
  pendant les nouvelles tentatives.

## 5. Chaîne de build & CI

* Ordre **non négociable** : `frontend` (`npm ci && npm run build`) **puis** `cargo build`
  (`rust-embed` sans `allow_missing` : pas de binaire sans UI).
* npm n'exécute jamais les scripts d'installation des paquets (`frontend/.npmrc` :
  `ignore-scripts=true`, et `npm ci --ignore-scripts` en CI) : c'est la voie habituelle d'un paquet
  compromis pour exécuter du code sur la machine de build. Aucun paquet du lock n'en a besoin
  (seul `fsevents`, optionnel et macOS uniquement, en déclare un).
* `.cargo/config.toml` : résolution compatible MSRV 1.85 (`cargo update` choisit des versions compatibles).
* `ci.yml` : frontend → rust (fmt, clippy `-D warnings`, tests) · msrv · shell (shellcheck) · gitleaks + RFC1918.
* `security.yml` : `cargo audit` · `npm audit` · dependency-review (PR) · CodeQL ; actions épinglées par SHA de commit.
* `release.yml` : musl x86_64/aarch64/armv7 (`cargo-zigbuild`), `.deb`, GitHub Release, image
  `ghcr.io` amd64/arm64 (distroless `nonroot`). Depuis 0.7.2 : `permissions: {}` par défaut, les jobs qui
  exécutent du code tiers (npm, scripts de build des crates) n'ont que `contents: read` ; seul `publish` a
  `contents: write` et seul `docker` a `packages: write` ; `persist-credentials: false` partout (le jeton ne reste
  pas dans `.git/config`) ; aucun cache (npm, `rust-cache`, Docker `gha`) dans les builds de release ; `deploy/min-version`
  est publié avec la release et doit être ≤ au tag. Toutes les actions, y compris celles de GitHub, sont
  épinglées par SHA de commit ; l'archive de gitleaks est vérifiée par sha256 ; l'image de base Docker est
  épinglée par digest ; un lancement manuel (`workflow_dispatch`) construit sans rien publier.
* Releases signées par minisign (D32) : `cuemix-update` vérifie la signature (clé épinglée, nom de fichier exact)
  avant le sha256 et avant toute extraction.
* `cuemix-update` n'exécute jamais en root un binaire seulement téléchargé : le contrôle `--version` passe par
  `setpriv` (uid/gid 65534, `--no-new-privs`, environnement vidé ; repli `runuser -u nobody`, sinon refus).
  Seul le membre `cuemix-web` de l'archive est extrait (`--no-same-owner`), et il doit être un fichier ordinaire
  (pas de lien symbolique).

## 6. Sécurité

* Aucun `unwrap`/`expect` en production ; trames et JSON malformés → erreurs, jamais de panique.
* Noms de présets validés (`[A-Za-z0-9 _.-]{1,64}`, pas de `.` initial) : pas de traversée.
* Serveur web : seuls les fichiers embarqués sont servis (`..`, `\`, NUL → `index.html`).
* **Garde d'hôte (anti DNS rebinding, 0.7.2)** : avant toute route, l'en-tête `Host` (ou le dernier
  `X-Forwarded-Host` si `trust_forwarded_headers`) doit être une adresse IP, `localhost`, un nom `*.local`,
  `network.mdns_hostname`, ou figurer dans `web.allowed_hosts` / les hôtes de `web.allowed_origins` ; sinon **421**.
  Une page piégée qui fait pointer son propre domaine vers le serveur envoie `Host` = `Origin` = son domaine :
  le contrôle `Origin` seul ne suffisait pas. Les noms acceptés sont journalisés au démarrage.
* **Jeton admin (D33, facultatif)** : installations, pré-versions, prise et OSC réservés aux appareils appairés
  quand `[web] admin_token = "auto"` (voir `src/web/admin.rs`).
* **Limites (0.7.2)** : boucle d'acceptation propre (`serve`, hyper HTTP/1 + `hyper-util`, déjà tirés par axum ;
  `axum::serve` n'a pas de délai de lecture des en-têtes) : **10 s** pour envoyer les en-têtes (attaque « slowloris »),
  **256** connexions HTTP servies à la fois (au-delà, la nouvelle connexion est fermée), WebSockets limités à **64**
  au total et **16** par adresse client (503 au-delà ; adresse = `X-Forwarded-For` si `trust_forwarded_headers`),
  **200** présets (nouveau nom refusé au-delà, écrasement toujours permis). Pont OSC : un datagramme dont la source
  n'est pas locale (même barrière que la prise : privée, lien-local, boucle) est ignoré, donc une adresse source
  falsifiée ne fait plus envoyer d'état à un tiers.
* **systemd (0.7.2)** : `cuemix-web.service` ajoute `PrivateDevices`, `ProtectProc=invisible`, `ProcSubset=pid`,
  `ProtectClock`, `ProtectHostname`, `ProtectKernelLogs`, `RestrictNamespaces`, `RestrictRealtime`,
  `MemoryDenyWriteExecute`, `SystemCallFilter=@system-service ~@privileged` (EPERM), `RestrictAddressFamilies=AF_INET
  AF_INET6 AF_UNIX AF_NETLINK` (netlink : liste des interfaces pour mDNS et le lien MOTU), `UMask=0077`,
  `MemoryMax=256M`, `TasksMax=256`, `LimitNOFILE=4096` (exposition `systemd-analyze security` : 1,4). Les unités root
  de mise à jour : `ProtectSystem=strict`, écriture limitée à `/usr/local/bin`, `/var/lib/cuemix-web`, `/run`,
  `StateDirectory=cuemix-update` (0700) et un `/tmp` privé. `atomic_publish` refuse de publier si les deux
  répertoires d'état ne sont pas sur le même système de fichiers (`stat -c %d`).
* **En-têtes de sécurité (0.7.2)** sur toutes les réponses : `Content-Security-Policy` (`default-src 'self'`,
  `script-src 'self'`, `style-src 'self' 'unsafe-inline'` pour les directives `style:` de Svelte,
  `connect-src 'self' ws://<hôte> wss://<hôte>`, `frame-ancestors 'none'`, `object-src 'none'`, `base-uri 'none'`,
  `form-action 'none'`), `X-Frame-Options: DENY` (anti clickjacking), `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer`, `Cross-Origin-Opener-Policy: same-origin`.
* `/ws` : en-tête `Origin` vérifié (même hôte que `Host`, ou `web.allowed_origins`), sinon 403
  (anti-détournement cross-site) ; envois bornés à 2 s (un client bloqué est déconnecté) ;
  `X-Forwarded-For` : entrée la plus à droite, seulement si `trust_forwarded_headers` (défaut `false`).
* `/api/v1/*` : même contrôle `Origin` que `/ws` ; commandes limitées à `main_vol`, `phones_vol`, `power`,
  `load_preset` (le test de pièce reste lié à son WebSocket) ; corps ≤ 16 KiB.
* **Couche de validation des entrées (0.7.1)** : tout ce qui entre est vérifié *avant* usage, puis relu
  dans des structures typées, jamais dans un `serde_json::Value` (`deny_unknown_fields` pour les corps
  de mise à jour et les réglages prise/OSC ; les `Command`/`Param` sont typées mais tolèrent les champs en trop).
  `web/body.rs` : taille, `Content-Type: application/json` (si corps), UTF-8, aucun caractère de
  contrôle (hors espaces JSON), puis le parse typé (413/415/400). `/api/v1/command` exige un corps JSON ;
  `POST /api/update` accepte l'absence de corps. `Command::validate()` (`state.rs`) est appelée par
  `/ws`, `/api/v1/command`, le pont OSC (`/preset/load`) **et** à l'entrée de `Engine::on_command` :
  noms de présets, texte d'import ≤ 256 KiB sans NUL, étiquettes (pas de contrôles ni de caractères bidi/
  largeur nulle, couleur `#RRGGBB`), textes de la prise et des réglages OSC. Les nombres des
  `Command::Set` restent contrôlés par `MotuState::apply` (non fini → refus, indice hors plage → refus,
  valeur finie → bornée). Prise : `plug::parse_url` n'accepte que l'ASCII non réservé `-._~/?&=:,+;` et
  `%XX` (≤ 2048), noms d'hôte par étiquettes (≤ 253) ; la requête GET écrite à la main ne peut donc jamais
  contenir CR/LF/espace. OSC : adresse ≤ 128 octets, ≤ 4 arguments, indices et ports en chiffres seulement.
  Fichiers relus (`labels.json`, `power.json`, `osc.json`, `speaker-plug.json`, statut de mise à jour) :
  contenu invalide → ignoré/valeurs par défaut. Limites connues : caractères de contrôle C1 dans le texte
  d'import (rejeté plus loin par le parse du préset) et noms réservés Windows (sans effet sous Linux). Le front reproduit les règles (formulaires), le serveur fait foi.
* **Prise enceintes** : le serveur reçoit une adresse à contacter, donc il est borné — `http://` en GET
  uniquement, sans redirection ni identifiants, délai court, réponse lue sur 8 KiB au plus, et toute
  adresse résolue doit être privée, lien-local ou de bouclage (à l'enregistrement **et** à chaque envoi,
  sur l'adresse vérifiée). Un seul test manuel à la fois. Les prises à mot de passe ne sont pas gérées.
* **Pas d'authentification** : n'exposer que sur un LAN de confiance ou derrière un proxy authentifiant.
* Fichiers locaux git-ignorés : `config.toml`, `*.local.toml`, `deploy/*.env`, `.env`, `*.pem`,
  `*.key`, `data/labels.json`, `data/power.json`, `data/speaker-plug.json`, `data/osc.json`, `data/update-prefs.json`, `data/presets/*.json`, `.claude/`.
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
| D24 | ~~(en partie remplacée par D30 : canal dans l'UI, fichier de demande lu)~~ Mises à jour **déclenchées par l'utilisateur depuis l'UI**, par canal de release (`[update] channel` = `stable` ou `prerelease`, dans `config.toml`, pas dans l'UI) ; jamais automatiques (arbitrage mainteneur 2026-11-15, remplace D16). Un timer root horaire (quotidien avant 0.5.1 ; `cuemix-update --check --status-file`) écrit le statut ; le bouton *Install* ne fait que créer un fichier de demande vide que l'unité `.path` root transforme en `cuemix-update` | Le service web reste non-root et sans accès réseau sortant ; seul le dernier release officiel du canal (sha256, santé, retour arrière) peut être installé, et seulement à la demande d'une personne |
| D25 | Meters : trame 6000 brute relayée (`meter_raw`, ≈ 30/s) et décodée **côté client** avec la table du bus de patch du mk5 (`frontend/src/lib/meterMap.ts`), confirmée au *Meter probe* et par la table de CueMix 5 | Le serveur reste agnostique ; d'autres modèles = une autre table |
| D26 | Onglet *Routing* supprimé ; `route` et la matrice restent dans l'état (inertes) pour la compatibilité des présets | La matrice booléenne ne correspond pas au matériel (5010 = une source par sortie) ; les mixes couvrent l'usage |
| D27 | Valeur 1006/1026 = **Q** (0,45-10) et non une largeur en octaves ; le champ garde le nom `bw_oct` pour les présets | Table et UI de CueMix 5 |
| D28 | Bruit de test de pièce fourni par le **MPD de l'utilisateur** (`[measure] mpd`, sortie analogique reliée à Line 7-8) et servi par `GET /api/measure/noise.wav` ; la carte n'a **aucun générateur** (table des propriétés de CueMix 5 vérifiée). Pendant le test : instantané puis silence des autres entrées du Main (bus 0), solos coupés, EQ de sortie Main (slot 0) et EQ d'entrée des sources en bypass, via les `Set` normaux ; restauration exacte des seules valeurs changées (une entrée déjà coupée reste coupée), puis reprise de MPD (morceau, position, pause ; MPD est mis en pause par le serveur pendant la phase `quiet`, un arrêt à ce stade fait simplement `pause 0`) ; chien de garde `max_seconds` (20-80 s) ; écritures « silence d'abord » (mutes/solos qui coupent, puis le reste, puis les démutes). L'analyse (micro du téléphone) se fait dans le navigateur | Pas de nouvelle fonction matériel ni de dépendance ; le bruit passe par la chaîne réelle (entrée → mixage → sortie) ; toute erreur réseau MPD aboutit à une restauration propre puis `error` → `idle`. La liste de restauration est écrite dans `measure-restore.json` (à côté de `labels.json`) avant tout changement et gardée jusqu'à ce que la liaison soit restée `online` 3 s après l'envoi : perte de liaison, débranchement, arrêt ou crash en cours de test → restauration rejouée dès que la MOTU est de nouveau `online` (y compris au redémarrage). Limite : après un crash, MPD n'est pas restauré |
| D29 | **Marche/arrêt de Monitor et Phones sur le serveur** (commande `power`, fondu du moteur, niveau mémorisé dans `power.json`), **prise enceintes** (Main : arrêt = fondu → délai → prise éteinte ; marche = prise → échauffement → fondu), et **API HTTP courte** `/api/v1` (arbitrage mainteneur 2026-11-25, `docs/IOS-PLAN.md` §5.1–5.2b ; sortie en 0.6.0). Le navigateur ne fait plus le fondu (`channelFade` et les clés `localStorage` supprimées, sans migration : le pire cas est un Marche qui monte à −70 dB). Le moteur applique la même courbe que l'ancien code web ; les réglages de la prise viennent de `[speaker_plug]` et du fichier enregistré depuis l'interface ; la requête est un `GET` HTTP écrit à la main sur `TcpStream` (aucune dépendance HTTP ajoutée) et **bornée par une barrière d'adresses locales** | Le niveau mémorisé et le fondu doivent être partagés entre appareils (téléphone, portable, OSC, contrôles iOS, qui ne peuvent pas faire tourner un fondu) ; une prise commandée par le serveur marche depuis n'importe quel client et ne coupe jamais les enceintes à pleine puissance. Compatibilité : `power` / `speaker_plug` / `notice` sont de nouveaux événements (un client ancien les ignore) ; l'événement `power` s'insère entre `measure` et `patch` à la connexion |
| D30 | **Réglages OSC enregistrés et appliqués en direct** (`osc.json`, commande `set_osc_settings`, événement `osc_settings`) ; **choix de la version à installer** dans l'interface, y compris plus ancienne. **Remplace les parties de D24** « canal pas dans l'UI » (désormais `update-prefs.json`, `PUT /api/update/prefs` ; `[update] channel` n'est plus que la valeur par défaut) et « contenu du fichier de demande jamais lu » (la racine le lit, comme **entrée non fiable** : 64 octets, regex stricte de tag, aucun passage par un shell, refus sans installation, aucune recopie dans les journaux ; le serveur web, lui, ne l'écrit qu'après avoir vérifié le tag dans la liste du statut). La séparation des droits ne change pas : le service web reste non privilégié, sans accès réseau sortant, et ne fait qu'écrire trois petits fichiers ; seule la racine télécharge, vérifie le sha256 et remplace le binaire. Les unités et `cuemix-update` ne se mettent pas à jour seuls : un statut sans `features` ∋ `pin` désactive le choix de version | Les réglages OSC se changent depuis la tablette sans éditer `config.toml` ni redémarrer ; revenir à une version antérieure est le seul remède rapide à une régression en séance (rétrogradation garantie par sha256, santé et retour arrière) ; rien de ce que le service web écrit ne peut choisir autre chose qu'un tag publié |
| D31 | **Plancher de version** (0.7.2) : ni l'interface ni la racine n'installent une release plus ancienne que la dernière qui corrige une faille ou un bug grave (`v0.7.2`, relevable par l'asset `min-version` de la dernière release) ; seul root, en ligne de commande, peut passer outre (`cuemix-update --version vX --force`). Restreint D30 (« y compris plus ancienne ») | Sans plancher, n'importe quel client du réseau (ou une page piégée) pouvait réinstaller une version vulnérable ; le retour arrière rapide reste possible entre versions saines |
| D32 | **Releases signées (minisign, 0.7.2)** : chaque fichier publié (`.tar.gz`, `.deb`, `min-version`) est signé par un job `sign` séparé, dans l'environnement GitHub protégé `release` (relecteur obligatoire, tags `v*`), qui n'exécute aucun code de build tiers et seul lit `MINISIGN_SECRET_KEY` / `MINISIGN_PASSWORD`. Le commentaire de confiance de chaque signature est `cuemix-web <nom du fichier>` ; le job vérifie tout contre `deploy/minisign.pub` avant publication. `cuemix-update` (clé épinglée `MINISIGN_PUBKEY`, égale à `deploy/minisign.pub`, vérifié par la CI), `linux-setup.sh` et le script de provisionnement Proxmox (clé passée par l'hôte) refusent tout fichier dont la signature est absente, invalide, d'une autre clé ou nomme un autre fichier ; `minisign` tourne sans privilèges. `min-version` non signé : ignoré (plancher intégré). Le sha256 reste vérifié | Le sha256 venait de la même release que le binaire : quiconque pouvait modifier une release (jeton volé, dépendance de build compromise) pouvait modifier les deux. Clé hors des jobs de build, approbation humaine à chaque release ; attestations GitHub écartées (un outil de plus dans le conteneur, dépendance à Sigstore) |
| D33 | **Jeton admin facultatif** (0.7.2, désactivé par défaut) : `[web] admin_token = "auto"` génère un jeton (fichier 0600) et journalise une fois un lien d'appairage ; `cuemix-web --pair-link` le réaffiche (root). Un appareil appairé (cookie `HttpOnly`, `SameSite=Strict`, ou `Authorization: Bearer`) seul peut : installer une release, changer l'interrupteur des pré-versions (`/api/update*` → 401 sinon), `set_speaker_plug`, `test_speaker_plug`, `set_osc_settings` (erreur au seul client sinon). Le droit d'un WebSocket est fixé à l'ouverture ; l'interface rouvre sa connexion après l'appairage. Faders, présets, marche/arrêt restent ouverts. Révocation : supprimer le fichier et redémarrer | Le réseau domestique compte aussi des invités et des objets connectés : ce qui change le logiciel installé ou les cibles réseau du serveur ne doit pas leur être ouvert, sans imposer une connexion à chaque musicien. Le jeton est dans le fragment de l'URL, jamais dans un journal ni un `Referer` |

### Application écran d'accueil (iOS)

- `frontend/public/` (copié tel quel dans `dist/`, donc embarqué par `rust-embed`) : `manifest.json` (`display: standalone`, `start_url: "./?simple"`, `scope: "./"`, couleurs `#121214`), icônes 192/512/maskable, `apple-touch-icon.png`, `icon.svg` (favicon). Tout est en URL **relative** (compatible sous-chemin de reverse proxy). Extension `.json` pour un `Content-Type` sûr côté `rust-embed`. Les icônes sont produites par `frontend/scripts/gen-icons.mjs` (Node seul, sans dépendance), qui régénère aussi `icon.svg`.
- Zones sûres : `viewport-fit=cover` + barre d'état `black-translucent` ; `env(safe-area-inset-*)` (repli `0px`, donc bureau inchangé) sur TopBar (haut, côtés), vue Simple et ControlRoom (bas), bandeau hors-ligne, toasts, tiroir DSP, RoomTest.
- `?simple` force la vue Simple pour ce chargement sans l'écrire dans `localStorage` ; priorité : `?simple` > `localStorage` > largeur d'écran. `?embed` seul (réservé à la future vue web de l'app native, où Simple est natif) ouvre la vue Avancée pour ce chargement (non sauvegardée) et masque le bouton Simple/Avancé et le point d'état ; `?embed&simple` ouvre Simple. Logique pure dans `frontend/src/lib/launch.ts`.
- Au retour de veille (`visibilitychange` visible, `pageshow` persisté), `store.reconnectNow()` rouvre la WebSocket immédiatement si elle n'est ni ouverte ni en cours de connexion (une seule tentative à la fois, backoff remis à zéro pour afficher « connexion » et non « hors ligne »). Si la socket paraît ouverte mais que la session est `online`/`demo` (mètres ≈ 30/s) et qu'aucun message n'arrive dans les 2 s suivant le retour au premier plan, elle est détachée sans attendre la fermeture et une nouvelle connexion est ouverte aussitôt. `manifest.json` porte `id: "./"` ; `theme_color` = couleur de la TopBar (`#16181d`), `background_color` = `#121214`.

### Vue Simple : affinages de la revue UX (non publié)

- Courbe des boutons de volume (Monitor, Phones ; vue Simple et vue Sortie) : `volKnobToNorm` / `volKnobFromNorm` (`frontend/src/lib/fader.ts`), c'est-à-dire la loi « trim » des curseurs étirée pour que le plancher (−100 dB) soit au début de l'arc. Vecteurs partagés avec l'app iOS : `design/vectors/volume-knob.json` (vérifiés par vitest). Flèches, molette et lecteur d'écran : pas exact de 1 dB (prop `keyStep` de `Knob.svelte`, avec `curve`).
- Boutons marche/arrêt : pendant un fondu, couleur de l'état visé et barre animée (immobile avec `prefers-reduced-motion`) ; préchauffage de la prise en ambre avec compte à rebours calculé côté client depuis `warmup_ms` des réglages de la prise et l'instant où l'état `warming` arrive. Sous un canal éteint, « returns to −18 dB » vient du champ `return_db` de l'événement `power` (ligne réservée, pour que le bouton ne bouge pas).
- Canal éteint : « Off » au lieu du plancher sous le bouton (et dans `aria-valuetext`).
- Présets en vue Simple : ligne « Preset » (préset courant, « ● Modified ») qui ouvre `PresetPicker.svelte` (fenêtre `Dialog`, un bouton Load par préset) ; `store.presetsOpen` compte dans `panelOpen`.
- Mètres (`meters.ts`) : graduations à −40/−18/−6 dBFS, repère d'écrêtage rouge mémorisé dès qu'une trame atteint ≈ −0,3 dBFS, effacé d'un toucher sur le mètre (les mètres ne servent plus de surface de glissement du bouton).

### Interface : corrections rapides de la revue UX (non publié)

- En-tête : commutateur à deux boutons **Simple | Avancé** (`aria-pressed` sur la vue courante, icônes sous 640 px) ; en vue Simple un espaceur pousse les boutons à droite comme en vue Avancée.
- Mode démo : pastille ambre « Demo » toujours visible (nom accessible « Demo — no MOTU ») ; le rouge reste réservé aux erreurs.
- Toasts : les erreurs (sans `level`) restent jusqu'au toucher, dans une région `role="alert"` ; avis et avertissements disparaissent après 5 s (`toastTimeout`, `frontend/src/lib/ui.ts`).
- L'onglet suit le fragment d'URL aussi après le chargement (`hashchange`, `tabFromHash` dans `ui.ts`).
- Sous 640 px, le panneau Présets est fixé aux deux bords de l'écran (il débordait de 121 px à 375 px) ; les cartes de la vue Entrée se partagent chaque ligne.

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
