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
| `Mono{on}` | 5018 | 0 | U8 | 0/1 (paramètre caché dans CueMix) | mk5-linux + licanccc |
| `Talkback{on}` | 5027 | 0 | U8 | 0/1 | mk5-linux |

Conversion volume (décision D8, unités natives) : `MainVol`/`PhonesVol` portent des **dB entiers -100..0** dans l'état ; le codec fait une copie directe `octet = -dB` (saturé à 0 dB : le trim ne sait qu'atténuer). La loi `0..1` n'existe qu'en OSC et dans l'UI. Non vérifié : si 100 = -inf ou -100 dB. Rappel de préset : trims et +48V ne sont **jamais** réécrits (règle 6 ci-dessous, arbitrage mainteneur).

Non mappés (pas de trame) : `MixVol/MixPan/MixMute/MixSolo`, `Route`, `EqBypass`, `EqBand`, `Hpf`, `Comp`, `Gate`, `Reverb`, `ReverbSend`, `Clock`, `Dim`, `SpeakerB`, et tout canal hors plage documentée.
- Mix (1016 fader 8.24 linéaire, 1017 pan 8.24 `0..1` avec 0,5 = centre, 1018 solo, 1019 mute ; 448 entrées) : **index = `bus × 32 + entrée`** (déduit du dump du 2026-11-15 : la diagonale « bus k ← retour USB k » tombe exactement sur `33k + 18`). Bus mono par paires : 0-1 Main 1-2, 2-3 Line 3-4, 4-5 Line 5-6, 6-7 Line 7-8, 8-9 Line 9-10, **10-11 Phones**, 12-13 Reverb. Une entrée mono est portée par le bus gauche du mix ; une paire stéréo par le bus gauche (entrée gauche) et le bus droit (entrée droite). **Lecture seule** dans cuemix-web : aucune écriture vérifiée. Le sens du pan (0 = gauche ?) n'est pas vérifié.
- Routage : 5010 est une table de 80 octets de sémantique inconnue, pas une matrice booléenne.
- EQ : 1003 = bypass **par bande** (32 = 4 bandes x 8 canaux, ordre inconnu) ; pas de bypass global.
- Dim : aucun paramètre matériel (licanccc l'émule avec le trim). SpeakerB : 5015-5017 (`ab/a/b_enable`) de sémantique inconnue.

`ENTRIES` contient 65 propriétés (tous types représentables, y compris celles non liées à un `Param`, pour que `kind_of` serve au décodage). Absentes car `Int16` non supporté par `ValueKind` : 8, 5012, 1009, 1010, 1013, 1014 ; `Array` (0, 2, 4). Meters : ID `6000` cité par mk5-linux (`METERS_ID`) mais jamais décodé ; non listé.

## Réponses aux questions du mainteneur

- **Q1 (fréquences)** : EQ/HPF en **entier Int32 en Hz brut** (`input_eq_freq` 1004, `output_eq_freq` 1024), pas en 8.24. Gain EQ (1005) et largeur de bande en octaves (1006) sont en 8.24. HPF = un mode de bande (`input_eq_mode` 3 = highpass), pas de propriété dédiée. Source : mk5-linux `properties.rs`. À implémenter avec `ValueKind::U32` dès que le layout d'index des bandes est connu.
- **Q2 (dimensions)** : entrées de mixage **32**, bus **14** (= 7 mixes stéréo, `mix_output_stereo` 14), préamps : gain sur **8** entrées (2 micros avec pad/48V + 6 lignes), EQ/compresseur sur 8 canaux, gate sur 2 ; noms : 18 entrées / 22 sorties (routage 18x22, cohérent avec `Topology` mais la table réelle est un patch de 80 octets), `output_trim` 12 sorties, EQ de sortie 3 bandes x 12. Appliqué : `Topology::ULTRALITE_MK5` = 32 entrées mixer, 8 préamplis, 2 entrées micro (décision D11).
- **Q4 (volumes mix = gain linéaire 8.24 ?)** : **non confirmé, non réfuté**. `mix_fader` est de type Float (8.24) ; les sources ne disent pas si c'est linéaire ou dB (seul indice : `gate_threshold` "0=-inf, 1=0dB", donc plutôt linéaire). Par contre, volumes Main et Phones ne sont **pas** en 8.24 : octets d'atténuation en dB (voir ci-dessus), ce qui contredit l'hypothèse Q4 pour ces deux paramètres.

## Dump de connexion (UltraLite-mk5, firmware 2.0.8+2568, capturé le 2026-11-15)

Une trame par message WebSocket, environ 2 500 trames non-mètre. Le nombre d'entrées d'une propriété par canal donne les dimensions : `1000` = 32 entrées de mixer, dans l'ordre de CueMix 5 : 18 physiques (8 analogiques, S/PDIF 1-2, ADAT 1-8), 12 retours USB (une paire par mix : Main 1-2, Line 3-4 … Line 9-10, Phones) et le retour de réverbération (30-31), `1001` = 14 bus mono (7 mix stéréo), `5001` = 8 préamplis, `5003` = 2 micros, `6` = 18 entrées, `7` = 22 sorties. `1000`/`1001` marquent le canal **gauche** de chaque paire stéréo. `5000` (output trim) a 16 entrées (casque = 10/11). Les noms `6`/`7` sont vides : CueMix calcule les noms par défaut (cuemix-web aussi, table par modèle). Écarts avec la table : `21` (modèle) fait 5 octets (`UL5E\0`), pas 32 ; `4` index 0 = version du firmware ; plusieurs propriétés ont des payloads `Int16` (2 octets : `8`, `1009`, `1010`, `1013`, `1014`, `5012`).

## Handshake / synchronisation complète

1. Découverte : annonces JSON en UDP multicast port 1280 (`{"uid","name","ip","model":"UltraLite-mk5","version","interval"}`).
2. WebSocket binaire sur `ws://<ip>:1280` (1281 sur certains Mac, et chemin `/<serial>` requis sur firmware récent d'après les commentaires du gist ; mk5-linux se connecte sans chemin avec le firmware 2.0.8).
3. **Aucune requête de lecture** : à la connexion l'appareil pousse tout seul l'état complet (> 2400 propriétés, une trame par index), puis diffuse chaque changement (y compris ceux de la face avant / d'autres clients). Pas de subscribe.
4. L'appareil envoie aussi en continu des mises à jour (meters) tant qu'il y a du signal : une synchro "jusqu'au silence" ne termine jamais ; mk5-linux borne la synchro initiale à 5 s (silence de 500 ms).
5. Écriture : trames `[id u16][index u16][len u16][payload]` (`Frame::encode_write`) — **le champ longueur est requis** (vérifié sur matériel le 2026-11-15 : sans lui, `5011` était émis mais jamais appliqué ; avec lui, le volume principal change). La lecture reste `[id][index][payload]` sans longueur ; l'appareil ferme le WebSocket si les écritures vont plus vite qu'il ne les applique (mk5-linux espace de 20 ms ; notre throttle de 25 ms est compatible). Ne pas ré-écrire tout l'état à la reconnexion (rafale de 2375 écritures -> déconnexions).
6. Ne jamais restaurer `main_trim`/`output_trim` automatiquement (niveau d'écoute).

## Écart de format de trame (à arbitrer, `protocol.rs` non modifié)

- Réception : `[id u16][index u16][payload]` conforme (mk5-linux ; licanccc lit la valeur en octet 4).
- Émission (tranché le 2026-11-15 sur un UltraLite-mk5) : il faut écrire `[id u16][index u16][len u16][payload]` comme licanccc et le gist m1no (ex. `1393 0000 0001 2b` = main trim −43 dB). Le format sans longueur de mk5-linux est ignoré par l'appareil.
- Types : les payloads `Int16` (2 octets) existent et n'ont pas de `ValueKind`. Le payload n'est pas autodécrit : sa taille dépend de la propriété (octet = 1, float 8.24 = 4, Int32 = 4, chaîne = 32).
- Le mute des bus de mix est `1028` (index 0 = Main, 10 = Phones) d'après licanccc, absent de mk5-linux ; la numérotation 1016-1019 (mk5-linux) et 1028 viennent peut-être de versions de firmware/CueMix différentes.

## Format des meters

Inconnu. mk5-linux définit `METERS_ID = 6000` sans le décoder et ignore ce flux. À capturer sur matériel avant d'alimenter `ServerEvent::Meters`.

## Inconnus à lever sur matériel

Écriture de la matrice de mix (layout de lecture connu) ; sens du pan ; layout EQ par bande/canal ; sémantique de 5010 (routage) et 5015-5017 ; plage réelle de gain ligne/micro par canal ; valeur 100 du trim ; linéarité de `mix_fader` ; chemin WebSocket selon firmware ; format des meters (6000).
