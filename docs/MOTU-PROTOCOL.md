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
- Mix : les propriétés existent (1016 fader, 1017 pan, 1018 solo, 1019 mute, 448 entrées = 32 entrées x 14 bus) mais **l'ordre des index (bus*32+in ou in*14+bus) n'est documenté nulle part**, ni la gestion du pan/stéréo sur 2 bus. Il faut une capture CueMix.
- Routage : 5010 est une table de 80 octets de sémantique inconnue, pas une matrice booléenne.
- EQ : 1003 = bypass **par bande** (32 = 4 bandes x 8 canaux, ordre inconnu) ; pas de bypass global.
- Dim : aucun paramètre matériel (licanccc l'émule avec le trim). SpeakerB : 5015-5017 (`ab/a/b_enable`) de sémantique inconnue.

`ENTRIES` contient 65 propriétés (tous types représentables, y compris celles non liées à un `Param`, pour que `kind_of` serve au décodage). Absentes car `Int16` non supporté par `ValueKind` : 8, 5012, 1009, 1010, 1013, 1014 ; `Array` (0, 2, 4). Meters : ID `6000` cité par mk5-linux (`METERS_ID`) mais jamais décodé ; non listé.

## Réponses aux questions du mainteneur

- **Q1 (fréquences)** : EQ/HPF en **entier Int32 en Hz brut** (`input_eq_freq` 1004, `output_eq_freq` 1024), pas en 8.24. Gain EQ (1005) et largeur de bande en octaves (1006) sont en 8.24. HPF = un mode de bande (`input_eq_mode` 3 = highpass), pas de propriété dédiée. Source : mk5-linux `properties.rs`. À implémenter avec `ValueKind::U32` dès que le layout d'index des bandes est connu.
- **Q2 (dimensions)** : entrées de mixage **32**, bus **14** (= 7 mixes stéréo, `mix_output_stereo` 14), préamps : gain sur **8** entrées (2 micros avec pad/48V + 6 lignes), EQ/compresseur sur 8 canaux, gate sur 2 ; noms : 18 entrées / 22 sorties (routage 18x22, cohérent avec `Topology` mais la table réelle est un patch de 80 octets), `output_trim` 12 sorties, EQ de sortie 3 bandes x 12. Appliqué : `Topology::ULTRALITE_MK5` = 32 entrées mixer, 8 préamplis, 2 entrées micro (décision D11).
- **Q4 (volumes mix = gain linéaire 8.24 ?)** : **non confirmé, non réfuté**. `mix_fader` est de type Float (8.24) ; les sources ne disent pas si c'est linéaire ou dB (seul indice : `gate_threshold` "0=-inf, 1=0dB", donc plutôt linéaire). Par contre, volumes Main et Phones ne sont **pas** en 8.24 : octets d'atténuation en dB (voir ci-dessus), ce qui contredit l'hypothèse Q4 pour ces deux paramètres.

## Handshake / synchronisation complète

1. Découverte : annonces JSON en UDP multicast port 1280 (`{"uid","name","ip","model":"UltraLite-mk5","version","interval"}`).
2. WebSocket binaire sur `ws://<ip>:1280` (1281 sur certains Mac, et chemin `/<serial>` requis sur firmware récent d'après les commentaires du gist ; mk5-linux se connecte sans chemin avec le firmware 2.0.8).
3. **Aucune requête de lecture** : à la connexion l'appareil pousse tout seul l'état complet (> 2400 propriétés, une trame par index), puis diffuse chaque changement (y compris ceux de la face avant / d'autres clients). Pas de subscribe.
4. L'appareil envoie aussi en continu des mises à jour (meters) tant qu'il y a du signal : une synchro "jusqu'au silence" ne termine jamais ; mk5-linux borne la synchro initiale à 5 s (silence de 500 ms).
5. Écriture : trames `[id][index][payload]` ; l'appareil ferme le WebSocket si les écritures vont plus vite qu'il ne les applique (mk5-linux espace de 20 ms ; notre throttle de 25 ms est compatible). Ne pas ré-écrire tout l'état à la reconnexion (rafale de 2375 écritures -> déconnexions).
6. Ne jamais restaurer `main_trim`/`output_trim` automatiquement (niveau d'écoute).

## Écart de format de trame (à arbitrer, `protocol.rs` non modifié)

- Réception : `[id u16][index u16][payload]` conforme (mk5-linux ; licanccc lit la valeur en octet 4).
- Émission : mk5-linux écrit `[id][index][payload]` **sans longueur** ; en revanche licanccc et le gist m1no écrivent `[id u16][index u16][len u16][payload]` (ex. `0404 0000 0001 01`). Les deux affirment fonctionner ; à trancher sur matériel. Si la longueur est requise en écriture, `Frame::encode` devra l'ajouter (non fait).
- Types : les payloads `Int16` (2 octets) existent et n'ont pas de `ValueKind`. Le payload n'est pas autodécrit : sa taille dépend de la propriété (octet = 1, float 8.24 = 4, Int32 = 4, chaîne = 32).
- Le mute des bus de mix est `1028` (index 0 = Main, 10 = Phones) d'après licanccc, absent de mk5-linux ; la numérotation 1016-1019 (mk5-linux) et 1028 viennent peut-être de versions de firmware/CueMix différentes.

## Format des meters

Inconnu. mk5-linux définit `METERS_ID = 6000` sans le décoder et ignore ce flux. À capturer sur matériel avant d'alimenter `ServerEvent::Meters`.

## Inconnus à lever sur matériel

Layout d'index des 1016-1019 ; layout EQ par bande/canal ; sémantique de 5010 (routage) et 5015-5017 ; plage réelle de gain ligne/micro par canal ; valeur 100 du trim ; linéarité de `mix_fader` ; champ longueur en écriture ; chemin WebSocket selon firmware ; format des meters (6000).
