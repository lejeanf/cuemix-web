# Handoff Document — `cuemix-web` (MOTU UltraLite-mk5 Web & OSC Control Bridge)

**Version du Document :** `12.0.0-UNIVERSAL-EDITION`  
**Statut :** Verrouillé & Prêt pour Exécution Multi-Agents  
**Dépôt GitHub :** `https://github.com/lejeanf/cuemix-web`  
**Licence recommandée :** MIT ou Apache-2.0  

---

## 1. Vision & Objectifs du Projet

**`cuemix-web`** est un serveur autonome ultra-léger écrit en **Rust**, embarquant une interface web moderne en **Svelte (Dark Theme)**, permettant de contrôler intégralement une interface audio **MOTU UltraLite-mk5** via sa connexion réseau virtuelle USB-C (`CDC Ethernet`), sans nécessiter l'application officielle CueMix 5 ni un ordinateur Mac/Windows allumé.

### Objectifs clés
1. **Parité fonctionnelle 100 % avec CueMix 5 :** Préamplis, 7 mixages stéréo, EQ paramétrique 4 bandes, compresseur, gate, reverb, matrice de routage complète, VU-mètres temps réel et réglages matériels.
2. **Contrôle réseau universel & OSC bidirectionnel :** Pilotage simultané depuis n'importe quel navigateur (tablette, smartphone, ordinateur) et depuis des contrôleurs OSC (TouchOSC, QLab, Stream Deck, Max/MSP) avec retour d'état en temps réel.
3. **Déploiement multi-plateforme clé en main :**
   * **Proxmox VE (LXC `x86_64`)** derrière un reverse proxy (HAProxy / pfSense).
   * **Cartes SBC ARM (`aarch64` / `armv7`) :** Raspberry Pi (Zero 2 W, 3, 4, 5), Orange Pi, Radxa Rock sous Raspberry Pi OS, Debian ou Armbian.
   * **Distributions Linux Desktop/Studio (`x86_64` & `aarch64`) :** Ubuntu Studio, Debian, Pop!_OS, Arch Linux, SteamOS via binaires statiques `musl` et paquets `.deb`.
   * **Conteneurs Docker / Podman (`ghcr.io/lejeanf/cuemix-web`) :** Multi-architecture (`linux/amd64` et `linux/arm64`) pour Unraid, TrueNAS, Synology et serveurs Linux génériques.
4. **Confidentialité & Sécurité stricte :** Aucune adresse IP privée, identifiant ou configuration réseau personnelle ne doit être versionné dans le dépôt public.

---

## 2. Architecture Technique & Protocole MOTU

### 2.1 Pile Logicielle
* **Backend (Rust 2021+ / MSRV 1.80+) :**
  * `tokio` : Runtime asynchrone multi-tâches.
  * `tokio-tungstenite` : Client WebSocket binaire vers la MOTU (`ws://<MOTU_IP>:1280`).
  * `axum` : Serveur HTTP/WebSocket JSON pour le frontend et compatibilité reverse-proxy (`X-Forwarded-*`).
  * `rosc` : Encodeur/décodeur OSC sur socket UDP (`0.0.0.0:8000`).
  * `rust-embed` : Inclusion des fichiers statiques compilés du frontend Svelte directement dans le binaire exécutable unique.
  * `if-addrs` & `mdns-sd` : Détection réseau lien-local et annonce Zeroconf (`http://cuemix-web.local:8080`).
  * `serde` & `toml` : Gestion de la configuration externalisée, des présets et des libellés personnalisés.
* **Frontend (Svelte 5 + Vite + TypeScript) :**
  * Compilé en bundle statique dans `frontend/dist/`.
  * Communication temps réel avec le backend via WebSocket JSON (`/ws`).

### 2.2 Protocole Binaire MOTU UltraLite-mk5 & Découverte IP
* **Transport :** Interface USB-C exposant une carte réseau virtuelle `CDC Ethernet` sur le sous-réseau lien-local `169.254.0.0/16`, serveur WebSocket sur le port **`1280`**.
* **Format des trames :** Paquets binaires *Big-Endian* : `[prop_id: u16] [index: u16] [payload]`.
* **Encodage des valeurs :**
  * Flottants (gains, volumes, fréquences) : **Virgule fixe signée `8.24`** (`raw_i32 = (val * 16_777_216.0) as i32`).
  * Booléens / États : `u32` ou `u8` selon la propriété (référentiel `motu-mk5-linux` & extraction `app.asar` de CueMix 5).
  * Chaînes (noms matériels) : Buffer de 32 octets terminé par `\0`.
* **Loi Logarithmique Audio Unifiée :**
  * Les faders Web et OSC utilisent une plage normalisée `[0.0 .. 1.0]`.
  * Conversion centralisée dans `motu_protocol::fader` : `0.0` = `-inf dB` (gain linéaire `0.0`), `0.75` = `0.0 dB` (gain linéaire `1.0`), `1.0` = `+12.0 dB` (gain linéaire `3.98107`).
* **Découverte Automatique de l'IP de la MOTU (sans saisie manuelle) :**
  1. Détection de l'interface réseau locale configurée en `169.254.0.0/16` (ex. `eth1` ou `enx...`) via `if-addrs`.
  2. Écoute des annonces mDNS/Bonjour sur cette interface via `mdns-sd`.
  3. **Fallback infaillible :** Lecture de la table des voisins ARP (`/proc/net/arp` ou `ip neigh`) sur l'interface USB et sonde TCP rapide sur le port **`1280`** pour chaque voisin `169.254.x.x`.
* **Synchronisation & Mémoire Interne :**
  * La MOTU sauvegarde ses réglages dans sa mémoire flash interne ~2 à 5 secondes après chaque modification.
  * **Politique au branchement (Option A - Matériel prioritaire) :** Lors de la connexion USB, le serveur Rust interroge l'état complet de la MOTU et synchronise l'interface Web et les contrôleurs OSC sur l'état matériel réel.
  * **Mode Offline / Démo automatique :** Si la MOTU est éteinte ou débranchée, le serveur bascule sans erreur sur un état simulé en mémoire permettant de tester l'UI, l'OSC et de préparer des présets.
* **Lissage des Commandes (Throttling) :** Limiteur de débit par paramètre (`25 ms` / max `40 msg/s` vers la MOTU) avec envoi garanti de la dernière valeur finale afin de ne jamais saturer le bus USB lors des mouvements rapides de faders.

---

## 3. Spécifications OSC, Web & Présets

### 3.1 Table d'Adressage OSC (Port UDP `8000` par défaut)
Toutes les commandes sont bidirectionnelles (le serveur renvoie le feedback OSC vers les clients enregistrés lors d'un mouvement matériel ou Web) :

| Adresse OSC | Type Argument | Plage / Valeur | Description |
| :--- | :--- | :--- | :--- |
| `/mix/{bus}/ch/{ch}/vol` | `float` | `0.0` à `1.0` | Volume du canal `ch` dans le bus `bus` (`0.75` = `0 dB`) |
| `/mix/{bus}/ch/{ch}/pan` | `float` | `-1.0` à `1.0` | Panoramique stéréo (`0.0` = centre) |
| `/mix/{bus}/ch/{ch}/mute` | `int` / `float` | `0` ou `1` | Mute du canal dans le bus |
| `/mix/{bus}/ch/{ch}/solo` | `int` / `float` | `0` ou `1` | Solo du canal dans le bus |
| `/mix/main/vol` | `float` | `0.0` à `1.0` | Volume général de sortie principale (*Main Out*) |
| `/mix/main/power` | `int` / `float` | `0` ou `1` | Marche/arrêt de Monitor : fondu serveur, mémorise le niveau ; commande aussi la prise enceintes si elle est configurée (feedback `0/1` = où va le canal) |
| `/controlroom/phones/power` | `int` / `float` | `0` ou `1` | Marche/arrêt du casque (fondu serveur, sans prise) |
| `/preamp/{ch}/gain` | `float` | `0.0` à `60.0` | Gain du préampli micro/ligne en dB |
| `/preamp/{ch}/48v` | `int` | `0` ou `1` | Alimentation fantôme +48V |
| `/preamp/{ch}/pad` | `int` | `0` ou `1` | Atténuateur Pad -20 dB |
| `/preamp/{ch}/phase` | `int` | `0` ou `1` | Inversion de polarité (Phase) |
| `/eq/{ch}/bypass` | `int` | `0` ou `1` | Bypass global des 4 bandes d'EQ du canal |
| `/routing/in/{in}/out/{out}` | `int` | `0` ou `1` | Activation d'un point de croisement matrice |
| `/controlroom/dim` | `int` | `0` ou `1` | Atténuation rapide Control Room (-20 dB) |
| `/controlroom/mono` | `int` | `0` ou `1` | Sommation Mono de contrôle |
| `/controlroom/ab` | `int` | `0` ou `1` | Bascule enceintes A / B |
| `/controlroom/talkback` | `int` | `0` ou `1` | Activation du micro Talkback |
| `/preset/load` | `string` | `"nom_preset"` | Charge instantanément un snapshot JSON |

### 3.2 Design UI/UX (Dark Theme & Améliorations Communautaires)
* **Charte graphique sombre anti-fatigue :**
  * Fond principal : Anthracite profond `#121214` / `#16181D`.
  * Tranches de console & cartes : `#1E2128` avec bordures subtiles `#2C303A`.
  * Couleurs fonctionnelles strictes : Rouge `#EF4444` (*Mute*, *+48V*), Ambre `#F59E0B` (*Solo*, *DIM*), Cyan `#06B6D4` (*EQ*, *Comp*, *Gate* actifs), VU-mètres dégradés Vert `#10B981` → Jaune `#EAB308` → Rouge `#EF4444`.
* **Améliorations clés par rapport à CueMix 5 :**
  1. **Bouton `EQ Bypass` global** en un clic sur chaque tranche.
  2. **Bandeau supérieur permanent** affichant le nom du **préset actif** avec un indicateur visuel `● Modifié` dès qu'un paramètre diverge du snapshot chargé.
  3. **Barre Control Room dédiée** toujours accessible (`DIM`, `MONO`, `A/B`, `Talkback`, volume casque).
  4. **Personnalisation locale (`labels.json`) :** Renommage libre des canaux et attribution d'un code couleur par tranche, persisté sur le serveur.
  5. **Ergonomie studio & tactile :** Double-clic sur tout fader/encodeur pour remise à `0.0 dB` (`0.75`), touche `Shift` pour réglage fin, grille responsive adaptée aussi bien aux écrans 27" qu'aux tablettes (iPad) en régie.

---

## 4. Matrice de Parité CueMix 5

| Module / Fonctionnalité | CueMix 5 Officiel | `cuemix-web` | Notes d'implémentation |
| :--- | :---: | :---: | :--- |
| Préamplis (Gain, +48V, Pad, Phase) | ✅ | ✅ | Contrôle binaire direct + retour d'état |
| 7 Mixages Stéréo indépendants | ✅ | ✅ | Faders log `0..1`, Pan, Mute, Solo |
| EQ Paramétrique 4 bandes + HPF | ✅ | ✅ + **Bypass Global** | Courbe graphique interactive en Svelte |
| Compresseur & Gate par canal | ✅ | ✅ | Threshold, Ratio, Attack, Release, Gain |
| Reverb intégrée & départs d'effets | ✅ | ✅ | Contrôle complet des sends & paramètres |
| Matrice de Routage complète | ✅ | ✅ | Grille interactive + commandes OSC |
| VU-mètres temps réel (60 FPS) | ✅ | ✅ | Flux binaire MOTU diffusé via WS JSON |
| Réglages Horloge & Sample Rate | ✅ | ✅ | 44.1 kHz à 192 kHz, source d'horloge |
| Gestion de Présets / Snapshots | ✅ (local OS) | ✅ (Serveur JSON) | Rappelable par Web et OSC |
| **Accès Multi-Clients Web & Mobile** | ❌ | ✅ **(Exclusif)** | Compatible reverse proxy HAProxy/ACME |
| **Contrôle OSC Bidirectionnel** | ❌ | ✅ **(Exclusif)** | Port UDP `8000` configurable |

---

## 5. Sécurité, Confidentialité & Gestion des Secrets

**Règle absolue :** Aucune information spécifique au réseau privé du mainteneur (ex. IP `192.0.2.44`, noms de domaines internes, certificats, identifiants Proxmox) ne doit être présente dans les fichiers suivis par Git.

### 5.1 Fichiers Modèles vs Fichiers Locaux Ignorés
* **Fichiers versionnés (neutres, valeurs génériques RFC 5737 / `192.0.2.x` ou `10.0.0.x`) :**
  * `config.example.toml`
  * `deploy/proxmox.env.example`
  * `deploy/linux-setup.env.example`
* **Fichiers strictement inscrits dans `.gitignore` :**
  * `config.toml` et `*.local.toml`
  * `deploy/*.env` (sauf `*.env.example`)
  * `data/presets/*.json`, `data/labels.json`, `data/power.json` et `data/speaker-plug.json`
  * `.env`, `*.pem`, `*.key`
* **Audit CI Anti-Fuite :** Un job GitHub Actions (`gitleaks` + vérification par expressions régulières) bloque automatiquement toute Pull Request contenant des IP privées réelles, clés ou secrets.

---

## 6. Déploiement Multi-Plateforme (Proxmox, SBC ARM, Linux & Docker)

### 6.1 Déploiement 1 : Proxmox VE (LXC) — `deploy/proxmox-setup.sh`
* **Cible :** Proxmox VE 7.4+ (`x86_64`).
* **Fonctionnement interactif & résilient :**
  1. Charge `deploy/proxmox.env` s'il existe, sinon demande interactivement à l'utilisateur : `CTID` (ex. `160`), `IP/CIDR` du LAN (ex. `192.0.2.50/24`), passerelle, pont réseau principal (détecté auto, ex. `vmbr0`), stockage (ex. `local-lvm`).
  2. **Réseau USB MOTU isolé :** Vérifie les ponts existants sur l'hôte (évite d'écraser `vmbr1` s'il est déjà utilisé) et crée un pont dédié sans passerelle (ex. `vmbr2`).
  3. **Règle `udev` à chaud sur l'hôte Proxmox (`/etc/udev/rules.d/99-motu-mk5.rules`) :** Détecte l'interface `CDC Ethernet` de la MOTU et exécute impérativement dans cet ordre :
     ```bash
     ip link set $INTERFACE up && brctl addif vmbr2 $INTERFACE
     ```
     **Rattachement après rechargement réseau :** `ifreload -a` (ifupdown2, aussi utilisé par « Apply Configuration » dans l'interface Proxmox) retire du pont tout port non déclaré dans `bridge-ports` (seuls `tap`/`veth`/`fwpr` sont épargnés par Proxmox). Le port MOTU (`enx…`) ne peut pas y être déclaré (le pont échouerait MOTU débranchée) : le bloc du pont dans `/etc/network/interfaces.d/cuemix-motu.cfg` porte donc un `post-up` qui exécute `/usr/local/sbin/cuemix-motu-bridge --all` (recherche la MOTU par vendeur USB `07fd` dans sysfs, sans effet si elle est absente). `proxmox-setup.sh --update` réinstalle aussi ce hook côté hôte, sans recharger le réseau.
     **NetworkManager et surveillance :** si l'hôte exécute NetworkManager, celui-ci prend le lien MOTU en charge (« Wired connection 1 »), tente un DHCP, échoue et se réactive, ce qui retire le lien du pont. Une règle `udev` supplémentaire (`NM_UNMANAGED=1` pour vendeur `07fd` / pilote `cdc_ether`) et `nmcli device set … managed no` (dans `cuemix-motu-bridge --all`, jamais dans le chemin `udev`) lui font ignorer le lien. Un minuteur `systemd` (`cuemix-motu-bridge.timer`, toutes les 30 s) relance `cuemix-motu-bridge --all` ; les rattachements sont visibles par `journalctl -u cuemix-motu-bridge`. Ces fichiers côté hôte arrivent avec `sudo deploy/proxmox-setup.sh --update` (le programme de mise à jour intégré ne met à jour que le conteneur).
     **Dépannage :** symptôme « Demo — no MOTU » alors que la MOTU est branchée → sur l'hôte, `bridge link show` doit lister l'interface `enx…` avec `master vmbr2`. Sinon, correctif manuel : `/usr/local/sbin/cuemix-motu-bridge --all` (ou `/usr/local/sbin/cuemix-motu-bridge enx…`).
  4. **Provisionnement LXC Debian 12 :** Crée le conteneur avec `--unprivileged 1 --features nesting=1`, `eth0` sur le pont LAN et `eth1` sur `vmbr2` avec l'IP statique **`169.254.254.1/16`** (garantissant l'absence de conflit lien-local avec la MOTU).
  5. **Installation du binaire :** Télécharge la dernière release `x86_64-unknown-linux-musl` depuis GitHub Releases (avec fallback automatique vers une compilation temporaire depuis `main` si aucune release n'existe encore), crée l'utilisateur non-root `cuemix`, configure `avahi-daemon` (`cuemix-web.local`) et active le service `systemd`.
  6. **Compatibilité Reverse Proxy (HAProxy / pfSense + ACME) :** Écoute sur `0.0.0.0:8080`, supporte l'upgrade WebSocket `/ws` et les en-têtes `X-Forwarded-For` / `X-Forwarded-Proto`.

### 6.2 Déploiement 2 : Cartes SBC ARM & Distributions Linux — `deploy/linux-setup.sh`
* **Cibles supportées :**
  * Raspberry Pi (Zero 2 W, Pi 3, Pi 4, Pi 5) sous Raspberry Pi OS (64-bit `aarch64` et 32-bit `armv7`).
  * Orange Pi, Radxa Rock, Banana Pi sous Armbian / Debian.
  * Mini-PC Intel N100 & Stations Linux Studio (Ubuntu Studio, Debian, Pop!_OS, Arch, SteamOS).
* **Fonctionnement :**
  1. Détecte automatiquement l'architecture (`uname -m` → `x86_64` ou `aarch64`) et installe le binaire statique `musl` correspondant (ou le paquet `.deb` sur systèmes Debian/Ubuntu).
  2. Configure la règle `udev` locale attribuant automatiquement `169.254.254.1/16` à l'interface USB-C de la MOTU dès le branchement.
  3. Configure `avahi-daemon` et le service `systemd` sous utilisateur non-root `cuemix`.

### 6.3 Déploiement 3 : Conteneur Docker / Podman (`ghcr.io`)
* **Image multi-architecture :** `ghcr.io/lejeanf/cuemix-web:latest` (`linux/amd64` et `linux/arm64`).
* **Configuration :** Fourniture d'un `deploy/docker-compose.yml` utilisant `network_mode: host` (requis pour l'accès direct au sous-réseau lien-local `169.254.0.0/16` et la diffusion mDNS) et montant les volumes `/etc/cuemix-web` et `/var/lib/cuemix-web`.

---

## 7. Structure du Dépôt & Chaîne de Compilation

```text
cuemix-web/
├── .github/
│   └── workflows/
│       ├── ci.yml              # fmt, clippy, tests, audit, gitleaks, build Svelte+Rust
│       └── release.yml         # Cross-compile musl (x86_64/aarch64), .deb, Docker ghcr.io
├── deploy/
│   ├── proxmox-setup.sh        # Script interactif Proxmox VE LXC + udev
│   ├── proxmox.env.example     # Modèle de variables Proxmox (sans secrets)
│   ├── linux-setup.sh          # Script universel Raspberry Pi / SBC / Linux Desktop
│   ├── Dockerfile              # Build multi-stage amd64/arm64
│   └── docker-compose.yml      # Déploiement conteneurisé host-network
├── docs/
│   └── ARCHITECTURE.md         # Document d'architecture vivant (source de vérité)
├── frontend/                   # Application Svelte 5 + Vite + TypeScript (Dark Theme)
├── src/
│   ├── main.rs                 # Point d'entrée, orchestration Tokio & arrêt propre
│   ├── config.rs               # Chargement config.toml & variables d'environnement
│   ├── state.rs                # État centralisé, loi log faders, présets & labels
│   ├── motu/
│   │   ├── discovery.rs        # Détection IP (if-addrs, mdns-sd, ARP + sonde 1280)
│   │   ├── protocol.rs         # Encodage/décodage binaire Big-Endian & virgule fixe 8.24
│   │   └── client.rs           # Client WebSocket MOTU, reconnexion, throttling, mode démo
│   ├── osc/
│   │   └── bridge.rs           # Serveur/client UDP OSC bidirectionnel (rosc)
│   └── web/
│       └── server.rs           # Serveur Axum, WebSocket JSON (/ws), assets rust-embed
├── config.example.toml         # Modèle de configuration neutre
├── Cargo.toml                  # Dépendances Rust & métadonnées cargo-deb
├── Makefile                    # Orchestration stricte : build frontend -> test/build Rust
├── .gitignore                  # Exclusion stricte des configs locales, secrets et données
└── README.md                   # Guide complet d'utilisation, OSC et déploiement
```

---

## 8. Gouvernance Multi-Agents, Qualité & Cycle de Vie

### 8.1 Organisation de l'Équipe d'Agents
1. **`agent-supervisor` (Orchestrateur & Release Manager) :**
   * Découpe les tâches, maintient le contrat d'interface partagé (`MotuState` et schémas JSON/OSC), gère les versions **SemVer** (`MAJOR.MINOR.PATCH`) et arbitre les merges.
2. **`agent-arch-guardian` (Gardien d'Architecture & Documentation) :**
   * Vérifie avant chaque merge la conformité stricte avec `docs/ARCHITECTURE.md`.
   * S'assure qu'aucune modification structurelle n'est fusionnée sans mise à jour conjointe de `docs/ARCHITECTURE.md` et `README.md`.
3. **`agent-qa-security` (Créateur de Tests & Auditeur Sécurité) :**
   * Développe un **serveur Mock WebSocket MOTU** pour exécuter des tests d'intégration sans matériel physique.
   * Écrit les tests unitaires (conversions `8.24`, courbe log, parsing OSC, fuzzing de trames malformées).
   * Vérifie l'absence totale de `.unwrap()` / `.expect()` dans le code de production, l'exécution non-root et l'absence de fuite de secrets.
4. **Agents d'Exécution en Parallèle :**
   * `agent-motu-protocol` : `src/motu/*` et `src/state.rs`.
   * `agent-bridges` : `src/osc/*`, `src/web/*` et `src/config.rs`.
   * `agent-frontend` : `frontend/*` (Svelte Dark Theme, grille responsive, parité CueMix 5).
   * `agent-devops` : `Makefile`, `.github/workflows/*`, `deploy/*` (Proxmox, Raspberry Pi, Linux, Docker).

### 8.2 Protocole de Pondération & Escalade Humaine
* **Règle de pondération :** Si un agent d'exécution détecte une contrainte technique imposant de modifier l'architecture définie dans `docs/ARCHITECTURE.md`, il doit soumettre une proposition argumentée. `agent-arch-guardian`, `agent-qa-security` et `agent-supervisor` doivent tous trois valider la modification par consensus.
* **Escalade humaine obligatoire :** En cas de désaccord entre les agents, d'ambiguïté sur un comportement matériel de la MOTU non documenté, ou de changement impactant la sécurité réseau, `agent-supervisor` crée une **GitHub Issue bloquante** étiquetée `needs-human-review` et suspend la branche concernée jusqu'à la décision du mainteneur (`@lejeanf`).

### 8.3 Feuille de Route d'Exécution
* **Phase 1 (Fondations & Contrats) :** Initialisation du dépôt, `.gitignore`, `config.example.toml`, `Makefile`, `docs/ARCHITECTURE.md`, structures de données `state.rs` et serveur Mock MOTU.
* **Phase 2 (Moteur Binaire & Passerelles en parallèle) :** Implémentation de `motu/*` (discovery, 8.24, throttling, mode démo), `osc/bridge.rs` et `web/server.rs`.
* **Phase 3 (Interface Svelte Dark Theme) :** Console de mixage, préamplis, EQ 4 bandes + Bypass global, Compresseur/Gate, Matrice de routage, VU-mètres 60 FPS, Control Room et gestionnaire de présets.
* **Phase 4 (Déploiement Multi-Plateforme & Release `v1.0.0`) :** Finalisation et tests de `proxmox-setup.sh`, `linux-setup.sh`, `Dockerfile`, CI/CD GitHub Actions et rédaction finale du `README.md`.
