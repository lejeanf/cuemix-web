#!/usr/bin/env bash
# cuemix-web: Proxmox VE host setup (see docs/HANDOFF.md section 6.1).
#
# Creates an unprivileged Debian 12 LXC container that runs cuemix-web, with:
#   - eth0 on the LAN bridge (static address, gateway),
#   - eth1 on an isolated MOTU bridge (no IP, no gateway) with the container
#     side at 169.254.254.1/16,
# and installs the udev rule that moves the MOTU CDC-Ethernet link onto that
# bridge when the interface appears.
#
# Run as root on the Proxmox VE host, from a checkout of this repository.
# Idempotent: re-running keeps existing bridges, rules, containers and config.
#
# Usage: sudo deploy/proxmox-setup.sh [--dry-run] [--help]

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
ENV_FILE="${SCRIPT_DIR}/proxmox.env"
UDEV_TEMPLATE="${SCRIPT_DIR}/99-motu-mk5.rules.template"
UNIT_FILE="${SCRIPT_DIR}/cuemix-web.service"
CONFIG_EXAMPLE="${REPO_DIR}/config.example.toml"
UPDATE_SCRIPT="${SCRIPT_DIR}/cuemix-update.sh"

HOST_HELPER="/usr/local/sbin/cuemix-motu-bridge"
UDEV_RULE="/etc/udev/rules.d/99-motu-mk5.rules"
MOTU_BRIDGE_CFG="/etc/network/interfaces.d/cuemix-motu.cfg"

TEMPLATE_STORAGE="${TEMPLATE_STORAGE:-local}"
ROOTFS_GB="${ROOTFS_GB:-4}"

DRY_RUN=0
WORKDIR=""
MODE="provision"
CTID_ARG=""
UPDATE_ARGS=()

# ---------------------------------------------------------------------------
# Logging (colors only on an interactive terminal, honours NO_COLOR)
# ---------------------------------------------------------------------------
if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  C_RED=$'\033[31m'; C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'
  C_BOLD=$'\033[1m'; C_RESET=$'\033[0m'
else
  C_RED=""; C_GREEN=""; C_YELLOW=""; C_BOLD=""; C_RESET=""
fi

log_info() { printf '%s[info]%s  %s\n' "${C_GREEN}" "${C_RESET}" "$*"; }
log_warn() { printf '%s[warn]%s  %s\n' "${C_YELLOW}" "${C_RESET}" "$*" >&2; }
log_err()  { printf '%s[error]%s %s\n' "${C_RED}" "${C_RESET}" "$*" >&2; }
log_step() { printf '\n%s==> %s%s\n' "${C_BOLD}" "$*" "${C_RESET}"; }
die()      { log_err "$*"; exit 1; }

# run CMD... : execute, or only print the command under --dry-run.
run() {
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    printf '%s[dry-run]%s %s\n' "${C_YELLOW}" "${C_RESET}" "$*"
    return 0
  fi
  "$@"
}

cleanup() {
  # Capture the exit status first and re-exit with it: bash otherwise lets
  # the last trap command decide the script's exit status.
  local rc=$?
  if [[ -n "${WORKDIR}" && -d "${WORKDIR}" ]]; then
    rm -rf -- "${WORKDIR}"
  fi
  exit "${rc}"
}
trap cleanup EXIT

usage() {
  cat <<'USAGE'
cuemix-web Proxmox VE setup

Creates an unprivileged Debian 12 LXC container running cuemix-web, an
isolated MOTU bridge (no gateway), and the udev rule that attaches the
MOTU USB CDC-Ethernet link to that bridge.

Usage:
  sudo deploy/proxmox-setup.sh [--dry-run] [--help]
  sudo deploy/proxmox-setup.sh --update [--pre | --version vX.Y.Z[-pre]]
                               [--ctid N] [--dry-run]

Options:
  -n, --dry-run     Print every action without changing the system.
  -h, --help        Show this help and exit.
  --update          Update cuemix-web inside the existing container: pushes the
                    current cuemix-update.sh and runs it there. Does not touch
                    bridges, udev rules or the container configuration.
  --pre             With --update: also consider pre-releases.
  --version vX.Y.Z  With --update: install this exact release tag.
  --ctid N          With --update: container ID (default: CTID in proxmox.env).

Settings are read from deploy/proxmox.env when it exists (copy
deploy/proxmox.env.example and edit it). Otherwise they are asked for.

Environment:
  TEMPLATE_STORAGE  Storage holding CT templates (default: local)
  ROOTFS_GB         Container root disk size in GiB (default: 4)
  NO_COLOR          Disable colored output

Requirements: root, Proxmox VE 7.4+ (pveversion), pct, pveam, pvesm,
ifreload (ifupdown2), udevadm, and network access to GitHub.
USAGE
}

# ---------------------------------------------------------------------------
# Argument and environment checks
# ---------------------------------------------------------------------------
parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -h|--help) usage; exit 0 ;;
      -n|--dry-run) DRY_RUN=1 ;;
      --update) MODE="update" ;;
      --pre) UPDATE_ARGS+=(--pre) ;;
      --version)
        [[ $# -ge 2 ]] || die "--version requires a tag, e.g. --version v0.2.0"
        UPDATE_ARGS+=(--version "$2")
        shift
        ;;
      --ctid)
        [[ $# -ge 2 ]] || die "--ctid requires a container ID"
        CTID_ARG="$2"
        shift
        ;;
      *) log_err "unknown option: $1"; usage >&2; exit 2 ;;
    esac
    shift
  done
  if [[ "${MODE}" != "update" ]]; then
    if [[ "${#UPDATE_ARGS[@]}" -gt 0 || -n "${CTID_ARG}" ]]; then
      die "--pre, --version and --ctid only apply together with --update"
    fi
  fi
}

require_root() {
  if [[ "$(id -u)" -ne 0 ]]; then
    die "this script must run as root on the Proxmox VE host (try: sudo $0)"
  fi
}

preflight() {
  log_step "Preflight"
  command -v pveversion >/dev/null 2>&1 \
    || die "pveversion not found: this is not a Proxmox VE host"
  local ver major
  ver="$(pveversion 2>/dev/null | head -n 1 || true)"
  major="$(printf '%s\n' "${ver}" | sed -n 's#^pve-manager/\([0-9][0-9]*\)\..*#\1#p')"
  [[ -n "${major}" && "${major}" -ge 7 ]] \
    || die "Proxmox VE 7.4 or newer is required (found: ${ver:-unknown})"
  log_info "Proxmox VE: ${ver}"

  local cmd
  for cmd in pct pveam pvesm ip udevadm ifreload; do
    command -v "${cmd}" >/dev/null 2>&1 || die "required command not found: ${cmd}"
  done

  [[ -f "${UDEV_TEMPLATE}" ]] || die "missing ${UDEV_TEMPLATE} (run from a repository checkout)"
  [[ -f "${UNIT_FILE}" ]] || die "missing ${UNIT_FILE} (run from a repository checkout)"
  [[ -f "${CONFIG_EXAMPLE}" ]] || die "missing ${CONFIG_EXAMPLE} (run from a repository checkout)"
  [[ -f "${UPDATE_SCRIPT}" ]] || die "missing ${UPDATE_SCRIPT} (run from a repository checkout)"
}

# ---------------------------------------------------------------------------
# Settings
# ---------------------------------------------------------------------------
load_config() {
  # Bash sets HOSTNAME itself; clear it so only the env file can define it.
  unset HOSTNAME
  if [[ -f "${ENV_FILE}" ]]; then
    log_info "loading ${ENV_FILE}"
    # shellcheck source=/dev/null
    source "${ENV_FILE}"
  else
    log_info "no ${ENV_FILE}; settings will be asked interactively"
  fi
  CT_HOSTNAME="${HOSTNAME:-}"
  MOTU_IP="${MOTU_IP:-}"
}

# ask VARNAME "Question" "default" : keep a value already set, else prompt.
ask() {
  local name="$1" question="$2" default="${3:-}" value=""
  if [[ -n "${!name:-}" ]]; then
    return 0
  fi
  if [[ ! -t 0 ]]; then
    [[ -n "${default}" ]] || die "${name} is not set and no terminal is available; set it in ${ENV_FILE}"
    value="${default}"
  elif [[ -n "${default}" ]]; then
    read -r -p "${question} [${default}]: " value || true
    value="${value:-${default}}"
  else
    read -r -p "${question}: " value || true
  fi
  [[ -n "${value}" ]] || die "${name} is required"
  printf -v "${name}" '%s' "${value}"
}

detect_lan_bridge() {
  local dev=""
  dev="$(ip -o route show default 2>/dev/null | awk '{print $5; exit}' || true)"
  if [[ -n "${dev}" && -d "/sys/class/net/${dev}/bridge" ]]; then
    printf '%s' "${dev}"
  else
    printf 'vmbr0'
  fi
}

collect_settings() {
  log_step "Settings"
  local detected_bridge
  detected_bridge="$(detect_lan_bridge)"

  ask CTID "Container ID (CTID)" "160"
  ask CT_HOSTNAME "Container hostname" "cuemix-web"
  ask WAN_IP "LAN address in IP/CIDR (example: 192.0.2.50/24)" ""
  ask WAN_GW "LAN gateway (example: 192.0.2.1)" ""
  ask WAN_BRIDGE "LAN bridge" "${detected_bridge}"
  ask STORAGE "Storage for the container root disk" "local-lvm"
  ask MOTU_BRIDGE "Isolated MOTU bridge (no gateway)" "vmbr2"
  MOTU_IP="${MOTU_IP:-169.254.254.1/16}"

  # CTID is assigned by `ask` above (printf -v).
  # shellcheck disable=SC2153
  if ! [[ "${CTID}" =~ ^[0-9]+$ ]] || (( CTID < 100 )); then
    die "CTID must be an integer >= 100"
  fi
  [[ "${CT_HOSTNAME}" =~ ^[A-Za-z0-9-]{1,63}$ ]] || die "invalid hostname: ${CT_HOSTNAME}"
  [[ "${WAN_IP}" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}/([0-9]|[12][0-9]|3[0-2])$ ]] \
    || die "WAN_IP must be IP/CIDR, e.g. 192.0.2.50/24"
  [[ "${WAN_GW}" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] || die "WAN_GW must be an IPv4 address"
  [[ "${WAN_BRIDGE}" =~ ^[A-Za-z0-9_.-]{1,15}$ ]] || die "invalid LAN bridge name: ${WAN_BRIDGE}"
  [[ "${MOTU_BRIDGE}" =~ ^[A-Za-z0-9_.-]{1,15}$ ]] || die "invalid MOTU bridge name: ${MOTU_BRIDGE}"
  [[ "${STORAGE}" =~ ^[A-Za-z0-9_.-]+$ ]] || die "invalid storage name: ${STORAGE}"
  [[ "${MOTU_BRIDGE}" != "${WAN_BRIDGE}" ]] || die "the MOTU bridge must differ from the LAN bridge"
  [[ "${MOTU_IP}" =~ ^169\.254\.[0-9]{1,3}\.[0-9]{1,3}/16$ ]] \
    || die "MOTU_IP must be a link-local 169.254.x.x/16 address"
  [[ -d "/sys/class/net/${WAN_BRIDGE}/bridge" ]] \
    || die "LAN bridge ${WAN_BRIDGE} does not exist or is not a Linux bridge"
  pvesm status | awk -v s="${STORAGE}" 'NR>1 && $1==s {found=1} END {exit !found}' \
    || die "storage ${STORAGE} not found (see: pvesm status)"

  log_info "CTID=${CTID} hostname=${CT_HOSTNAME} lan=${WAN_IP} gw=${WAN_GW} bridge=${WAN_BRIDGE}"
  log_info "storage=${STORAGE} motu_bridge=${MOTU_BRIDGE} motu_ip=${MOTU_IP}"
}

# ---------------------------------------------------------------------------
# Host: isolated MOTU bridge
# ---------------------------------------------------------------------------
# interfaces_sources_cfg : true when /etc/network/interfaces loads our bridge
# file, through `source <dir>/*`, `source <our file>` or `source-directory`.
interfaces_sources_cfg() {
  grep -qsE '^[[:space:]]*(source[[:space:]]+/etc/network/interfaces\.d/(\*|cuemix-motu\.cfg)|source-directory[[:space:]]+/etc/network/interfaces\.d/?)[[:space:]]*$' \
    /etc/network/interfaces
}

# ensure_interfaces_source : without this, ifreload never sees the bridge
# file, the bridge is not created and the container cannot start.
# Only our own file is sourced, so no other file in interfaces.d is activated.
ensure_interfaces_source() {
  local line="source ${MOTU_BRIDGE_CFG}" backup answer=""
  interfaces_sources_cfg && return 0

  log_warn "/etc/network/interfaces does not load ${MOTU_BRIDGE_CFG}"
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    printf '%s[dry-run]%s %s\n' "${C_YELLOW}" "${C_RESET}" \
      "would ask to append '${line}' to /etc/network/interfaces (with a backup)"
    return 0
  fi
  if [[ ! -t 0 ]]; then
    die "add this line to /etc/network/interfaces, then re-run: ${line}"
  fi
  read -r -p "Append '${line}' to /etc/network/interfaces (a backup is kept)? [Y/n]: " answer || true
  case "${answer:-y}" in
    [Yy]|[Yy][Ee][Ss]) ;;
    *) die "not changed; add this line to /etc/network/interfaces, then re-run: ${line}" ;;
  esac

  backup="/etc/network/interfaces.bak-cuemix-$(date +%Y%m%d%H%M%S)"
  cp -p -- /etc/network/interfaces "${backup}"
  printf '\n# Added by cuemix-web deploy/proxmox-setup.sh (isolated MOTU bridge).\n%s\n' \
    "${line}" >> /etc/network/interfaces
  log_info "appended '${line}' to /etc/network/interfaces (backup: ${backup})"
}

ensure_motu_bridge() {
  log_step "Isolated MOTU bridge ${MOTU_BRIDGE}"
  local br="${MOTU_BRIDGE}" cfg_tmp port name drv users defs needs_reload=0

  if [[ -d "/sys/class/net/${br}" ]]; then
    [[ -d "/sys/class/net/${br}/bridge" ]] \
      || die "${br} exists but is not a Linux bridge; choose another MOTU bridge"

    if [[ -n "$(ip -o -4 addr show dev "${br}" 2>/dev/null || true)" ]]; then
      die "${br} already has an IPv4 address; refusing to reuse it (the MOTU bridge must have no IP)"
    fi

    for port in /sys/class/net/"${br}"/brif/*; do
      [[ -e "${port}" ]] || continue
      name="$(basename -- "${port}")"
      # Our own container's tap devices are expected; anything else is not.
      if [[ "${name}" == "tap${CTID}i"* ]]; then
        continue
      fi
      drv="$(basename -- "$(readlink -f "/sys/class/net/${name}/device/driver" 2>/dev/null || echo none)")"
      [[ "${drv}" == "cdc_ether" ]] \
        || die "${br} has port ${name} (driver ${drv}) that is in use for something else; refusing to reuse it"
    done

    users="$(grep -l -E "bridge=${br}([,[:space:]]|$)" /etc/pve/lxc/*.conf /etc/pve/qemu-server/*.conf 2>/dev/null || true)"
    users="$(printf '%s\n' "${users}" | grep -v -E "/${CTID}\.conf$" | grep -v '^$' || true)"
    [[ -z "${users}" ]] \
      || die "${br} is used by other guests (${users//$'\n'/ }); refusing to reuse it"

    defs="$(grep -l -E "^[[:space:]]*(auto|iface)[[:space:]]+${br}([[:space:]]|$)" \
      /etc/network/interfaces /etc/network/interfaces.d/* 2>/dev/null || true)"
    defs="$(printf '%s\n' "${defs}" | grep -v -F "${MOTU_BRIDGE_CFG}" | grep -v '^$' || true)"
    [[ -z "${defs}" ]] \
      || die "${br} is defined in another network config (${defs//$'\n'/ }); refusing to reuse it"

    log_info "${br} exists, has no IP and is not used by other guests; reusing it"
  else
    log_info "creating ${br} (bridge-ports none, no IP, no gateway)"
    needs_reload=1
  fi

  cfg_tmp="${WORKDIR}/motu-bridge.cfg"
  cat > "${cfg_tmp}" <<EOF
# Managed by cuemix-web deploy/proxmox-setup.sh.
# Isolated MOTU bridge: no IP address, no gateway. The MOTU USB link is
# attached by the udev rule ${UDEV_RULE}.
auto ${br}
iface ${br} inet manual
    bridge-ports none
    bridge-stp off
    bridge-fd 0
EOF

  if [[ -f "${MOTU_BRIDGE_CFG}" ]] && cmp -s "${cfg_tmp}" "${MOTU_BRIDGE_CFG}"; then
    log_info "unchanged: ${MOTU_BRIDGE_CFG}"
  else
    run mkdir -p -- "$(dirname -- "${MOTU_BRIDGE_CFG}")"
    run install -m 0644 -- "${cfg_tmp}" "${MOTU_BRIDGE_CFG}"
    log_info "wrote: ${MOTU_BRIDGE_CFG}"
    needs_reload=1
  fi

  ensure_interfaces_source

  if [[ "${needs_reload}" -eq 1 ]]; then
    run ifreload -a
  fi

  # The container's eth1 needs this bridge: fail here, not at `pct start`.
  if [[ "${DRY_RUN}" -eq 0 && ! -d "/sys/class/net/${br}/bridge" ]]; then
    die "${br} was not created by 'ifreload -a'; check that /etc/network/interfaces loads ${MOTU_BRIDGE_CFG} (ifquery ${br})"
  fi
}

# ---------------------------------------------------------------------------
# Host: udev rule and helper
# ---------------------------------------------------------------------------
# install_if_changed SRC DST MODE : returns 0 when DST was (re)written.
install_if_changed() {
  local src="$1" dst="$2" mode="$3"
  if [[ -f "${dst}" ]] && cmp -s "${src}" "${dst}"; then
    log_info "unchanged: ${dst}"
    return 1
  fi
  run install -m "${mode}" -- "${src}" "${dst}"
  log_info "installed: ${dst}"
  return 0
}

install_host_udev() {
  log_step "MOTU udev rule and helper (host)"
  local helper_src="${WORKDIR}/motu-bridge-helper" rule_src="${WORKDIR}/99-motu-mk5.rules"
  local changed=0

  # Order matters: bring the link up first, then enslave it to the bridge.
  cat > "${helper_src}" <<'HELPER'
#!/bin/sh
# Installed by cuemix-web deploy/proxmox-setup.sh. Called by udev with the
# MOTU CDC-Ethernet interface name. Order: link up, then add to the bridge.
set -eu
IF="$1"
BRIDGE="@BRIDGE@"
ip link set "$IF" up
ip link set "$IF" master "$BRIDGE"
HELPER
  sed -i "s|@BRIDGE@|${MOTU_BRIDGE}|" "${helper_src}"

  sed "s|@HELPER@|${HOST_HELPER}|" "${UDEV_TEMPLATE}" > "${rule_src}"

  install_if_changed "${helper_src}" "${HOST_HELPER}" 0755 && changed=1 || true
  install_if_changed "${rule_src}" "${UDEV_RULE}" 0644 && changed=1 || true

  if [[ "${changed}" -eq 1 ]]; then
    run udevadm control --reload-rules
    run udevadm trigger --subsystem-match=net --action=add
  fi
}

# ---------------------------------------------------------------------------
# Container template and creation
# ---------------------------------------------------------------------------
TEMPLATE_VOLID=""

ensure_template() {
  log_step "Debian 12 container template"
  local found
  found="$(pveam list "${TEMPLATE_STORAGE}" 2>/dev/null \
    | awk '{print $1}' \
    | grep -E 'vztmpl/debian-12-standard_[^ ]*_amd64\.tar\.' \
    | sort -V | tail -n 1 || true)"
  if [[ -n "${found}" ]]; then
    TEMPLATE_VOLID="${found}"
    log_info "template present: ${TEMPLATE_VOLID}"
    return 0
  fi

  log_info "template missing on ${TEMPLATE_STORAGE}; downloading"
  run pveam update || log_warn "pveam update failed; using the cached template list"
  local name=""
  name="$(pveam available --section system 2>/dev/null \
    | awk '{print $2}' \
    | grep -E '^debian-12-standard_[^ ]*_amd64\.tar\.' \
    | sort -V | tail -n 1 || true)"
  if [[ -z "${name}" ]]; then
    [[ "${DRY_RUN}" -eq 1 ]] || die "no Debian 12 standard template available through pveam"
    name="debian-12-standard_<version>_amd64.tar.zst"
  fi
  run pveam download "${TEMPLATE_STORAGE}" "${name}"
  TEMPLATE_VOLID="${TEMPLATE_STORAGE}:vztmpl/${name}"
  log_info "template: ${TEMPLATE_VOLID}"
}

create_container() {
  log_step "LXC container ${CTID}"
  if pct config "${CTID}" >/dev/null 2>&1; then
    log_info "container ${CTID} already exists; reusing it (network settings unchanged)"
    return 0
  fi
  run pct create "${CTID}" "${TEMPLATE_VOLID}" \
    --hostname "${CT_HOSTNAME}" \
    --unprivileged 1 \
    --features nesting=1 \
    --ostype debian \
    --arch amd64 \
    --cores 2 \
    --memory 1024 \
    --swap 0 \
    --rootfs "${STORAGE}:${ROOTFS_GB}" \
    --net0 "name=eth0,bridge=${WAN_BRIDGE},ip=${WAN_IP},gw=${WAN_GW}" \
    --net1 "name=eth1,bridge=${MOTU_BRIDGE},ip=${MOTU_IP}" \
    --onboot 1 \
    --start 0 \
    --description "cuemix-web (MOTU UltraLite-mk5 controller)"
  log_info "container ${CTID} created"
}

start_container() {
  log_step "Start container ${CTID}"
  if [[ "$(pct status "${CTID}" 2>/dev/null || true)" != "status: running" ]]; then
    run pct start "${CTID}"
  else
    log_info "container ${CTID} already running"
  fi

  if [[ "${DRY_RUN}" -eq 1 ]]; then
    run pct exec "${CTID}" -- sh -c 'wait for a default route (LAN gateway)'
    return 0
  fi
  for _ in $(seq 1 60); do
    if pct exec "${CTID}" -- sh -c 'ip -4 route show default | grep -q .' >/dev/null 2>&1; then
      log_info "container network is up"
      return 0
    fi
    sleep 2
  done
  die "container ${CTID} has no default route after 120 s; check net0 (${WAN_BRIDGE}, ${WAN_GW})"
}

# ---------------------------------------------------------------------------
# Container provisioning
# ---------------------------------------------------------------------------
write_provision_script() {
  cat > "$1" <<'PROVISION'
#!/usr/bin/env bash
# Runs inside the cuemix-web container (Debian 12). Pushed by proxmox-setup.sh.
set -euo pipefail

REPO_API="https://api.github.com/repos/lejeanf/cuemix-web"
RELEASE_BASE="https://github.com/lejeanf/cuemix-web/releases/download"
SOURCE_TARBALL="https://github.com/lejeanf/cuemix-web/archive/refs/heads/main.tar.gz"
ARCH_TARGET="x86_64-unknown-linux-musl"
BIN=/usr/local/bin/cuemix-web
CONF_DIR=/etc/cuemix-web
STATE_DIR=/var/lib/cuemix-web
MOTU_IFACE="${CUEMIX_MOTU_IFACE:-eth1}"
WORK="$(mktemp -d)"
trap 'rm -rf -- "${WORK}"' EXIT

log()  { printf '[ct] %s\n' "$*"; }
warn() { printf '[ct][warn] %s\n' "$*" >&2; }
die()  { printf '[ct][error] %s\n' "$*" >&2; exit 1; }

export DEBIAN_FRONTEND=noninteractive
log "installing packages (avahi-daemon, curl)"
apt-get update -qq
apt-get install -y -qq --no-install-recommends avahi-daemon curl ca-certificates

if ! id -u cuemix >/dev/null 2>&1; then
  useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin cuemix
  log "created system user cuemix"
fi

build_from_source() {
  warn "building from main inside the container: slow, needs several GB and network access"
  apt-get install -y -qq --no-install-recommends build-essential pkg-config make xz-utils
  # Official rustup-init, downloaded to a file and checked against its published sha256.
  local rustup_base="https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu"
  curl -fsSL -o "${WORK}/rustup-init" "${rustup_base}/rustup-init"
  curl -fsSL -o "${WORK}/rustup-init.sha256" "${rustup_base}/rustup-init.sha256"
  local expected
  expected="$(awk '{print $1; exit}' "${WORK}/rustup-init.sha256")"
  printf '%s  %s\n' "${expected}" "${WORK}/rustup-init" | sha256sum -c -
  sh "${WORK}/rustup-init" -y --profile minimal --default-toolchain stable
  export PATH="/root/.cargo/bin:${PATH}"

  # Node.js 22 from the official tarball, verified against SHASUMS256.txt.
  local node_index="https://nodejs.org/dist/latest-v22.x"
  local sums node_file
  sums="$(curl -fsSL "${node_index}/SHASUMS256.txt")"
  node_file="$(printf '%s\n' "${sums}" | awk '/linux-x64\.tar\.xz$/ {print $2; exit}')"
  [[ -n "${node_file}" ]] || die "could not find a Node.js 22 linux-x64 tarball"
  curl -fsSL -o "${WORK}/${node_file}" "${node_index}/${node_file}"
  printf '%s\n' "${sums}" | grep -F "  ${node_file}" > "${WORK}/node.sha256"
  (cd "${WORK}" && sha256sum -c node.sha256)
  tar -xJf "${WORK}/${node_file}" -C /opt
  ln -sfn "/opt/${node_file%.tar.xz}" /opt/node22
  export PATH="/opt/node22/bin:${PATH}"

  curl -fsSL -o "${WORK}/src.tar.gz" "${SOURCE_TARBALL}"
  tar -xzf "${WORK}/src.tar.gz" -C "${WORK}"
  (cd "${WORK}/cuemix-web-main" && make all)
  install -m 0755 "${WORK}/cuemix-web-main/target/release/cuemix-web" "${BIN}"
  log "built and installed ${BIN} from main"
}

TAG=""
if json="$(curl -fsSL -H 'Accept: application/vnd.github+json' "${REPO_API}/releases/latest" 2>/dev/null)"; then
  TAG="$(printf '%s\n' "${json}" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1 || true)"
fi

if [[ -n "${TAG}" ]]; then
  asset="cuemix-web-${TAG}-${ARCH_TARGET}.tar.gz"
  base="${RELEASE_BASE}/${TAG}"
  log "downloading release ${TAG} (${ARCH_TARGET})"
  curl -fsSL -o "${WORK}/${asset}" "${base}/${asset}"
  curl -fsSL -o "${WORK}/${asset}.sha256" "${base}/${asset}.sha256"
  (cd "${WORK}" && sha256sum -c "${asset}.sha256")
  tar -xzf "${WORK}/${asset}" -C "${WORK}"
  install -m 0755 "${WORK}/cuemix-web" "${BIN}"
  log "installed ${BIN} (${TAG})"
else
  log "no published release found; falling back to a source build"
  build_from_source
fi

install -d -m 0755 "${CONF_DIR}"
if [[ ! -e "${CONF_DIR}/config.toml" ]]; then
  install -m 0640 -o root -g cuemix /root/cuemix-config.example.toml "${CONF_DIR}/config.toml"
  sed -i "s|^motu_interface = .*|motu_interface = \"${MOTU_IFACE}\"|" "${CONF_DIR}/config.toml"
  log "wrote ${CONF_DIR}/config.toml (motu_interface = ${MOTU_IFACE})"
else
  log "keeping existing ${CONF_DIR}/config.toml"
fi

install -d -o cuemix -g cuemix -m 0750 "${STATE_DIR}"

install -m 0644 /root/cuemix-web.service /etc/systemd/system/cuemix-web.service
systemctl daemon-reload
systemctl enable avahi-daemon >/dev/null 2>&1 || warn "could not enable avahi-daemon"
systemctl restart avahi-daemon || warn "could not restart avahi-daemon (mDNS unavailable)"
systemctl enable cuemix-web
systemctl restart cuemix-web

rm -f /root/cuemix-web.service /root/cuemix-config.example.toml
log "provisioning complete"
PROVISION
}

provision_container() {
  log_step "Provision cuemix-web inside container ${CTID}"
  local script="${WORKDIR}/cuemix-provision.sh"
  write_provision_script "${script}"

  run pct push "${CTID}" "${script}" /root/cuemix-provision.sh --perms 0700
  run pct push "${CTID}" "${CONFIG_EXAMPLE}" /root/cuemix-config.example.toml --perms 0644
  run pct push "${CTID}" "${UNIT_FILE}" /root/cuemix-web.service --perms 0644
  run pct push "${CTID}" "${UPDATE_SCRIPT}" /usr/local/sbin/cuemix-update --perms 0755
  run pct exec "${CTID}" -- env CUEMIX_MOTU_IFACE=eth1 bash /root/cuemix-provision.sh
  run pct exec "${CTID}" -- rm -f /root/cuemix-provision.sh
  push_update_units "${CTID}"
}

# push_update_units CTID : install the daily release check (timer) and the
# web-requested install (path unit) in the container, and run a first check.
push_update_units() {
  local ctid="$1" u
  for u in cuemix-update-check.service cuemix-update-check.timer cuemix-update-request.path cuemix-update-request.service; do
    [[ -f "${SCRIPT_DIR}/systemd/${u}" ]] || die "missing ${SCRIPT_DIR}/systemd/${u} (run from a repository checkout)"
    run pct push "${ctid}" "${SCRIPT_DIR}/systemd/${u}" "/etc/systemd/system/${u}" --perms 0644
  done
  run pct exec "${ctid}" -- systemctl daemon-reload
  run pct exec "${ctid}" -- systemctl enable --now cuemix-update-check.timer cuemix-update-request.path
  run pct exec "${ctid}" -- systemctl start --no-block cuemix-update-check.service || log_warn "first update check could not start"
}

# update_container : re-push the current cuemix-update.sh into the container
# and run it there. Leaves bridges, udev rules and the container config alone.
update_container() {
  log_step "Update cuemix-web in the container"
  command -v pct >/dev/null 2>&1 || die "pct not found: this is not a Proxmox VE host"
  [[ -f "${UPDATE_SCRIPT}" ]] || die "missing ${UPDATE_SCRIPT} (run from a repository checkout)"
  if [[ -f "${ENV_FILE}" ]]; then
    log_info "loading ${ENV_FILE}"
    # shellcheck source=/dev/null
    source "${ENV_FILE}"
  fi
  local ctid="${CTID_ARG:-${CTID:-}}"
  if ! [[ "${ctid}" =~ ^[0-9]+$ ]] || (( ctid < 100 )); then
    die "no container ID: pass --ctid N or set CTID in ${ENV_FILE}"
  fi
  pct config "${ctid}" >/dev/null 2>&1 || die "container ${ctid} does not exist (see: pct list)"
  if [[ "$(pct status "${ctid}" 2>/dev/null || true)" != "status: running" ]]; then
    die "container ${ctid} is not running; start it with: pct start ${ctid}"
  fi
  log_info "container ${ctid} is running"

  local dry_args=()
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    dry_args+=(--dry-run)
  fi
  run pct push "${ctid}" "${UPDATE_SCRIPT}" /usr/local/sbin/cuemix-update --perms 0755
  push_update_units "${ctid}"
  run pct exec "${ctid}" -- /usr/local/sbin/cuemix-update "${dry_args[@]}" "${UPDATE_ARGS[@]}"
  log_info "Update later with: ./deploy/proxmox-setup.sh --update  (or: pct exec ${ctid} -- cuemix-update)"
}

print_summary() {
  log_step "Done"
  local lan_ip="${WAN_IP%%/*}"
  log_info "cuemix-web URL:    http://${lan_ip}:8080"
  log_info "mDNS URL:          http://${CT_HOSTNAME}.local:8080"
  cat <<NOTE

HAProxy / reverse proxy notes (optional):
  - Proxy /ws as a WebSocket upgrade (Upgrade and Connection headers).
  - Forward X-Forwarded-For and X-Forwarded-Host (client address, Origin check).
  - When proxied, set in /etc/cuemix-web/config.toml inside the container:
        [web]
        trust_forwarded_headers = true
        allowed_origins = ["https://<your-public-host>"]
  - cuemix-web has no authentication: expose it only on a trusted LAN or behind
    an authenticating proxy.

Config: pct exec ${CTID} -- nano /etc/cuemix-web/config.toml
Logs:   pct exec ${CTID} -- journalctl -u cuemix-web -f
NOTE
  log_info "Update later with: ./deploy/proxmox-setup.sh --update  (or: pct exec ${CTID} -- cuemix-update)"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
  parse_args "$@"
  require_root
  WORKDIR="$(mktemp -d)"
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log_warn "dry run: no changes will be made"
  fi
  if [[ "${MODE}" == "update" ]]; then
    update_container
    return 0
  fi
  preflight
  load_config
  collect_settings
  ensure_motu_bridge
  install_host_udev
  ensure_template
  create_container
  start_container
  provision_container
  print_summary
}

main "$@"
