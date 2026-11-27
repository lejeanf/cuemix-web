#!/usr/bin/env bash
# cuemix-web: manual updater, installed as /usr/local/sbin/cuemix-update.
#
# Downloads a published release of cuemix-web, verifies its sha256, checks that
# the new binary reports the expected version, swaps it in atomically (the
# previous binary is kept as /usr/local/bin/cuemix-web.prev), restarts the
# service and waits for its health endpoint. If the restart or the health check
# fails, the previous binary is restored automatically.
#
# Updates are never automatic: an hourly timer only *checks* (--check
# --status-file), and an install runs when a person asks for it, from a shell
# or from the web UI (which only writes a request file, empty or holding one
# release tag, that the root unit cuemix-update-request.path turns into
# `cuemix-update --run-request`: the file is untrusted input, see
# do_run_request). The configuration is never modified; in /var/lib/cuemix-web
# only the status and result files are written.
#
# Usage: sudo cuemix-update [--check | --rollback] [--pre | --version vX.Y.Z[-pre]]
#                           [--force] [--dry-run] [--help]
#        sudo cuemix-update --run-request FILE --result-file FILE [--status-file FILE]

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
# Root-only directory where every file this script publishes is built (see atomic_publish).
# The web service's state directory is writable by that unprivileged user, so root never
# creates or writes a file there, it only renames finished files into it.
PRIVATE_DIR="/var/lib/cuemix-update"
# Oldest release that may be installed (the floor): anything older would bring back a
# security hole or a serious bug that a later release fixed. Keep it equal to MIN_PIN_VERSION
# in src/web/update.rs and to deploy/min-version (CI checks), and raise all three with every
# release that fixes one. The newest release also publishes its floor as the `min-version`
# asset, so an installed updater learns a higher floor without being reinstalled; that value
# can only raise this one. Older releases need --version with --force, from a root shell.
MIN_VERSION="0.7.2"
MIN_VERSION_URL="https://github.com/${REPO}/releases/latest/download/min-version"
# The floor in force (MIN_VERSION, or higher from MIN_VERSION_URL); set by effective_floor.
FLOOR=""
# Unprivileged account that runs a downloaded binary's --version check.
CHECK_UID=65534
CHECK_GID=65534
# Seconds to wait for another cuemix-update to finish (--wait-lock, used by --run-request).
LOCK_WAIT=0
# Capabilities this updater adds to the status file ("features"): the web UI only offers
# what the root side can do. pin = --run-request installs the tag the request file holds.
FEATURES='"pin"'
# Release tag syntax accepted from the request file and listed in the status file.
TAG_RE='^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'
# The request file is untrusted input (written by the unprivileged web service).
REQUEST_MAX_BYTES=64
# Releases asked from GitHub, and kept in the status file.
RELEASES_FETCH=30
RELEASES_KEEP=20

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
STATUS_FILE=""
RUN_REQUEST=""
RESULT_FILE=""
REQUEST_TAG=""
RELEASES_JSON="[]"
LATEST_STABLE=""
LATEST_PRE=""
CHANNEL=""
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

# Invoked indirectly via `trap cleanup EXIT` (set in main).
# shellcheck disable=SC2329
cleanup() {
  # Keep the exit status of the script (the last command here must not change it).
  local rc=$?
  if [[ -n "${WORKDIR}" && -d "${WORKDIR}" ]]; then
    rm -rf -- "${WORKDIR}"
  fi
  exit "${rc}"
}

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
                          Default: [update] channel in config.toml ("stable", or
                          "prerelease" which implies --pre).
  --status-file PATH      With --check (or --run-request): also write the result as
                          JSON to PATH (read by the web UI): installed and latest
                          versions, the newest releases, and the "features" this
                          updater supports.
  --version vX.Y.Z[-pre]  Install this exact release tag, newer or older than the
                          installed one (older = a downgrade; the same sha256,
                          health check and rollback apply).
  --run-request FILE      What the web UI's request unit runs: FILE is untrusted
                          input written by the unprivileged web service. At most
                          64 bytes: empty (newest release of the channel) or one
                          release tag vX.Y.Z[-pre]; anything else is refused. FILE
                          is removed, the install runs, and the exit code is
                          written to --result-file.
  --result-file PATH      With --run-request: "CODE TIMESTAMP [TAG]" is written here.
  --force                 Reinstall even when that version is already installed, or
                          install a release older than the floor (releases that lack a
                          security or serious bug fix; the web UI can never do this).
  --wait-lock             Wait up to 60 s for another cuemix-update to finish instead
                          of exiting with code 4 (used by --run-request).
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
      --wait-lock) LOCK_WAIT=60 ;;
      --version)
        [[ $# -ge 2 ]] || fail "${EXIT_USAGE}" "--version requires a tag, e.g. --version v0.2.0"
        PIN="$2"
        PIN_SET=1
        shift
        ;;
      --version=*) PIN="${1#--version=}"; PIN_SET=1 ;;
      --status-file)
        [[ $# -ge 2 ]] || fail "${EXIT_USAGE}" "--status-file requires a path"
        STATUS_FILE="$2"
        shift
        ;;
      --run-request)
        [[ $# -ge 2 ]] || fail "${EXIT_USAGE}" "--run-request requires a path"
        RUN_REQUEST="$2"
        shift
        ;;
      --result-file)
        [[ $# -ge 2 ]] || fail "${EXIT_USAGE}" "--result-file requires a path"
        RESULT_FILE="$2"
        shift
        ;;
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
  if [[ -n "${STATUS_FILE}" && "${CHECK}" -eq 0 && -z "${RUN_REQUEST}" ]]; then
    fail "${EXIT_USAGE}" "--status-file only works with --check or --run-request"
  fi
  if [[ -n "${RUN_REQUEST}" ]]; then
    if [[ "${CHECK}" -eq 1 || "${ROLLBACK}" -eq 1 || "${PRE}" -eq 1 || "${PIN_SET}" -eq 1 || "${FORCE}" -eq 1 || "${DRY_RUN}" -eq 1 ]]; then
      fail "${EXIT_USAGE}" "--run-request cannot be combined with --check, --rollback, --pre, --version, --force or --dry-run"
    fi
    [[ -n "${RESULT_FILE}" ]] || fail "${EXIT_USAGE}" "--run-request requires --result-file"
  elif [[ -n "${RESULT_FILE}" ]]; then
    fail "${EXIT_USAGE}" "--result-file only works with --run-request"
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
  for cmd in curl sha256sum tar install mv systemctl flock timeout mktemp awk head tr wc; do
    command -v "${cmd}" >/dev/null 2>&1 || die "required command not found: ${cmd}"
  done
}

acquire_lock() {
  exec 9>"${LOCK_FILE}" || die "cannot open lock file ${LOCK_FILE}"
  if [[ "${LOCK_WAIT}" -gt 0 ]]; then
    flock -w "${LOCK_WAIT}" 9 || fail "${EXIT_LOCKED}" "another cuemix-update is still running after ${LOCK_WAIT}s (lock: ${LOCK_FILE})"
  else
    flock -n 9 || fail "${EXIT_LOCKED}" "another cuemix-update is already running (lock: ${LOCK_FILE})"
  fi
}

# ---------------------------------------------------------------------------
# Versions and configuration
# ---------------------------------------------------------------------------
# unprivileged CMD... : run CMD as nobody when this script runs as root (a binary that has
# only been downloaded never runs as root), else as is. Returns 126 when root cannot drop.
unprivileged() {
  if [[ "$(id -u)" -ne 0 ]]; then
    "$@"
  elif command -v setpriv >/dev/null 2>&1; then
    env -i PATH=/usr/bin:/bin setpriv --reuid="${CHECK_UID}" --regid="${CHECK_GID}" --clear-groups --no-new-privs -- "$@"
  elif command -v runuser >/dev/null 2>&1; then
    env -i PATH=/usr/bin:/bin runuser -u nobody -- "$@"
  else
    return 126
  fi
}

# binary_version FILE : print the version FILE reports for --version
# ("cuemix-web 0.2.0" -> "0.2.0"), run unprivileged. Returns 1 when it does not run or prints
# something else.
binary_version() {
  local out=""
  if [[ -x "$1" ]]; then
    out="$(unprivileged timeout 10 "$1" --version 2>/dev/null || true)"
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

# config_channel : print [update] channel from the config ("stable" by default).
config_channel() {
  local line="" ch=""
  if [[ -r "${CONF_FILE}" ]]; then
    line="$(sed -n '/^[[:space:]]*\[update\][[:space:]]*$/,/^[[:space:]]*\[/ s/^[[:space:]]*channel[[:space:]]*=//p' "${CONF_FILE}" | head -n 1 || true)"
    ch="${line%%#*}"
    ch="${ch//[\" ]/}"
  fi
  case "${ch}" in
    prerelease|pre|alpha|beta) printf 'prerelease' ;;
    *) printf 'stable' ;;
  esac
}

# apply_channel : the config's channel decides --pre unless the command line did. The web
# UI's pre-release switch (update-prefs.json, writable by the web user) is never read here:
# it only changes what the UI offers, and the install it asks for names its tag.
apply_channel() {
  CHANNEL="$(config_channel)"
  if [[ "${PRE}" -eq 0 && "${PIN_SET}" -eq 0 && "${CHANNEL}" == "prerelease" ]]; then
    PRE=1
  fi
  if [[ "${PRE}" -eq 1 ]]; then
    CHANNEL="prerelease"
  fi
}

# json_tag TAG : TAG as a JSON string, or null when empty (TAG is already validated).
json_tag() {
  if [[ -n "$1" ]]; then
    printf '"%s"' "$1"
  else
    printf 'null'
  fi
}

# private_dir : make sure PRIVATE_DIR exists and can be trusted: a real directory (not a
# symlink), owned by the user running this script (root), not writable by anyone else.
private_dir() {
  if [[ ! -e "${PRIVATE_DIR}" && ! -L "${PRIVATE_DIR}" ]]; then
    mkdir -m 0700 -- "${PRIVATE_DIR}" || die "cannot create ${PRIVATE_DIR}"
  fi
  if [[ -L "${PRIVATE_DIR}" || ! -d "${PRIVATE_DIR}" ]]; then
    die "${PRIVATE_DIR} must be a real directory"
  fi
  if [[ "$(stat -c %u -- "${PRIVATE_DIR}")" != "$(id -u)" ]] || (( 0$(stat -c %a -- "${PRIVATE_DIR}") & 022 )); then
    die "${PRIVATE_DIR} must be owned by the user running this script and not writable by others"
  fi
}

# atomic_publish FILE TEXT : publish TEXT as FILE, world-readable. The file is built in
# PRIVATE_DIR (a directory only this script's user can touch, on the same filesystem as the
# web service's state directory), then moved over FILE with mv -fT. FILE's own directory is
# writable by the unprivileged web user, so nothing is ever created, written or chmod-ed
# there by root: a rename replaces whatever sits at FILE (a planted symlink included, which
# is replaced, not followed) and -T refuses to move into a planted directory.
atomic_publish() {
  local file="$1" text="$2" tmp="" dest_dir=""
  private_dir
  # mv between filesystems is a copy, which would write as root inside the web user's
  # directory (following a planted symlink): refuse instead.
  dest_dir="$(dirname -- "${file}")"
  if [[ "$(stat -c %d -- "${PRIVATE_DIR}")" != "$(stat -c %d -- "${dest_dir}")" ]]; then
    die "${PRIVATE_DIR} and ${dest_dir} are on different filesystems; cannot publish ${file} safely"
  fi
  tmp="$(mktemp "${PRIVATE_DIR}/publish.XXXXXX")" || die "cannot create a temporary file in ${PRIVATE_DIR}"
  if ! { printf '%s\n' "${text}" >"${tmp}" && chmod 0644 -- "${tmp}" && mv -fT -- "${tmp}" "${file}"; }; then
    rm -f -- "${tmp}"
    die "cannot publish ${file}"
  fi
}

# write_status INSTALLED AVAILABLE : JSON for the web UI, written atomically,
# world-readable. Every value is a validated version, tag or timestamp, or a fixed word.
# "features" tells the UI what this updater can do (see FEATURES); "releases",
# "latest_stable" and "latest_pre" come from fetch_releases.
write_status() {
  local installed="$1" available="$2" update="false" json=""
  [[ -n "${STATUS_FILE}" ]] || return 0
  [[ "${installed}" =~ ^[0-9A-Za-z.+-]+$ ]] || installed="unknown"
  [[ "${installed}" != "${available}" ]] && update="true"
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    dry "would write ${STATUS_FILE}"
    return 0
  fi
  json="$(printf '{"installed":"%s","latest":"%s","tag":"%s","channel":"%s","update_available":%s,"checked_at":"%s","release_url":"%s/%s","features":[%s],"latest_stable":%s,"latest_pre":%s,"min_version":"v%s","releases":%s}' \
    "${installed}" "${VERSION}" "${TAG}" "${CHANNEL}" "${update}" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${RELEASE_PAGE}" "${TAG}" \
    "${FEATURES}" "$(json_tag "${LATEST_STABLE}")" "$(json_tag "${LATEST_PRE}")" "${FLOOR:-${MIN_VERSION}}" "${RELEASES_JSON}")"
  atomic_publish "${STATUS_FILE}" "${json}"
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

# fetch_releases : the newest published releases (drafts excluded), newest first, for the
# status file. Sets RELEASES_JSON (a JSON array of at most RELEASES_KEEP objects),
# LATEST_STABLE and LATEST_PRE (the newest tag of each kind, or empty). Never fails the
# check: without a list the status carries an empty one and the UI offers no choice.
#
# No jq: the GitHub API pretty-prints, one key per line, with the keys of a release at an
# indent of four spaces and everything nested (author, assets) deeper, so awk reads the
# top-level keys by their indent. Every field is validated again before it is written.
fetch_releases() {
  local json="" rows="" tag="" pre="" published="" published_json="" items="" count=0
  RELEASES_JSON="[]"
  LATEST_STABLE=""
  LATEST_PRE=""
  if ! json="$(curl -fsSL -H 'Accept: application/vnd.github+json' "${API_BASE}/releases?per_page=${RELEASES_FETCH}" 2>/dev/null)"; then
    log_warn "could not list the releases from GitHub; the status will not offer a version choice"
    return 0
  fi
  rows="$(printf '%s\n' "${json}" | awk '
    /^  \{[[:space:]]*$/ { tag = ""; draft = "false"; pre = "false"; pub = ""; next }
    /^    "tag_name": *"/ { v = $0; sub(/^    "tag_name": *"/, "", v); sub(/".*$/, "", v); tag = v; next }
    /^    "draft": *(true|false)/ { draft = ($0 ~ /true/) ? "true" : "false"; next }
    /^    "prerelease": *(true|false)/ { pre = ($0 ~ /true/) ? "true" : "false"; next }
    /^    "published_at": *"/ { v = $0; sub(/^    "published_at": *"/, "", v); sub(/".*$/, "", v); pub = v; next }
    /^  \},?[[:space:]]*$/ { if (tag != "" && draft == "false") printf "%s\t%s\t%s\n", tag, pre, pub; tag = "" }
  ' || true)"
  if [[ -z "${rows}" ]]; then
    log_warn "no release found in the GitHub list; the status will not offer a version choice"
    return 0
  fi
  effective_floor
  while IFS=$'\t' read -r tag pre published; do
    [[ "${tag}" =~ ${TAG_RE} ]] || continue
    tag_pinnable "${tag}" || continue
    if [[ "${pre}" == "true" || "${tag}" == *-* ]]; then
      pre="true"
    else
      pre="false"
    fi
    if [[ "${published}" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]]; then
      published_json="\"${published}\""
    else
      published_json="null"
    fi
    if [[ "${pre}" == "true" ]]; then
      [[ -n "${LATEST_PRE}" ]] || LATEST_PRE="${tag}"
    else
      [[ -n "${LATEST_STABLE}" ]] || LATEST_STABLE="${tag}"
    fi
    if (( count < RELEASES_KEEP )); then
      [[ -z "${items}" ]] || items+=","
      items+="$(printf '{"tag":"%s","version":"%s","prerelease":%s,"published_at":%s}' "${tag}" "${tag#v}" "${pre}" "${published_json}")"
      count=$((count + 1))
    fi
  done <<<"${rows}"
  RELEASES_JSON="[${items}]"
  log_info "listed ${count} releases (latest stable: ${LATEST_STABLE:-none}, latest pre-release: ${LATEST_PRE:-none})"
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

# extract_payload : unpack the payload and set NEW_SRC to the cuemix-web binary. Only that
# one member is unpacked, owned by root, and it must be a regular file (a symlink in a
# tampered archive would make the chmod and the install below act on its target).
extract_payload() {
  local dir="${WORKDIR}/payload"
  mkdir -m 0755 -- "${dir}"
  case "${PAYLOAD_KIND}" in
    tar)
      tar -xzf "${WORKDIR}/${PAYLOAD}" -C "${dir}" --no-same-owner --no-same-permissions -- cuemix-web \
        || die "cannot unpack ${PAYLOAD}"
      NEW_SRC="${dir}/cuemix-web"
      ;;
    deb)
      command -v dpkg-deb >/dev/null 2>&1 || die "dpkg-deb is needed to unpack the .deb payload"
      dpkg-deb -x "${WORKDIR}/${PAYLOAD}" "${dir}" || die "cannot unpack ${PAYLOAD}"
      NEW_SRC="${dir}/usr/bin/cuemix-web"
      ;;
  esac
  if [[ -L "${NEW_SRC}" || ! -f "${NEW_SRC}" ]]; then
    die "release payload ${PAYLOAD} does not contain cuemix-web as a regular file"
  fi
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
  log_info "installed: ${current}   available: ${VERSION} (${TAG}, channel ${CHANNEL})"
  if [[ -n "${STATUS_FILE}" ]]; then
    fetch_releases
  fi
  write_status "${current}" "${VERSION}"
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

# version_at_least TAG FLOOR : TAG (vX.Y.Z[-pre], already validated) is FLOOR (X.Y.Z) or
# newer. A pre-release of the floor itself is older than the floor (semantic versioning).
version_at_least() {
  local tag="${1#v}" floor="${2#v}" core="" pre="" i=0
  local -a a=() b=()
  core="${tag%%-*}"
  [[ "${tag}" == *-* ]] && pre="${tag#*-}"
  IFS=. read -r -a a <<<"${core}"
  IFS=. read -r -a b <<<"${floor}"
  for i in 0 1 2; do
    if (( 10#${a[i]:-0} > 10#${b[i]:-0} )); then return 0; fi
    if (( 10#${a[i]:-0} < 10#${b[i]:-0} )); then return 1; fi
  done
  [[ -z "${pre}" ]]
}

# tag_pinnable TAG : the tag is the floor or newer (FLOOR once effective_floor ran, else the
# built-in MIN_VERSION).
tag_pinnable() {
  version_at_least "$1" "${FLOOR:-${MIN_VERSION}}"
}

# effective_floor : set FLOOR to MIN_VERSION, raised to the newest release's published floor
# (MIN_VERSION_URL) when that one is higher. That file is fetched over HTTPS from the same
# release as the binaries, read at most 64 bytes and must be one plain X.Y.Z; a missing or
# malformed file (releases before 0.7.2 have none) keeps MIN_VERSION. Runs once.
effective_floor() {
  local raw=""
  [[ -z "${FLOOR}" ]] || return 0
  FLOOR="${MIN_VERSION}"
  raw="$(curl -fsSL --max-time 10 --max-filesize 64 "${MIN_VERSION_URL}" 2>/dev/null | head -c 64 | tr -d '[:space:]' || true)"
  raw="${raw#v}"
  if [[ "${raw}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && version_at_least "v${raw}" "${MIN_VERSION}"; then
    FLOOR="${raw}"
  fi
  if [[ "${FLOOR}" != "${MIN_VERSION}" ]]; then
    log_info "the newest release raises the floor: releases older than v${FLOOR} are refused"
  fi
}

# read_request FILE : check the request file the unprivileged web service wrote, and set
# REQUEST_TAG to the release tag it holds ("" = no tag: newest release of the channel).
# The file is untrusted input. It is opened once, by dd, with O_NOFOLLOW and O_NONBLOCK (a
# symlink is refused and a FIFO planted in the meantime cannot block root) under a 5 s
# timeout, and only REQUEST_MAX_BYTES + 1 bytes are ever read. It must hold nothing but a
# tag that matches TAG_RE (surrounding whitespace allowed) and is at least v0.5.0. Anything
# else is refused (return 1), and the log never repeats what the file contained.
read_request() {
  local file="$1" raw=""
  REQUEST_TAG=""
  if [[ -L "${file}" || ! -f "${file}" ]]; then
    log_err "update request refused: it is not a regular file"
    return 1
  fi
  # The trailing x keeps trailing newlines through the command substitution, so the length
  # is the real one. Whatever is not a character a tag may hold becomes "?" (NUL included).
  raw="$({ timeout 5 dd if="${file}" iflag=nofollow,nonblock bs=$((REQUEST_MAX_BYTES + 1)) count=1 status=none 2>/dev/null | LC_ALL=C tr -c 'A-Za-z0-9.\n\r\t -' '?'; } || true; printf x)"
  raw="${raw%x}"
  if (( ${#raw} > REQUEST_MAX_BYTES )); then
    log_err "update request refused: larger than ${REQUEST_MAX_BYTES} bytes"
    return 1
  fi
  raw="${raw#"${raw%%[![:space:]]*}"}"
  raw="${raw%"${raw##*[![:space:]]}"}"
  if [[ -z "${raw}" ]]; then
    return 0
  fi
  if [[ ! "${raw}" =~ ${TAG_RE} ]]; then
    log_err "update request refused: it does not hold exactly one release tag (vX.Y.Z[-pre])"
    return 1
  fi
  if ! tag_pinnable "${raw}"; then
    log_err "update request refused: releases older than v${MIN_VERSION} lack security or serious bug fixes"
    return 1
  fi
  REQUEST_TAG="${raw}"
}

# write_result CODE TAG : "CODE TIMESTAMP [TAG]" for the web UI (exit code of the install).
write_result() {
  local text=""
  text="$1 $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  if [[ -n "$2" ]]; then
    text+=" $2"
  fi
  atomic_publish "${RESULT_FILE}" "${text}"
}

# do_run_request : what cuemix-update-request.service runs. The request file is read,
# validated and removed first (the path unit fires again as long as it exists), then this
# script installs: with the requested tag (a downgrade is allowed), or, for an empty file,
# the newest release of the channel. The tag only ever reaches the child as one argument
# after --version, never through a shell. The exit code goes to RESULT_FILE, and the status
# is refreshed so the UI sees the new version. A refused request installs nothing.
do_run_request() {
  local self="" rc=0 ok=1
  local -a args=()
  self="$(readlink -f -- "${BASH_SOURCE[0]}" 2>/dev/null || true)"
  [[ -n "${self}" && -x "${self}" ]] || self="$0"
  if [[ ! -e "${RUN_REQUEST}" && ! -L "${RUN_REQUEST}" ]]; then
    log_info "no update request at ${RUN_REQUEST}; nothing to do"
    exit 0
  fi
  read_request "${RUN_REQUEST}" || ok=0
  # A directory (or anything rm cannot remove) must not stop the result from being recorded.
  rm -f -- "${RUN_REQUEST}" 2>/dev/null || true
  if [[ "${ok}" -eq 1 ]]; then
    if [[ -n "${REQUEST_TAG}" ]]; then
      args=(--version "${REQUEST_TAG}")
      log_info "update requested from the web UI: ${REQUEST_TAG}"
    else
      log_info "update requested from the web UI: newest release of the channel"
    fi
    "${self}" --wait-lock ${args[@]+"${args[@]}"} || rc=$?
  else
    rc="${EXIT_USAGE}"
  fi
  write_result "${rc}" "${REQUEST_TAG}"
  if [[ -n "${STATUS_FILE}" ]]; then
    "${self}" --wait-lock --check --status-file "${STATUS_FILE}" || true
  fi
  exit 0
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
  effective_floor
  if ! version_at_least "${TAG}" "${FLOOR}"; then
    if [[ "${FORCE}" -eq 0 ]]; then
      fail "${EXIT_USAGE}" "${TAG} is older than v${FLOOR}, the oldest release with every security and serious bug fix; refusing (a root shell can override with --version ${TAG} --force)"
    fi
    log_warn "${TAG} is older than v${FLOOR} and lacks security or serious bug fixes; installing anyway (--force)"
  fi
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
  # The unprivileged --version check must reach the unpacked binary (no listing for others).
  chmod 0711 -- "${WORKDIR}"
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
  trap cleanup EXIT
  parse_args "$@"
  validate_args
  require_root "$@"
  require_commands
  if [[ -n "${RUN_REQUEST}" ]]; then
    do_run_request
  fi
  acquire_lock
  apply_channel

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

# Not when sourced (the tests call the functions above).
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
