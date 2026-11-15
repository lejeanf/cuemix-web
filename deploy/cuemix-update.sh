#!/usr/bin/env bash
# cuemix-web: manual updater, installed as /usr/local/sbin/cuemix-update.
#
# Downloads a published release of cuemix-web, verifies its sha256, checks that
# the new binary reports the expected version, swaps it in atomically (the
# previous binary is kept as /usr/local/bin/cuemix-web.prev), restarts the
# service and waits for its health endpoint. If the restart or the health check
# fails, the previous binary is restored automatically.
#
# Updates are manual only: no timer, no web trigger. /etc/cuemix-web and
# /var/lib/cuemix-web are never modified.
#
# Usage: sudo cuemix-update [--check | --rollback] [--pre | --version vX.Y.Z[-pre]]
#                           [--force] [--dry-run] [--help]

set -euo pipefail

REPO="lejeanf/cuemix-web"
API_BASE="https://api.github.com/repos/${REPO}"
RELEASE_BASE="https://github.com/${REPO}/releases/download"
RELEASE_PAGE="https://github.com/${REPO}/releases/tag"

BIN="/usr/local/bin/cuemix-web"
PREV="${BIN}.prev"
NEW="${BIN}.new"
CONF_FILE="/etc/cuemix-web/config.toml"
SERVICE="cuemix-web"
DEFAULT_PORT=8080
HEALTH_TIMEOUT=15
LOCK_FILE="/run/cuemix-update.lock"

# Exit codes (see --help).
EXIT_UPDATE_AVAILABLE=10
EXIT_USAGE=2
EXIT_LOCKED=4
EXIT_ROLLED_BACK=5
EXIT_ROLLBACK_FAILED=6

DRY_RUN=0
CHECK=0
ROLLBACK=0
PRE=0
FORCE=0
PIN_SET=0
PIN=""
TAG=""
VERSION=""
TARGET=""
WORKDIR=""
PAYLOAD=""
PAYLOAD_KIND=""
NEW_SRC=""

# ---------------------------------------------------------------------------
# Logging (colors only on an interactive terminal, honours NO_COLOR)
# ---------------------------------------------------------------------------
if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  C_RED=$'\033[31m'; C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'
  C_BOLD=$'\033[1m'; C_RESET=$'\033[0m'
else
  C_RED=""; C_GREEN=""; C_YELLOW=""; C_BOLD=""; C_RESET=""
fi

# syslog_note MSG : best effort copy to syslog; never fails the run.
syslog_note() {
  if [[ "${DRY_RUN}" -eq 0 ]] && command -v logger >/dev/null 2>&1; then
    logger -t cuemix-update -- "$*" 2>/dev/null || true
  fi
}

log_info() { printf '%s[info]%s  %s\n' "${C_GREEN}" "${C_RESET}" "$*"; syslog_note "$*"; }
log_warn() { printf '%s[warn]%s  %s\n' "${C_YELLOW}" "${C_RESET}" "$*" >&2; syslog_note "warning: $*"; }
log_err()  { printf '%s[error]%s %s\n' "${C_RED}" "${C_RESET}" "$*" >&2; syslog_note "error: $*"; }
log_step() { printf '\n%s==> %s%s\n' "${C_BOLD}" "$*" "${C_RESET}"; }
dry()      { printf '%s[dry-run]%s %s\n' "${C_YELLOW}" "${C_RESET}" "$*"; }
fail()     { local code="$1"; shift; log_err "$*"; exit "${code}"; }
die()      { fail 1 "$@"; }

# run CMD... : execute, or only print the command under --dry-run.
run() {
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    dry "$*"
    return 0
  fi
  "$@"
}

cleanup() {
  # Keep the exit status of the script (the last command here must not change it).
  local rc=$?
  if [[ -n "${WORKDIR}" && -d "${WORKDIR}" ]]; then
    rm -rf -- "${WORKDIR}"
  fi
  exit "${rc}"
}
trap cleanup EXIT

usage() {
  cat <<'USAGE'
cuemix-web manual updater

Downloads a cuemix-web release from GitHub, verifies its sha256, swaps the
binary in (the previous one is kept as cuemix-web.prev), restarts the service
and checks /api/health. A failed update is rolled back automatically.

Usage:
  sudo cuemix-update [options]

Options:
  --check                 Print installed and available versions, change nothing.
                          Exit 0 when up to date, 10 when an update is available.
  --pre                   Consider pre-releases too (newest release overall).
  --version vX.Y.Z[-pre]  Install this exact release tag.
  --force                 Reinstall even when that version is already installed.
  --rollback              Restore the previous binary (cuemix-web.prev).
  -n, --dry-run           Print what would happen, change nothing.
  -h, --help              Show this help and exit.

Exit codes:
  0   success, or already up to date
  10  --check: an update is available
  1   error (nothing changed, or the change was reverted)
  2   usage error
  4   another cuemix-update is running
  5   update failed and was rolled back to the previous version
  6   rollback failed: the service needs manual attention

Requires root. Installed binary: /usr/local/bin/cuemix-web. The HTTP port is
read from [network] http_bind in /etc/cuemix-web/config.toml (default 8080).
Configuration and state (/etc/cuemix-web, /var/lib/cuemix-web) are never touched.
USAGE
}

# ---------------------------------------------------------------------------
# Arguments, root and environment checks
# ---------------------------------------------------------------------------
parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -h|--help) usage; exit 0 ;;
      -n|--dry-run) DRY_RUN=1 ;;
      --check) CHECK=1 ;;
      --rollback) ROLLBACK=1 ;;
      --pre) PRE=1 ;;
      --force) FORCE=1 ;;
      --version)
        [[ $# -ge 2 ]] || fail "${EXIT_USAGE}" "--version requires a tag, e.g. --version v0.2.0"
        PIN="$2"
        PIN_SET=1
        shift
        ;;
      --version=*) PIN="${1#--version=}"; PIN_SET=1 ;;
      *) log_err "unknown option: $1"; usage >&2; exit "${EXIT_USAGE}" ;;
    esac
    shift
  done
}

validate_args() {
  if [[ "${ROLLBACK}" -eq 1 && ( "${CHECK}" -eq 1 || "${PRE}" -eq 1 || "${PIN_SET}" -eq 1 || "${FORCE}" -eq 1 ) ]]; then
    fail "${EXIT_USAGE}" "--rollback cannot be combined with --check, --pre, --version or --force"
  fi
  if [[ "${PRE}" -eq 1 && "${PIN_SET}" -eq 1 ]]; then
    fail "${EXIT_USAGE}" "--pre and --version are mutually exclusive"
  fi
  if [[ "${CHECK}" -eq 1 && "${FORCE}" -eq 1 ]]; then
    fail "${EXIT_USAGE}" "--force has no effect with --check"
  fi
  if [[ "${PIN_SET}" -eq 1 ]]; then
    [[ "${PIN}" == v* ]] || PIN="v${PIN}"
    [[ "${PIN}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] \
      || fail "${EXIT_USAGE}" "invalid version '${PIN}' (expected vX.Y.Z or vX.Y.Z-pre)"
  fi
}

require_root() {
  if [[ "$(id -u)" -ne 0 ]]; then
    die "this script must run as root (try: sudo $0 $*)"
  fi
}

require_commands() {
  local cmd
  for cmd in curl sha256sum tar install mv systemctl flock timeout; do
    command -v "${cmd}" >/dev/null 2>&1 || die "required command not found: ${cmd}"
  done
}

acquire_lock() {
  exec 9>"${LOCK_FILE}" || die "cannot open lock file ${LOCK_FILE}"
  flock -n 9 || fail "${EXIT_LOCKED}" "another cuemix-update is already running (lock: ${LOCK_FILE})"
}

# ---------------------------------------------------------------------------
# Versions and configuration
# ---------------------------------------------------------------------------
# binary_version FILE : print the version FILE reports for --version
# ("cuemix-web 0.2.0" -> "0.2.0"). Returns 1 when it does not run or prints
# something else.
binary_version() {
  local out=""
  if [[ -x "$1" ]]; then
    out="$(timeout 10 "$1" --version 2>/dev/null || true)"
  fi
  out="${out%%$'\n'*}"
  if [[ "${out}" =~ ^cuemix-web[[:space:]]+([^[:space:]]+)$ ]]; then
    printf '%s' "${BASH_REMATCH[1]}"
    return 0
  fi
  return 1
}

installed_version() {
  binary_version "${BIN}" || die "cuemix-web is not installed at ${BIN}; run deploy/linux-setup.sh first"
}

# http_port : print the port of [network] http_bind in the config (default 8080).
http_port() {
  local line="" bind="" port=""
  if [[ -r "${CONF_FILE}" ]]; then
    line="$(sed -n '/^[[:space:]]*\[network\][[:space:]]*$/,/^[[:space:]]*\[/ s/^[[:space:]]*http_bind[[:space:]]*=//p' "${CONF_FILE}" | head -n 1 || true)"
    bind="${line%%#*}"
    bind="${bind//[\" ]/}"
  fi
  port="${bind##*:}"
  if [[ "${port}" =~ ^[0-9]{1,5}$ ]]; then
    printf '%s' "${port}"
  else
    printf '%s' "${DEFAULT_PORT}"
  fi
}

# ---------------------------------------------------------------------------
# Release resolution and download
# ---------------------------------------------------------------------------
# api_tag PATH : print the tag_name of the (first) release object at PATH.
api_tag() {
  local path="$1" json="" tag=""
  json="$(curl -fsSL -H 'Accept: application/vnd.github+json' "${API_BASE}/${path}" 2>/dev/null)" \
    || die "GitHub API request failed (${path}); check network access and that the release exists"
  tag="$(printf '%s\n' "${json}" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1 || true)"
  [[ -n "${tag}" ]] || die "no release tag in the GitHub API response for ${path}"
  printf '%s' "${tag}"
}

# resolve_target : set TAG (vX.Y.Z[-pre]) and VERSION (without the v).
resolve_target() {
  if [[ "${PIN_SET}" -eq 1 ]]; then
    TAG="$(api_tag "releases/tags/${PIN}")"
  elif [[ "${PRE}" -eq 1 ]]; then
    TAG="$(api_tag 'releases?per_page=5')"
  else
    TAG="$(api_tag 'releases/latest')"
  fi
  [[ "${TAG}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] \
    || die "unexpected release tag from GitHub: ${TAG}"
  VERSION="${TAG#v}"
}

detect_target() {
  local machine=""
  machine="$(uname -m)"
  case "${machine}" in
    x86_64) TARGET="x86_64-unknown-linux-musl" ;;
    aarch64|arm64) TARGET="aarch64-unknown-linux-musl" ;;
    armv7l|armv6l)
      TARGET="armv7-unknown-linux-musleabihf"
      if [[ "${machine}" == "armv6l" ]]; then
        log_warn "armv6l (e.g. Raspberry Pi Zero/1): the armv7 build may not run; check the release notes"
      fi
      ;;
    *) die "unsupported architecture: ${machine} (supported: x86_64, aarch64, armv7l)" ;;
  esac
}

# fetch_asset NAME : download NAME and NAME.sha256 from the release into WORKDIR.
fetch_asset() {
  local name="$1" url="${RELEASE_BASE}/${TAG}/$1"
  curl -fsSL -o "${WORKDIR}/${name}" "${url}" 2>/dev/null || return 1
  curl -fsSL -o "${WORKDIR}/${name}.sha256" "${url}.sha256" 2>/dev/null || return 1
}

# fetch_payload : download the tarball (or the .deb when no tarball is
# published) and verify its sha256.
fetch_payload() {
  local tarball="cuemix-web-${TAG}-${TARGET}.tar.gz"
  local deb="cuemix-web-${TAG}-${TARGET}.deb"
  log_step "Download ${TAG} (${TARGET})"
  if fetch_asset "${tarball}"; then
    PAYLOAD="${tarball}"
    PAYLOAD_KIND="tar"
  else
    log_warn "${tarball} is not available; falling back to the .deb payload"
    fetch_asset "${deb}" || die "neither ${tarball} nor ${deb} could be downloaded for ${TAG}"
    PAYLOAD="${deb}"
    PAYLOAD_KIND="deb"
  fi
  if ! (cd -- "${WORKDIR}" && sha256sum -c "${PAYLOAD}.sha256" >/dev/null); then
    die "sha256 verification failed for ${PAYLOAD}; nothing was installed"
  fi
  log_info "downloaded and verified ${PAYLOAD}"
}

# extract_payload : unpack the payload and set NEW_SRC to the cuemix-web binary.
extract_payload() {
  local dir="${WORKDIR}/payload"
  mkdir -p -- "${dir}"
  case "${PAYLOAD_KIND}" in
    tar)
      tar -xzf "${WORKDIR}/${PAYLOAD}" -C "${dir}" || die "cannot unpack ${PAYLOAD}"
      NEW_SRC="${dir}/cuemix-web"
      ;;
    deb)
      command -v dpkg-deb >/dev/null 2>&1 || die "dpkg-deb is needed to unpack the .deb payload"
      dpkg-deb -x "${WORKDIR}/${PAYLOAD}" "${dir}" || die "cannot unpack ${PAYLOAD}"
      NEW_SRC="${dir}/usr/bin/cuemix-web"
      ;;
  esac
  [[ -f "${NEW_SRC}" ]] || die "release payload ${PAYLOAD} does not contain cuemix-web"
  chmod 0755 -- "${NEW_SRC}"
}

# verify_new_binary : the unpacked binary must run and report exactly VERSION.
verify_new_binary() {
  local reported=""
  reported="$(binary_version "${NEW_SRC}" || true)"
  if [[ "${reported}" != "${VERSION}" ]]; then
    die "the new binary reports '${reported:-nothing}' instead of ${VERSION}; nothing was installed"
  fi
  log_info "new binary runs and reports ${reported}"
}

# ---------------------------------------------------------------------------
# Swap, restart and health
# ---------------------------------------------------------------------------
# atomic_replace SRC : install SRC next to the binary, then rename it over
# BIN (same filesystem, so the rename is atomic).
atomic_replace() {
  run install -m 0755 -- "$1" "${NEW}"
  run mv -f -- "${NEW}" "${BIN}"
}

restart_service() {
  run systemctl restart "${SERVICE}"
}

# wait_healthy EXPECTED_VERSION : poll /api/health on 127.0.0.1 for up to
# HEALTH_TIMEOUT seconds until it reports status ok and that version.
wait_healthy() {
  local expected="$1" port="" url="" body="" esc="" vre="" deadline=0
  port="$(http_port)"
  url="http://127.0.0.1:${port}/api/health"
  esc="${expected//./\\.}"
  vre="\"version\"[[:space:]]*:[[:space:]]*\"${esc}\""
  deadline=$((SECONDS + HEALTH_TIMEOUT))
  while :; do
    body="$(curl -fsS --max-time 3 "${url}" 2>/dev/null || true)"
    if [[ "${body}" =~ \"status\"[[:space:]]*:[[:space:]]*\"ok\" && "${body}" =~ ${vre} ]]; then
      log_info "health check passed: ${url} -> ${body}"
      return 0
    fi
    if (( SECONDS >= deadline )); then
      break
    fi
    sleep 1
  done
  log_err "health check failed after ${HEALTH_TIMEOUT}s: ${url} answered '${body:-no response}' (expected status ok, version ${expected})"
  return 1
}

print_journal() {
  log_warn "last 30 journal lines of ${SERVICE}:"
  journalctl -u "${SERVICE}" -n 30 --no-pager || true
}

# rollback_after_failure TARGET_VERSION REASON : restore the previous binary,
# restart the service and check it. Always exits.
rollback_after_failure() {
  local target="$1" reason="$2" prev_version=""
  log_err "update to ${target} failed: ${reason}"
  [[ -f "${PREV}" ]] || { print_journal; fail "${EXIT_ROLLBACK_FAILED}" "no ${PREV} to restore; fix ${SERVICE} manually"; }
  log_warn "restoring the previous binary from ${PREV}"
  atomic_replace "${PREV}"
  prev_version="$(binary_version "${PREV}" || true)"
  if restart_service && wait_healthy "${prev_version}"; then
    print_journal
    fail "${EXIT_ROLLED_BACK}" "rolled back to ${prev_version}; ${target} was not kept (see journal above)"
  fi
  print_journal
  fail "${EXIT_ROLLBACK_FAILED}" "rollback to ${prev_version:-unknown} did not become healthy; check: systemctl status ${SERVICE}"
}

# ---------------------------------------------------------------------------
# Modes
# ---------------------------------------------------------------------------
do_check() {
  local current="" hint="sudo cuemix-update"
  current="$(installed_version)"
  resolve_target
  log_info "installed: ${current}   available: ${VERSION} (${TAG})"
  if [[ "${current}" == "${VERSION}" ]]; then
    log_info "up to date"
    exit 0
  fi
  if [[ "${PRE}" -eq 1 ]]; then
    hint+=" --pre"
  fi
  if [[ "${PIN_SET}" -eq 1 ]]; then
    hint+=" --version ${PIN}"
  fi
  log_info "update available; install it with: ${hint}"
  exit "${EXIT_UPDATE_AVAILABLE}"
}

do_rollback() {
  local current="" prev_version=""
  [[ -f "${PREV}" ]] || die "no previous binary at ${PREV}; nothing to roll back to"
  current="$(binary_version "${BIN}" || true)"
  prev_version="$(binary_version "${PREV}" || true)"
  [[ -n "${prev_version}" ]] || die "${PREV} does not run (--version failed); refusing to roll back"
  log_step "Roll back ${SERVICE}"
  log_info "rolling back: ${current:-unknown} -> ${prev_version}"
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    dry "would install ${PREV} over ${BIN}, restart ${SERVICE}, then check http://127.0.0.1:$(http_port)/api/health for ${prev_version}"
    exit 0
  fi
  atomic_replace "${PREV}"
  if restart_service && wait_healthy "${prev_version}"; then
    log_info "rolled back to ${prev_version}"
    exit 0
  fi
  print_journal
  fail "${EXIT_ROLLBACK_FAILED}" "rollback to ${prev_version} was applied but the service is not healthy; check: systemctl status ${SERVICE}"
}

do_update() {
  local current="" tarball=""
  current="$(installed_version)"
  resolve_target
  log_info "installed: ${current}   available: ${VERSION} (${TAG})"
  if [[ "${current}" == "${VERSION}" && "${FORCE}" -eq 0 ]]; then
    log_info "cuemix-web ${VERSION} is already installed; nothing to do (use --force to reinstall)"
    exit 0
  fi
  detect_target
  tarball="cuemix-web-${TAG}-${TARGET}.tar.gz"

  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log_step "Plan (dry run)"
    dry "download ${tarball} (or the .deb) and its .sha256 from ${RELEASE_BASE}/${TAG}/"
    dry "verify sha256, unpack, and require the new binary to report ${VERSION}"
    dry "keep ${BIN} as ${PREV}, install the new binary as ${NEW}, mv -f over ${BIN}"
    dry "systemctl restart ${SERVICE}; poll http://127.0.0.1:$(http_port)/api/health for ${HEALTH_TIMEOUT}s"
    dry "on failure: restore ${PREV}, restart, check health, print journal, exit ${EXIT_ROLLED_BACK}"
    exit 0
  fi

  WORKDIR="$(mktemp -d)"
  fetch_payload
  extract_payload
  verify_new_binary

  log_step "Install ${VERSION}"
  run install -m 0755 -- "${BIN}" "${PREV}.tmp"
  run mv -f -- "${PREV}.tmp" "${PREV}"
  log_info "previous binary kept as ${PREV} (${current})"
  atomic_replace "${NEW_SRC}"

  log_step "Restart ${SERVICE} and check health"
  if restart_service && wait_healthy "${VERSION}"; then
    log_info "updated cuemix-web ${current} -> ${VERSION}"
    log_info "release notes: ${RELEASE_PAGE}/${TAG}"
    exit 0
  fi
  rollback_after_failure "${VERSION}" "restart or health check failed"
}

main() {
  parse_args "$@"
  validate_args
  require_root "$@"
  require_commands
  acquire_lock

  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log_warn "dry run: no changes will be made"
  fi

  if [[ "${ROLLBACK}" -eq 1 ]]; then
    do_rollback
  elif [[ "${CHECK}" -eq 1 ]]; then
    do_check
  else
    do_update
  fi
}

main "$@"
