#!/usr/bin/env bash
# Contract watch (CONCEPT §6): diff upstream's newest stable tag against its
# default branch on sentinel paths (RESEARCH §1). Failure = human review
# before bumping the supported pin.
#
# Sentinel paths:
#   shell/services/PluginRegistry.qml   manifest schema
#   shell/plugins/menu/MenuModel.js     menu-provider protocol
#   bin/omarchy-plugin-*                plugin CLI surface (opb help mirrors it)
#   migrations/*  mentioning shell.json storage-rule migrations
#   themes/                             theme engine surface
#   default/hypr/bindings/*.lua         keybind catalog source (opb keys)
#
# The comparison is between TREES, not commit logs: upstream rewrites its
# branch history between releases, which would otherwise resurface already-
# shipped commits as phantom drift.
#
# Exit 0 = clean; exit 1 = contract drift detected (report printed);
# exit 2 = infrastructure error (no network, no tag, ...).
#
# OPB_UPSTREAM_URL overrides the remote — test hook for synthetic fixtures.
# OPB_MAIN_BRANCH overrides the drifting branch (default: quattro).
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

drift=0
report="contract drift between $TAG and branch $MAIN_BRANCH ($REMOTE)"$'\n\n'

flag_paths() { # $1 = heading, rest = pathspecs — tree diff, history-proof
  local heading="$1"; shift
  local stat
  # --stat-count keeps asset-heavy surfaces (themes/) from flooding the
  # report: first files listed, total still shown.
  stat="$(git -C "$SCRATCH" diff --stat --stat-count=12 "$TAGREF" "$MAINREF" -- "$@")"
  if [ -n "$stat" ]; then
    drift=1
    report+="$heading"$'\n'"$stat"$'\n\n'
  fi
}

# Storage-rule migrations: added/changed files whose new content mentions
# shell.json (deleted ones are gone ids — reconcile handles them silently).
flag_migrations() {
  local hits=""
  while IFS= read -r f; do
    if git -C "$SCRATCH" show "$MAINREF:$f" 2>/dev/null | grep -q 'shell\.json'; then
      hits+="  $f"$'\n'
    fi
  done < <(git -C "$SCRATCH" diff --name-only --diff-filter=d \
    "$TAGREF" "$MAINREF" -- migrations/)
  if [ -n "$hits" ]; then
    drift=1
    report+="storage-rule migrations (migrations/* mentioning shell.json):"$'\n'"$hits"$'\n'
  fi
}

flag_paths "manifest schema (shell/services/PluginRegistry.qml):" \
  shell/services/PluginRegistry.qml
flag_paths "menu protocol (shell/plugins/menu/MenuModel.js):" \
  shell/plugins/menu/MenuModel.js
flag_paths "plugin CLI surface (bin/omarchy-plugin-*):" "bin/omarchy-plugin-*"
flag_migrations
flag_paths "theme surface (themes/):" themes/
flag_paths "keybind catalog source (default/hypr/bindings/*.lua):" \
  "default/hypr/bindings/"

if [ "$drift" -eq 1 ]; then
  report+=$'HUMAN REVIEW REQUIRED before bumping the supported pin.\n'
  echo "$report"
  exit 1
fi

echo "upstream-watch: clean — no sentinel changes between $TAG and branch $MAIN_BRANCH"
