# omarchy-plugin-bridge

**`opb`** — run upstream [omarchy-shell](https://github.com/basecamp/omarchy) (Quickshell QML) on a raw Arch + Hyprland system, component by component, without an Omarchy install.

Omarchy ships a first-class shell — bar, panels, notifications, launcher, plugin ecosystem — but assumes it owns your whole desktop. `opb` is a thin bridge for people who already have a Hyprland setup and want to opt in on their own terms.

## How it works

- **Pinned, not forked** — clones upstream at a release tag; updates swap an immutable checkout via symlink flip; `opb update rollback` undoes any bump instantly
- **Bridge, not reimplementation** — no QML here; plugins and shell operations delegate to upstream's own tools at the pinned version
- **Upstream writes its own state** — after bootstrap, every plugin mutation flows through upstream over IPC; opb touches `shell.json` only in the update down-window
- **Minimal invasiveness** — zero keybinds by default, all-off shell, marker-scoped artifacts you can delete by hand

## Install

```sh
git clone https://github.com/Qurupeco01/omarchy-plugin-bridge.git
cd omarchy-plugin-bridge
cargo install --path .
```

Requirements: Arch + Hyprland, `quickshell ≥ 0.3.1`, `hyprctl`, `git`. Run `opb status` to verify.

## Quickstart

```sh
opb bootstrap          # clone + pin the newest stable omarchy, generate all-off shell.json
opb enable --now       # wire session autostart (asks before touching hyprland.lua) + start the shell
opb plugin list        # see what exists; everything starts off
opb plugin enable omarchy.clock    # something appears on your bar
opb keys import-suggested          # optional: pick suggested binds for what you enabled
```

Reboot with `require("opb")` in place and the shell comes back on its own.

## Commands

| Command | What it does |
|---|---|
| `opb bootstrap [--ref TAG] [--redo]` | Clone + pin upstream (newest stable tag by default), generate all-off `shell.json` |
| `opb enable [--now] [\--yes\|\--no-line]` | Install session wiring; activation line written on consent |
| `opb disable [--now]` | Remove session wiring (`keys.lua` stays yours) |
| `opb up` / `opb down` | Start / stop the shell now |
| `opb status` | Dependencies, pin state, generations, channel distance — start here when anything looks off |
| `opb update [--ref TAG] [--rename OLD=NEW]` | Preview → confirm → flip to a newer pin, reconciling `shell.json` |
| `opb update rollback` | Flip back one generation |
| `opb plugin add URL [\--yes]` | Install a third-party plugin (upstream's flow) |
| `opb plugin enable/disable ID` | Toggle components (upstream IPC; conflicts between running apps are not analyzed — that's yours) |
| `opb plugin list` | Read-only x-ray: manifests × storage rules, works headless |
| `opb keys set ACTION COMBO` | Bind a shell action (refuses occupied combos) |
| `opb keys import-suggested` | Bulk-pick upstream's suggested binds |

## Updating

```sh
opb update             # preview first, nothing happens until you confirm
```

Pin bumps never patch the checkout: a fresh clone is validated, the symlink flips, and first-party ids are reconciled against the new release (renames need explicit `--rename old=new`). The previous generation stays on disk for `rollback`. Upstream migrations never run on your machine — package-level requirements surface in `opb status` instead.

## Scope

`opb` deliberately does **not**: analyze conflicts with other desktop apps, manage packages, or carry patches to upstream. If upstream's contract changes in ways the bridge can't absorb, the pin holds until it's resolved — safe by design.
