#!/usr/bin/env bash
# Contract watch (CONCEPT §6): diff upstream main against the newest stable
# tag and flag any change touching sentinel paths (RESEARCH §1). Failure =
# human review before bumping the supported pin.
#
# Sentinel paths:
#   shell/services/PluginRegistry.qml   manifest schema
#   shell/plugins/menu/MenuModel.js     menu-provider protocol
#   bin/omarchy-plugin-*                plugin CLI surface (opb help mirrors it)
#   migrations/*  touching shell.json   storage-rule migrations (-G match)
#   themes/                             theme engine surface
#   default/hypr/bindings/*.lua         keybind catalog source (opb keys)
#
# Exit 0 = clean; exit 1 = contract drift detected (report printed);
# exit 2 = infrastructure error (no network, no tag, ...).
#
# OPB_UPSTREAM_URL overrides the remote — test hook for synthetic fixtures.
# OPB_MAIN_BRANCH overrides the drifting branch (default: upstream's default
# branch, quattro).
set -euo pipefail

REMOTE="${OPB_UPSTREAM_URL:-https://github.com/basecamp/omarchy}"
MAIN_BRANCH="${OPB_MAIN_BRANCH:-quattro}"

die() { echo "upstream-watch: $*" >&2; exit 2; }

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

git init -q "$SCRATCH"
git -C "$SCRATCH" remote add origin "$REMOTE"

# Newest final release tag on the remote. Pre-release tags (-rc/-beta) are
# excluded outright: the stable channel ships finals only (RESEARCH §5), and
# `sort -V` does not implement semver pre-release ordering anyway (it would
# rank v4.0.0-beta3 above v4.0.0 — git.rs's semver-crate logic does not).
TAG="$(git ls-remote --tags "$REMOTE" \
  | awk -F'refs/tags/' 'NF==2 && $2 !~ /\^\{\}$/ {print $2}' \
  | grep -E '^v?[0-9]+\.[0-9]+\.[0-9]+$' \
  | sort -V | tail -n1)"
[ -n "$TAG" ] || die "no semver release tags found at $REMOTE"

git -C "$SCRATCH" fetch -q origin \
  "+refs/tags/$TAG:refs/opb/tag" "+$MAIN_BRANCH:refs/opb/main"

TAGREF=refs/opb/tag
MAINREF=refs/opb/main

report=""
append_log() { # $1 = heading, rest = git log args
  local heading="$1"; shift
  local log
  log="$(git -C "$SCRATCH" log --oneline "$TAGREF..$MAINREF" -- "$@")"
  [ -z "$log" ] && return 0
  report+="$heading"$'\n'"$log"$'\n\n'
}

report+="contract drift between $TAG and branch $MAIN_BRANCH ($REMOTE)"$'\n\n'
append_log "manifest schema (shell/services/PluginRegistry.qml):" \
  shell/services/PluginRegistry.qml
append_log "menu protocol (shell/plugins/menu/MenuModel.js):" \
  shell/plugins/menu/MenuModel.js
append_log "plugin CLI surface (bin/omarchy-plugin-*):" \
  "bin/omarchy-plugin-*"
append_log "storage-rule migrations (migrations/* mentioning shell.json):" \
  -G'shell\.json' migrations/
append_log "theme surface (themes/):" themes/
append_log "keybind catalog source (default/hypr/bindings/*.lua):" \
  "default/hypr/bindings/"

if [ -n "$(echo "$report" | tail -n +3)" ]; then
  report+=$'HUMAN REVIEW REQUIRED before bumping the supported pin (CONCEPT §6 playbook).\n'
  echo "$report"
  exit 1
fi

echo "upstream-watch: clean — no sentinel changes between $TAG and branch $MAIN_BRANCH"
