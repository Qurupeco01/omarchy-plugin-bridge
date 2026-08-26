# omarchy-plugin-bridge

**`opb`** is a project done to run upstream [omarchy-shell](https://github.com/basecamp/omarchy) (Quickshell QML) on a raw Arch + Hyprland system, component by component, without an Omarchy install, minimally invasive to your existing setup.

Note that this was developed for personal use and is provided as-is. I would love to hear about any feature requests or issues you encounter.

Omarchy ships a first-class shell: bar, panels, notifications, launcher, plugin ecosystem, but assumes it owns your whole desktop. `opb` is a thin bridge for people who already have a Hyprland setup and want to opt in on their own terms.

## How it works

- **Upstream version pinned** — clones upstream at a release tag; updates swap an immutable checkout via symlink flip; `opb update rollback` undoes any bump instantly. Using source for simplicity, planning to support a package manager in the future.
- **Bridge, there is no reimplementation of shell** — no QML here; plugins and shell operations delegate to upstream's own tools at the pinned version.
- **Upstream writes its own state** — after bootstrap, every plugin mutation flows through upstream over IPC; opb touches `shell.json` only in the update down-window.
- **Minimal invasiveness** — zero keybinds by default, all-off shell, empty bar by default, marker-scoped artifacts you can delete by hand. Just touches a single line in `~/.config/hypr/hyprland.lua` to wire autostart, and only with your consent.

## Caveats / Next planned features

Caveats today:

- Upstream is pinned to **release tags only** — tracking `main` or arbitrary refs is not supported yet, as well as other channels cannot be used.
- Currently only the default omarchy bar is supported through running the whole shell, so when you enable opb, the bar appears always on your screen. Individual plugin integration inside custom bars is not supported but planned, maybe this would require more manual setup.
- `opb` installs only from source; packaging (AUR) is planned

Planned / want to try:

- Theme support aligned with upstream's theme system, expandable to user's quickshell theming maybe? I need to think a bit more about this
- Launching *your own* quickshell configs integrating plugins inside them, not only the omarchy one: this would allow using individual plugins from the marketplace in your own setups. 
- Related with previous point: installing plugins without cloning upstream, just having a thin translator.
- Getting omarchy shell from AUR/pacman/main branch (as original allows with channels)
- Waybar bridge (idk if this will be possible at all)

## Install

```sh
# prebuilt binary, no Rust toolchain needed (x86_64 Linux)
curl -fsSL https://raw.githubusercontent.com/Qurupeco01/omarchy-plugin-bridge/main/install.sh | sh

# or via cargo
cargo install --git https://github.com/Qurupeco01/omarchy-plugin-bridge

# or from a local clone, e.g. if you want to read the code first
git clone https://github.com/Qurupeco01/omarchy-plugin-bridge.git
cd omarchy-plugin-bridge
cargo install --path . --locked
```

The curl script installs into `~/.local/bin`, cargo into `~/.cargo/bin` — make
sure the target directory is on your `PATH`. Both are per-user installs.
(AUR package: planned — see Caveats.)

Requirements: Arch + Hyprland, `quickshell ≥ 0.3.1`, `hyprctl`, `git`. Run `opb status` to verify.

## Quickstart

```sh
opb bootstrap          # clone + pin the newest stable omarchy, generate all-off shell.json
opb enable --now       # wire session autostart (asks before touching hyprland.lua) + start the shell
opb plugin list        # see what exists; everything starts off
opb plugin enable omarchy.clock    # something appears on your bar
opb keys import-suggested          # optional: pick suggested binds for what you enabled
```

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

`opb` deliberately does **not**: analyze conflicts with other desktop apps, manage packages, or carry patches to upstream. If upstream's contract changes in ways the bridge can't absorb, the pin holds until it's resolved. 

The aim for the tool is to stay minimal and practical. I hope you enjoy it!! :)
