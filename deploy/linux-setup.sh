#!/usr/bin/env bash
# cuemix-web: Linux / single-board computer setup (see docs/HANDOFF.md section 6.2).
#
# Installs the prebuilt static release binary, a non-root service user, the
# config and state directories, a systemd unit, and a udev rule that gives the
# MOTU USB CDC-Ethernet link its link-local address when it appears.
# Targets: Raspberry Pi, SBCs, and Linux desktops (x86_64, aarch64, armv7).
#
# Run as root from a checkout of this repository. Idempotent: existing config
# is never overwritten.
#
# Usage: sudo deploy/linux-setup.sh [--dry-run] [--uninstall] [--help]

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
ENV_FILE="${SCRIPT_DIR}/linux-setup.env"
UDEV_TEMPLATE="${SCRIPT_DIR}/99-motu-mk5.rules.template"
UNIT_TEMPLATE="${SCRIPT_DIR}/cuemix-web.service"
UPDATER_SRC="${SCRIPT_DIR}/cuemix-update.sh"
CONFIG_EXAMPLE="${REPO_DIR}/config.example.toml"

CONF_DIR="/etc/cuemix-web"
STATE_DIR="/var/lib/cuemix-web"
UNIT_PATH="/etc/systemd/system/cuemix-web.service"
UDEV_RULE="/etc/udev/rules.d/99-motu-mk5.rules"
HELPER_PATH="/usr/local/sbin/cuemix-motu-linklocal"
NM_CONF="/etc/NetworkManager/conf.d/99-cuemix-motu.conf"
UPDATER_PATH="/usr/local/sbin/cuemix-update"
# Daily release check + install requested from the web UI (deploy/systemd).
UPDATE_UNITS=(cuemix-update-check.service cuemix-update-check.timer cuemix-update-request.path cuemix-update-request.service)

RELEASE_API="https://api.github.com/repos/lejeanf/cuemix-web/releases/latest"
RELEASE_BASE="https://github.com/lejeanf/cuemix-web/releases/download"

# Defaults, overridden by deploy/linux-setup.env.
MOTU_IFACE_IP="169.254.254.1/16"
INSTALL_DIR="/usr/local/bin"
SERVICE_USER="cuemix"
HTTP_BIND="0.0.0.0:8080"
OSC_PORT="8000"

DRY_RUN=0
UNINSTALL=0
WORKDIR=""
TARGET=""
TAG=""
ASSET_NAME=""

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
cuemix-web Linux / SBC setup

Installs cuemix-web from the GitHub release (static musl build, or the .deb on
Debian/Ubuntu), creates the cuemix service user, config and state directories,
the systemd unit, and the MOTU udev rule (assigns 169.254.254.1/16 on hotplug).

Usage:
  sudo deploy/linux-setup.sh [--dry-run] [--uninstall] [--help]

Options:
  -n, --dry-run   Print every action without changing the system.
  -u, --uninstall Remove the service, binary, udev rule, helper, user and the
                  NetworkManager exception. /etc/cuemix-web and
                  /var/lib/cuemix-web are kept. Packages are not removed.
  -h, --help      Show this help and exit.

Settings are read from deploy/linux-setup.env when it exists (copy
deploy/linux-setup.env.example). Defaults match that example.

Requirements: root, systemd, curl, sha256sum, install, tar. Network access to
GitHub. Supported machine types: x86_64, aarch64/arm64, armv7l/armv6l.
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
      -u|--uninstall) UNINSTALL=1 ;;
      *) log_err "unknown option: $1"; usage >&2; exit 2 ;;
    esac
    shift
  done
}

require_root() {
  if [[ "$(id -u)" -ne 0 ]]; then
    local hint="sudo $0"
    [[ "${DRY_RUN}" -eq 1 ]] && hint+=" --dry-run"
    [[ "${UNINSTALL}" -eq 1 ]] && hint+=" --uninstall"
    die "this script must run as root (try: ${hint})"
  fi
}

load_config() {
  if [[ -f "${ENV_FILE}" ]]; then
    log_info "loading ${ENV_FILE}"
    # shellcheck source=/dev/null
    source "${ENV_FILE}"
  fi
  [[ "${MOTU_IFACE_IP}" =~ ^169\.254\.[0-9]{1,3}\.[0-9]{1,3}/16$ ]] \
    || die "MOTU_IFACE_IP must be a link-local 169.254.x.x/16 address"
  [[ "${INSTALL_DIR}" =~ ^/[A-Za-z0-9_./-]+$ ]] || die "INSTALL_DIR must be an absolute path"
  [[ "${SERVICE_USER}" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || die "invalid SERVICE_USER: ${SERVICE_USER}"
  [[ "${HTTP_BIND}" =~ ^[][A-Za-z0-9._:-]+$ ]] || die "invalid HTTP_BIND: ${HTTP_BIND}"
  [[ "${OSC_PORT}" =~ ^[0-9]{1,5}$ ]] || die "OSC_PORT must be a port number"
}

require_commands() {
  local cmd
  for cmd in systemctl install sha256sum; do
    command -v "${cmd}" >/dev/null 2>&1 || die "required command not found: ${cmd} (is systemd present?)"
  done
  if [[ "${UNINSTALL}" -eq 0 ]]; then
    for cmd in curl tar useradd udevadm ip; do
      command -v "${cmd}" >/dev/null 2>&1 || die "required command not found: ${cmd}"
    done
    [[ -f "${UDEV_TEMPLATE}" ]] || die "missing ${UDEV_TEMPLATE} (run from a repository checkout)"
    [[ -f "${UNIT_TEMPLATE}" ]] || die "missing ${UNIT_TEMPLATE} (run from a repository checkout)"
    [[ -f "${CONFIG_EXAMPLE}" ]] || die "missing ${CONFIG_EXAMPLE} (run from a repository checkout)"
    [[ -f "${UPDATER_SRC}" ]] || die "missing ${UPDATER_SRC} (run from a repository checkout)"
  fi
}

# ---------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------
uninstall() {
  log_step "Uninstall cuemix-web"

  run systemctl disable --now cuemix-web || log_warn "cuemix-web service was not active"
  run systemctl disable --now cuemix-update-check.timer cuemix-update-request.path || true
  local u
  for u in "${UPDATE_UNITS[@]}"; do run rm -f -- "/etc/systemd/system/${u}"; done
  run rm -f -- "${UNIT_PATH}"
  run systemctl daemon-reload

  run rm -f -- "${INSTALL_DIR}/cuemix-web" "${INSTALL_DIR}/cuemix-web.prev" "${INSTALL_DIR}/cuemix-web.new"
  run rm -f -- "${UPDATER_PATH}"

  run rm -f -- "${UDEV_RULE}" "${HELPER_PATH}"
  run udevadm control --reload-rules || log_warn "could not reload udev rules"

  if [[ -f "${NM_CONF}" ]]; then
    run rm -f -- "${NM_CONF}"
    if systemctl is-active --quiet NetworkManager 2>/dev/null; then
      run systemctl reload NetworkManager || log_warn "could not reload NetworkManager"
    fi
  fi

  # Remove the service user only if it still looks like the one this script
  # creates (no login shell, no home directory).
  local passwd_line
  passwd_line="$(getent passwd "${SERVICE_USER}" || true)"
  if [[ -n "${passwd_line}" && "${passwd_line}" == *":/nonexistent:/usr/sbin/nologin" ]]; then
    run userdel "${SERVICE_USER}" || log_warn "could not remove user ${SERVICE_USER}"
  elif [[ -n "${passwd_line}" ]]; then
    log_warn "user ${SERVICE_USER} was not created by this script; leaving it in place"
  fi

  log_info "kept: ${CONF_DIR} (configuration) and ${STATE_DIR} (presets, labels)"
  log_info "kept: installed packages such as avahi-daemon (they may be used by other software)"
  log_info "uninstall complete"
}

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------
detect_target() {
  local machine
  machine="$(uname -m)"
  case "${machine}" in
    x86_64)
      TARGET="x86_64-unknown-linux-musl" ;;
    aarch64|arm64)
      TARGET="aarch64-unknown-linux-musl" ;;
    armv7l|armv6l)
      TARGET="armv7-unknown-linux-musleabihf"
      if [[ "${machine}" == "armv6l" ]]; then
        log_warn "armv6l (e.g. Raspberry Pi Zero/1): the armv7 build may not run; check the release notes"
      fi
      ;;
    *)
      die "unsupported architecture: ${machine} (supported: x86_64, aarch64, armv7l)" ;;
  esac
  log_info "architecture ${machine} -> ${TARGET}"
}

use_deb() {
  command -v dpkg >/dev/null 2>&1 && command -v dpkg-deb >/dev/null 2>&1
}

# detect_motu_iface : print the name of a MOTU (USB vendor 07fd) CDC-Ethernet
# interface if one is present; return 1 otherwise.
detect_motu_iface() {
  local dev name driver vendor
  for dev in /sys/class/net/*; do
    [[ -e "${dev}/device" ]] || continue
    name="${dev##*/}"
    driver="$(basename -- "$(readlink -f "${dev}/device/driver" 2>/dev/null || echo none)")"
    [[ "${driver}" == "cdc_ether" ]] || continue
    vendor="$(cat "$(readlink -f "${dev}/device")/../idVendor" 2>/dev/null || true)"
    if [[ "${vendor}" == "07fd" ]]; then
      printf '%s' "${name}"
      return 0
    fi
  done
  return 1
}

# ---------------------------------------------------------------------------
# Download and verify the release
# ---------------------------------------------------------------------------
resolve_tag() {
  local json=""
  if json="$(curl -fsSL -H 'Accept: application/vnd.github+json' "${RELEASE_API}" 2>/dev/null)"; then
    TAG="$(printf '%s\n' "${json}" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1 || true)"
  fi
  if [[ -z "${TAG}" ]]; then
    if [[ "${DRY_RUN}" -eq 1 ]]; then
      TAG="<latest-tag>"
      log_warn "could not resolve the latest release tag (dry run continues with a placeholder)"
    else
      die "no published release found (${RELEASE_API}); build from source or try again later"
    fi
  fi
  log_info "latest release: ${TAG}"
}

download_release() {
  log_step "Download and verify release ${TAG} (${TARGET})"
  local asset base
  if use_deb; then
    asset="cuemix-web-${TAG}-${TARGET}.deb"
    log_info "package manager detected: using the .deb asset"
  else
    asset="cuemix-web-${TAG}-${TARGET}.tar.gz"
    log_info "using the tarball asset"
  fi
  base="${RELEASE_BASE}/${TAG}"

  run curl -fsSL -o "${WORKDIR}/${asset}" "${base}/${asset}"
  run curl -fsSL -o "${WORKDIR}/${asset}.sha256" "${base}/${asset}.sha256"
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    run sha256sum -c "${asset}.sha256"
  else
    (cd -- "${WORKDIR}" && sha256sum -c "${asset}.sha256")
  fi
  ASSET_NAME="${asset}"
}

# ---------------------------------------------------------------------------
# Installation steps
# ---------------------------------------------------------------------------
install_binary() {
  log_step "Install binary to ${INSTALL_DIR}"
  local src_bin
  run install -d -m 0755 "${INSTALL_DIR}"
  if use_deb; then
    run mkdir -p -- "${WORKDIR}/deb"
    run dpkg-deb -x "${WORKDIR}/${ASSET_NAME}" "${WORKDIR}/deb"
    src_bin="${WORKDIR}/deb/usr/bin/cuemix-web"
  else
    run mkdir -p -- "${WORKDIR}/tar"
    run tar -xzf "${WORKDIR}/${ASSET_NAME}" -C "${WORKDIR}/tar"
    src_bin="${WORKDIR}/tar/cuemix-web"
  fi
  # The .deb payload is extracted rather than registered with dpkg, so the
  # binary location matches the systemd unit and uninstall stays complete.
  run install -m 0755 -- "${src_bin}" "${INSTALL_DIR}/cuemix-web"
}

install_updater() {
  log_step "Update command (manual only)"
  if [[ "${INSTALL_DIR}" != "/usr/local/bin" ]]; then
    log_warn "cuemix-update manages /usr/local/bin/cuemix-web; INSTALL_DIR=${INSTALL_DIR} will not be updated by it"
  fi
  run install -d -m 0755 "$(dirname -- "${UPDATER_PATH}")"
  install_if_changed "${UPDATER_SRC}" "${UPDATER_PATH}" 0755 || true
}

ensure_service_user() {
  log_step "Service user ${SERVICE_USER}"
  if id -u "${SERVICE_USER}" >/dev/null 2>&1; then
    log_info "user ${SERVICE_USER} exists"
  else
    log_info "creating system user ${SERVICE_USER} (no login shell, no home)"
    run useradd --system --no-create-home --home-dir /nonexistent \
      --shell /usr/sbin/nologin "${SERVICE_USER}"
  fi
}

install_directories() {
  log_step "Configuration and state directories"
  run install -d -m 0755 -- "${CONF_DIR}"
  run install -d -m 0750 -o "${SERVICE_USER}" -g "${SERVICE_USER}" -- "${STATE_DIR}"
}

install_config() {
  log_step "Configuration"
  if [[ -e "${CONF_DIR}/config.toml" ]]; then
    log_info "keeping existing ${CONF_DIR}/config.toml"
    return 0
  fi
  local gen="${WORKDIR}/config.toml" iface=""
  iface="$(detect_motu_iface || true)"
  if [[ -z "${iface}" ]]; then
    iface="eth1"
    log_warn "no MOTU interface detected now; config uses motu_interface = eth1. Edit it after connecting the MOTU."
  else
    log_info "MOTU interface detected: ${iface}"
  fi

  cp -- "${CONFIG_EXAMPLE}" "${gen}"
  sed -i \
    -e "s|^motu_interface = .*|motu_interface = \"${iface}\"|" \
    -e "s|^http_bind = .*|http_bind = \"${HTTP_BIND}\"|" \
    -e "s|^osc_listen = .*|osc_listen = \"0.0.0.0:${OSC_PORT}\"|" \
    "${gen}"
  run install -m 0640 -o root -g "${SERVICE_USER}" -- "${gen}" "${CONF_DIR}/config.toml"
  log_info "wrote ${CONF_DIR}/config.toml"
}

# install_if_changed SRC DST MODE : returns 0 when DST was (re)written.
install_if_changed() {
  local src="$1" dst="$2" mode="$3"
  if [[ -f "${dst}" ]] && cmp -s -- "${src}" "${dst}"; then
    log_info "unchanged: ${dst}"
    return 1
  fi
  run install -m "${mode}" -- "${src}" "${dst}"
  log_info "installed: ${dst}"
  return 0
}

install_motu_link() {
  log_step "MOTU link-local address (udev)"
  local helper_src="${WORKDIR}/motu-linklocal" rule_src="${WORKDIR}/99-motu-mk5.rules"
  local changed=0

  cat > "${helper_src}" <<'HELPER'
#!/bin/sh
# Installed by cuemix-web deploy/linux-setup.sh. Called by udev with the MOTU
# CDC-Ethernet interface name: bring the link up and give it its link-local address.
set -eu
IF="$1"
ip link set "$IF" up
ip addr replace "@ADDR@" dev "$IF"
HELPER
  sed -i "s|@ADDR@|${MOTU_IFACE_IP}|" "${helper_src}"
  sed "s|@HELPER@|${HELPER_PATH}|" "${UDEV_TEMPLATE}" > "${rule_src}"

  install_if_changed "${helper_src}" "${HELPER_PATH}" 0755 && changed=1 || true
  install_if_changed "${rule_src}" "${UDEV_RULE}" 0644 && changed=1 || true
  if [[ "${changed}" -eq 1 ]]; then
    run udevadm control --reload-rules
    run udevadm trigger --subsystem-match=net --action=add
  fi
}

configure_network_manager() {
  if ! command -v nmcli >/dev/null 2>&1; then
    return 0
  fi
  log_step "NetworkManager exception for the MOTU link"
  local src="${WORKDIR}/99-cuemix-motu.conf"
  cat > "${src}" <<'NMCONF'
# Managed by cuemix-web deploy/linux-setup.sh. NetworkManager must not
# reconfigure the MOTU CDC-Ethernet link: its link-local address is set by udev.
# Note: this applies to every cdc_ether (USB Ethernet) device on the machine.
[keyfile]
unmanaged-devices=driver:cdc_ether
NMCONF
  run install -d -m 0755 -- "$(dirname -- "${NM_CONF}")"
  if install_if_changed "${src}" "${NM_CONF}" 0644; then
    if systemctl is-active --quiet NetworkManager 2>/dev/null; then
      run systemctl reload NetworkManager || log_warn "could not reload NetworkManager"
    fi
  fi
}

install_avahi() {
  log_step "mDNS (avahi-daemon), best effort"
  if [[ -f /usr/lib/systemd/system/avahi-daemon.service || -f /lib/systemd/system/avahi-daemon.service ]]; then
    log_info "avahi-daemon already installed"
  elif command -v apt-get >/dev/null 2>&1; then
    run env DEBIAN_FRONTEND=noninteractive apt-get update -qq || log_warn "apt-get update failed"
    run env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends avahi-daemon \
      || log_warn "could not install avahi-daemon; cuemix-web.local will not resolve"
  elif command -v dnf >/dev/null 2>&1; then
    run dnf install -y avahi || log_warn "could not install avahi; cuemix-web.local will not resolve"
  elif command -v pacman >/dev/null 2>&1; then
    run pacman -S --needed --noconfirm avahi || log_warn "could not install avahi; cuemix-web.local will not resolve"
  else
    log_warn "no supported package manager (apt/dnf/pacman): install avahi-daemon yourself for mDNS"
    return 0
  fi
  run systemctl enable --now avahi-daemon || log_warn "could not enable avahi-daemon"
}

install_unit() {
  log_step "systemd unit"
  local gen="${WORKDIR}/cuemix-web.service"
  sed \
    -e "s|^ExecStart=/usr/local/bin/cuemix-web |ExecStart=${INSTALL_DIR}/cuemix-web |" \
    -e "s|^User=cuemix$|User=${SERVICE_USER}|" \
    -e "s|^Group=cuemix$|Group=${SERVICE_USER}|" \
    "${UNIT_TEMPLATE}" > "${gen}"
  install_if_changed "${gen}" "${UNIT_PATH}" 0644 || true
  run systemctl daemon-reload
  run systemctl enable cuemix-web
  run systemctl restart cuemix-web
  install_update_units
}

# install_update_units : hourly check timer + web-requested install (path unit).
# Only for the default binary location, which is what cuemix-update manages.
install_update_units() {
  if [[ "${INSTALL_DIR}" != "/usr/local/bin" ]]; then
    log_warn "update units skipped: cuemix-update only manages /usr/local/bin/cuemix-web"
    return 0
  fi
  log_step "Update check and web-requested updates"
  local u
  for u in "${UPDATE_UNITS[@]}"; do
    install_if_changed "${SCRIPT_DIR}/systemd/${u}" "/etc/systemd/system/${u}" 0644 || true
  done
  run systemctl daemon-reload
  run systemctl enable --now cuemix-update-check.timer cuemix-update-request.path
  # First check now, so the UI knows the update status right away.
  run systemctl start --no-block cuemix-update-check.service || log_warn "first update check could not start"
}

print_summary() {
  log_step "Done"
  log_info "binary:  ${INSTALL_DIR}/cuemix-web (${TAG}, ${TARGET})"
  log_info "config:  ${CONF_DIR}/config.toml"
  log_info "web UI:  http://<this-host>:${HTTP_BIND##*:}  (mDNS: http://$(hostname -s 2>/dev/null || echo this-host).local:${HTTP_BIND##*:})"
  log_info "status:  systemctl status cuemix-web    logs: journalctl -u cuemix-web -f"
  log_info "reverse proxy: when proxied, set [web] trust_forwarded_headers = true and allowed_origins in config.toml"
  log_info "Update later with: sudo cuemix-update"
}

main() {
  parse_args "$@"
  require_root
  require_commands
  load_config
  WORKDIR="$(mktemp -d)"

  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log_warn "dry run: no changes will be made"
  fi

  if [[ "${UNINSTALL}" -eq 1 ]]; then
    uninstall
    return 0
  fi

  detect_target
  resolve_tag
  download_release
  install_binary
  install_updater
  ensure_service_user
  install_directories
  install_config
  install_motu_link
  configure_network_manager
  install_avahi
  install_unit
  print_summary
}

main "$@"
