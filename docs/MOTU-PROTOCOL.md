# Protocole MOTU UltraLite-mk5 — table des propriétés

Statut : **tout est `verified: false`** (rien n'a été testé sur matériel). Aucun identifiant n'a été inventé : ce qui n'est pas documenté par les sources est laissé non mappé (`encode` -> aucune trame, `decode` -> `None`).

## Sources consultées

| Source | Licence | Usage |
|---|---|---|
| <https://github.com/MarkWind85/motu-mk5-linux> (`src/protocol/properties.rs`, `types.rs`, `device/connection.rs`, `device/state.rs`, `ctl.rs`, README, CHANGELOG), commit `bf43e082fb6891cc3fe845e953c7e16ad81c85e4` | GPL-2.0-or-later | IDs, types, comptes : faits recopiés ; aucun code copié, mapping ré-implémenté |
| <https://github.com/licanccc/motu-monitor-control> (`plugin.js`), commit `70b4969011264648b593778dd4a4c7249f726485` | MIT | confirmations sur matériel (5003, 5011, 5000, 5018, 1028), format d'écriture |
| <https://gist.github.com/m1no/90c5776df3f1c06e067076d14477ef43> (+ commentaires) | MIT | format d'écriture avec champ longueur, chemin `/<serial>` |
| <https://github.com/jpcarrascal/motu-ultralite-mk5-bridge> | n/a | README seul : pas de détails de protocole |

Pas d'accès à `app.asar` de CueMix 5 : les IDs viennent de `motu-mk5-linux` qui dit les avoir extraits de `dev.js`/`dev_common.js`.

## Propriétés mappées sur un `Param`

| Param | prop_id | index | payload | Encodage | Source |
|---|---|---|---|---|---|
| `PreampGain{ch,db}` | 5001 | `ch` (0..7) | U8 | dB entiers (arrondi ; clamp 0..74 micros ch 0-1, 0..20 lignes ch 2-7) | mk5-linux |
| `Phase{ch}` | 5002 | `ch` (0..7) | U8 | 0/1 | mk5-linux |
| `Pad{ch}` | 5003 | `ch` (0..1, micros) | U8 | 0/1 | mk5-linux + licanccc (confirmé) |
| `Phantom{ch}` | 5004 | `ch` (0..1) | U8 | 0/1 | mk5-linux |
| `MainVol{db}` | 5011 `main_trim` | 0 | U8 | atténuation 0..100 = 0..-100 dB | mk5-linux + licanccc |
| `PhonesVol{db}` | 5000 `output_trim` | écriture 10 et 11 (2 trames) ; lecture 10 seulement | U8 | idem | mk5-linux + licanccc |
| `OutputTrim{ch,db}` | 5000 `output_trim` | `ch` 0..=9 (sorties analogiques 1-10, comme l'onglet Output de CueMix 5) | U8 | idem | mk5-linux + dump du 2026-11-15 |
| `Mono{on}` | 5018 | 0 | U8 | 0/1 (paramètre caché dans CueMix) | mk5-linux + licanccc |
| `Talkback{on}` | 5027 | 0 | U8 | 0/1 | mk5-linux |

Conversion volume (décision D8, unités natives) : `MainVol`/`PhonesVol` portent des **dB entiers -100..0** dans l'état ; le codec fait une copie directe `octet = -dB` (saturé à 0 dB : le trim ne sait qu'atténuer). La loi `0..1` n'existe qu'en OSC et dans l'UI. Non vérifié : si 100 = -inf ou -100 dB. Rappel de préset : trims et +48V ne sont **jamais** réécrits (règle 6 ci-dessous, arbitrage mainteneur).

## Propriétés DSP / réverbe / horloge — lues sur le matériel, écriture non active

**Lu dans le dump ; écritures non vérifiées.** Layouts déduits du dump de connexion d'un UltraLite-mk5 (firmware 2.0.8, 2026-11-15) : les motifs d'index sont sans ambiguïté (valeurs par défaut répétées par canal). `props::decode` les lit ; `props::encode_unverified` sait les encoder (testé en aller-retour), mais **`props::encode` ne l'appelle pas** : `is_mapped` reste faux et rien n'est écrit. Chaque fonction sera activée séparément après un essai sur la carte.

| Param | prop_id | index | payload | Unité / remarque (valeurs du dump) |
|---|---|---|---|---|
| `InputEqMode{ch,band,mode}` | 1002 | `ch × 4 + band` (8 entrées analogiques × 4 bandes) | U8 | type brut : 0 = cloche, 2 = shelf ? (vu une fois), 3 = passe-haut (bande 0 de chaque canal) ; valeurs inconnues conservées |
| `InputEqBypass{ch,band,on}` | 1003 | idem | U8 | 1 = bypass (tout à 0 dans le dump) |
| `InputEqFreq{ch,band,hz}` | 1004 | idem | Int32 (U32 lu signé) | Hz entiers (défauts 45 / 350 / 3500 / 7000) |
| `InputEqGain{ch,band,db}` | 1005 | idem | 8.24 | dB (ex. −2,0, 2,0, −12,7) |
| `InputEqBw{ch,band,octaves}` | 1006 | idem | 8.24 | octaves (1,0 partout) |
| `OutputEq{Mode,Bypass,Freq,Gain,Bw}` | 1022–1026 | `out × 3 + band` (12 sorties × 3 bandes) | comme 1002–1006 | mêmes unités |
| `GateBypass{ch,on}` | 1007 | `ch` 0–1 (micros) | U8 | 1 = bypass |
| `GateThreshold{ch,db}` | 1008 | idem | 8.24 | dB (−46) |
| `GateAttack{ch,ms}` / `GateRelease` | 1009 / 1010 | idem | Int16 | ms (10 / 1000) |
| `CompBypass{ch,on}` | 1011 | `ch` 0–7 (entrées analogiques) | U8 | 1 = bypass (1) |
| `CompThreshold{ch,db}` | 1012 | idem | 8.24 | dB (−10) |
| `CompAttack{ch,ms}` / `CompRelease` | 1013 / 1014 | idem | Int16 | ms (10 / 1000) |
| `CompRatio{ch,ratio}` | 1015 | idem | 8.24 | `n` de `n:1` (2,0) |
| `CompMakeup{ch,gain}` | 1021 | idem | 8.24 | gain **linéaire**, 1,0 = 0 dB (un canal à 9,638) |
| `ReverbDecay` / `ReverbDamping` / `ReverbWidth{value}` | 1030 / 1031 / 1033 | 0 | U8 | 0–100 (86 / 90 / 50) |
| `ReverbPredelay{ms}` | 1032 | 0 | U8 | ms 0–100 (50) |
| `ReverbPreset{preset}` | 1034 | 0 | U8 | numéro brut (1) |
| `SampleRate{rate}` | 10 | 0 | U32 | Hz (48000) ; débit inconnu → ignoré |
| `ClockSource{source}` | 11 | 0 | U8 | 0 = S/PDIF, 2 = Optique, 3 = Interne ; autre code → ignoré |

Réverbe : les **départs** sont les faders du mix « Reverb » (mix 6 = bus mono 12-13 de la matrice, déjà lus comme `MixVol{bus:6,..}`) et le **retour** est l'entrée de mixage 30-31 (« Reverb L/R ») dans chaque mix. Il n'y a donc pas de propriété de départ/retour séparée. `1027` (niveau de sortie des bus, 14 entrées 8.24 linéaire) n'est pas mappé. Plages des params (`state::dsp_limits`) **provisoires**, choisies larges pour ne pas déformer une valeur lue.

### Non mappés en écriture

Pas de trame : `MixVol/MixPan/MixMute/MixSolo` et les params DSP / réverbe / horloge ci-dessus (lus seulement), `Route`, `Dim`, `SpeakerB`, et tout canal hors plage documentée.
- Mix (1016 fader 8.24 linéaire, 1017 pan 8.24 `0..1` avec 0,5 = centre, 1018 solo, 1019 mute ; 448 entrées) : **index = `bus × 32 + entrée`** (déduit du dump du 2026-11-15 : la diagonale « bus k ← retour USB k » tombe exactement sur `33k + 18`). Bus mono par paires : 0-1 Main 1-2, 2-3 Line 3-4, 4-5 Line 5-6, 6-7 Line 7-8, 8-9 Line 9-10, **10-11 Phones**, 12-13 Reverb. Une entrée mono est portée par le bus gauche du mix ; une paire stéréo par le bus gauche (entrée gauche) et le bus droit (entrée droite). Écriture : même layout que la lecture (`motu::identify::encode_mix`) ; une tranche stéréo de l'UI écrit les deux moitiés (fader, mute, solo), le pan seulement à gauche. **À confirmer sur la carte** : l'écriture elle-même et le sens du pan (0 = gauche supposé).
- Routage : 5010 est une table de 80 octets de sémantique inconnue, pas une matrice booléenne.
- EQ : 1003 = bypass **par bande** (index `ch × 4 + bande`, voir ci-dessus) ; pas de bypass global (le `eq_bypass` de cuemix-web ≤ 0.3 a été retiré).
- Dim : aucun paramètre matériel (licanccc l'émule avec le trim). SpeakerB : 5015-5017 (`ab/a/b_enable`) de sémantique inconnue.

`ENTRIES` contient 69 propriétés (tous types représentables, y compris celles non liées à un `Param`, pour que `kind_of` serve au décodage). `Int16` est représenté par `ValueKind::I16` (2 octets BE signés) : 1009, 1010, 1013, 1014 sont listées ; 8 et 5012 (`Int16` aussi) restent absentes faute de sémantique connue. Absentes : `Array` (0, 2, 4) ; `1027` (niveau de sortie des bus, 14 entrées 8.24 linéaire) volontairement non mappée. Meters : ID `6000` cité par mk5-linux (`METERS_ID`) mais jamais décodé ; non listé.

## Réponses aux questions du mainteneur

- **Q1 (fréquences)** : EQ/HPF en **entier Int32 en Hz brut** (`input_eq_freq` 1004, `output_eq_freq` 1024), pas en 8.24. Gain EQ (1005) et largeur de bande en octaves (1006) sont en 8.24. HPF = un mode de bande (`input_eq_mode` 3 = highpass), pas de propriété dédiée. Source : mk5-linux `properties.rs`. ✅ Décodé (`ValueKind::U32` lu en Int32) depuis le dump du 2026-11-15 (layout `ch × 4 + bande`).
- **Q2 (dimensions)** : entrées de mixage **32**, bus **14** (= 7 mixes stéréo, `mix_output_stereo` 14), préamps : gain sur **8** entrées (2 micros avec pad/48V + 6 lignes), EQ/compresseur sur 8 canaux, gate sur 2 ; noms : 18 entrées / 22 sorties (routage 18x22, cohérent avec `Topology` mais la table réelle est un patch de 80 octets), `output_trim` 12 sorties, EQ de sortie 3 bandes x 12. Appliqué : `Topology::ULTRALITE_MK5` = 32 entrées mixer, 8 préamplis, 2 entrées micro (décision D11).
- **Q4 (volumes mix = gain linéaire 8.24 ?)** : **non confirmé, non réfuté**. `mix_fader` est de type Float (8.24) ; les sources ne disent pas si c'est linéaire ou dB (seul indice : `gate_threshold` "0=-inf, 1=0dB", donc plutôt linéaire). Par contre, volumes Main et Phones ne sont **pas** en 8.24 : octets d'atténuation en dB (voir ci-dessus), ce qui contredit l'hypothèse Q4 pour ces deux paramètres.

## Dump de connexion (UltraLite-mk5, firmware 2.0.8+2568, capturé le 2026-11-15)

Une trame par message WebSocket, environ 2 500 trames non-mètre. Le nombre d'entrées d'une propriété par canal donne les dimensions : `1000` = 32 entrées de mixer, dans l'ordre de CueMix 5 : 18 physiques (8 analogiques, S/PDIF 1-2, ADAT 1-8), 12 retours USB (une paire par mix : Main 1-2, Line 3-4 … Line 9-10, Phones) et le retour de réverbération (30-31), `1001` = 14 bus mono (7 mix stéréo), `5001` = 8 préamplis, `5003` = 2 micros, `6` = 18 entrées, `7` = 22 sorties. `1000`/`1001` marquent le canal **gauche** de chaque paire stéréo. `5000` (output trim) a 16 entrées (casque = 10/11). Les noms `6`/`7` sont vides : CueMix calcule les noms par défaut (cuemix-web aussi, table par modèle). Écarts avec la table : `21` (modèle) fait 5 octets (`UL5E\0`), pas 32 ; `4` index 0 = version du firmware ; plusieurs propriétés ont des payloads `Int16` (2 octets : `8`, `1009`, `1010`, `1013`, `1014`, `5012`).

## Handshake / synchronisation complète

1. Découverte : annonces JSON en UDP multicast port 1280 (`{"uid","name","ip","model":"UltraLite-mk5","version","interval"}`).
2. WebSocket binaire sur `ws://<ip>:1280` (1281 sur certains Mac, et chemin `/<serial>` requis sur firmware récent d'après les commentaires du gist ; mk5-linux se connecte sans chemin avec le firmware 2.0.8). **Observé sur matériel (2026-11-15, Proxmox) :** découverte par l'annonce UDP puis connexion à `ws://169.254.x.x:1280` **sans chemin** réussie.
3. **Aucune requête de lecture** : à la connexion l'appareil pousse tout seul l'état complet (> 2400 propriétés, une trame par index), puis diffuse chaque changement (y compris ceux de la face avant / d'autres clients). Pas de subscribe.
4. L'appareil envoie aussi en continu des mises à jour (meters) tant qu'il y a du signal : une synchro "jusqu'au silence" ne termine jamais ; mk5-linux borne la synchro initiale à 5 s (silence de 500 ms).
5. Écriture : trames `[id u16][index u16][len u16][payload]` (`Frame::encode_write`) — **le champ longueur est requis** (vérifié sur matériel le 2026-11-15 : sans lui, `5011` était émis mais jamais appliqué ; avec lui, le volume principal change). La lecture reste `[id][index][payload]` sans longueur ; l'appareil ferme le WebSocket si les écritures vont plus vite qu'il ne les applique (mk5-linux espace de 20 ms ; notre throttle de 25 ms est compatible). Ne pas ré-écrire tout l'état à la reconnexion (rafale de 2375 écritures -> déconnexions).
6. Ne jamais restaurer `main_trim`/`output_trim` automatiquement (niveau d'écoute).

## Écart de format de trame (à arbitrer, `protocol.rs` non modifié)

- Réception : `[id u16][index u16][payload]` conforme (mk5-linux ; licanccc lit la valeur en octet 4).
- Émission (tranché le 2026-11-15 sur un UltraLite-mk5) : il faut écrire `[id u16][index u16][len u16][payload]` comme licanccc et le gist m1no (ex. `1393 0000 0001 2b` = main trim −43 dB). Le format sans longueur de mk5-linux est ignoré par l'appareil.
- Types : les payloads `Int16` (2 octets) sont décodés via `ValueKind::I16`. Le payload n'est pas autodécrit : sa taille dépend de la propriété (octet = 1, float 8.24 = 4, Int32 = 4, chaîne = 32).
- Le mute des bus de mix est `1028` (index 0 = Main, 10 = Phones) d'après licanccc, absent de mk5-linux ; la numérotation 1016-1019 (mk5-linux) et 1028 viennent peut-être de versions de firmware/CueMix différentes.

## Format des meters

Partiellement observé (dump de connexion d'un UltraLite-mk5, firmware 2.0.8, musique sur Line 3-4) : chaque trame est `[6000][index 0][100 octets]`, un octet par point de mesure, environ 20 trames/s. `0xff` sur la plupart des points (point inactif ou silence ?), `0x00` sur certains, des valeurs qui bougent (≈ 4-17, ≈ 150-160, ≈ 190-224) sur une trentaine d'autres. Ni l'affectation des 100 points (entrées, mixes, sorties) ni la loi octet → dB ne sont connues.

**Table établie (UltraLite-mk5)** : la trame suit la numérotation du bus de patch interne. Niveau = −octet/2 dB, 255 = rien.

| Points | Contenu |
|---|---|
| 0-22 | lecture USB (ordinateur → carte), une paire par mix : 0-1 Main, 2-3 Line 3-4 … 10-11 Phones, 12-13 S/PDIF, 14-21 ADAT |
| 24-31 | entrées ADAT 1-8 |
| 32-43 | entrées analogiques dans l'ordre matériel : 32 Mic 2, 33 Line 7, 34 Line 8, 35 Line 5, 36 Line 6, 37 Mic 1, 38 Line 3, 39 Line 4 (après préampli, avant DSP) |
| 44-45 | entrée S/PDIF |
| 46-47, 48-49, 54-57, 62-63, 64-65 | sorties (traitement de sortie, avant trim) : Main, Line 3-4, Line 5-8, Line 9-10, Phones |
| 50-53, 58-61 | entrées analogiques après EQ/dynamique : Mic 1, Mic 2, Line 3, Line 4, puis Line 5-8 |
| 68-69 | retour de réverbe |
| 80-87 | après EQ : Mic 1-2, Line 3-8 |
| 88-89, 90-91 | réduction de gain du gate, après gate (Mic 1-2) |
| 92-93, 94-99 | réduction de gain du compresseur : Mic 1-2, Line 3-8 (0 = aucune) |

Vérifié au probe : souffle de Mic 1 / Mic 2 (37/50/80/90, 32/40?/51/81/91), entrée optique (24-31), Line 7-8 câblées (33-34, 60-61, 86-87), Main (46-47). Les points 78-79 suivent le mix Main mais ne sont pas identifiés.

Pour vérifier une autre carte, cuemix-web transmet la trame brute (`ServerEvent::MeterRaw`, ≈ 15/s) à la vue de diagnostic **Meter probe** (`/?probe`) : on règle une référence en silence, on joue un signal sur une entrée/sortie connue, la vue surligne les points qui ont bougé et copie un instantané JSON. `ServerEvent::Meters` (niveaux linéaires par point) reste vide tant que la table n'est pas établie.

## Inconnus à lever sur matériel

Écriture des params DSP / réverbe / horloge (layouts connus, encodeurs écrits mais non actifs) ; sens du pan ; numéros de preset de réverbe ; linéarité de `mix_fader` ; points 78-79 des meters ; chemin WebSocket selon firmware (sans chemin : confirmé sur un appareil, firmware 2.0.8).

## Table des propriétés de CueMix 5 (UltraLite-mk5)

CueMix 5 (installé, `resources/app/src/www/dev.js`) décrit chaque propriété du mk5 (id, type, sens). Faits retenus, vérifiés quand c'était possible :

- **5000 (trims de sortie)** : indexé dans l'**ordre matériel des DAC**, pas l'ordre de l'interface : 0-5 = Line 5-10, 6-7 = Main 1-2, 8-9 = Line 3-4, 10-11 = Phones. Octet 0-100 = 0 à −100 dB. cuemix-web traduit (`OUTPUT_TRIM_INDEX`).
- **1002 / 1022 (type de bande EQ)** : 0 = cloche, 1 = plateau grave, 2 = plateau aigu, 3 = passe-haut.
- **5006 (format optique)** : octet, index 0 = entrée, 1 = sortie ; 0 = ADAT, 1 = TOSLINK. Écrit par cuemix-web (`optical_mode`).
- **5001 (gain d'entrée)** : micros 0-74 dB, lignes 0-20 dB.
- **5010 (patch/routage)** : un octet par destination (`kOD_*`), valeur = source du bus de patch (même numérotation que les meters). Non encore implémenté.
- **5014** : source du loopback ; **5015-5019** : A/B, A, B, mono et mute *enable* ; **5024** sortie word clock ; **5025** expandeur optique ; **5026-5031** talkback (latch, enable, level, dim, source, destination).
- Entrées analogiques dans l'ordre matériel (bus de patch 32 + n) : Mic 2, Line 7, Line 8, Line 5, Line 6, Mic 1, Line 3, Line 4.

