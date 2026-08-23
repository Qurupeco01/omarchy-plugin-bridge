# omarchy-plugin-bridge

**`opb`**: run upstream [omarchy-shell](https://github.com/basecamp/omarchy) (Quickshell QML) on a raw Arch + Hyprland system, without an Omarchy install.

## Aim

Omarchy ships a first-class shell: bar, panels, notifications, launcher, plugin ecosystem. But it assumes it owns your whole desktop. `opb` is a thin bridge for people who already have a Hyprland setup and want to adopt omarchy-shell **component by component**:

- **Bootstrap & pin** — clones omarchy at a pinned ref; updates are new dir + symlink flip, instantly rollbackable
- **Opt-in everything** — generates a complete all-off `shell.json`; you enable only what you select (clock, notifications, launcher…). Nothing changes on your system until you choose it
- **Zero modified keybinds by default** —  collisions checked against your live config, add keybindings only if you explicitly ask
- **Conflict-aware** — detects existing notification daemons, polkit agents, etc., and stays out of their way
- **Bridge, not fork** — no QML is written here; plugins, themes, and CLI operations delegate to upstream's own tools at the pinned version

Status: **under construction**
