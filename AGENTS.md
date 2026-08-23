# AGENTS.md — omarchy-plugin-bridge

## What this repo is

`opb`: thin Rust CLI that runs upstream [omarchy-shell](https://github.com/basecamp/omarchy) (Quickshell QML) on a raw Arch + Hyprland system, pinned to an upstream ref. Bridge-only: bootstrap/pin/env/wiring/conflicts/select/update. Never re-implements Omarchy.

**Prime directive — minimal invasiveness**: nothing on the user's machine changes unless explicitly selected. Measure every change against this.

## Layout

```
src/        Rust crate
design/     Product docs - follow for any implementations
```

## Code organization

Follow good Rust practices and keep the code well organized — small modules with a clear purpose, split when something earns its own home rather than by upfront doctrine:

- Keep decision logic (shell.json rules, conflict matching, pin math) separate from side effects (subprocesses, fs) wherever natural — pure functions are trivial to test
- Effects stay thin: subprocess/fs helpers do one thing, return `Result`, never panic
- Errors via `anyhow` with context at boundaries
- No async/tokio (D3) — fully sync
- Atomic writes (tmp file + rename) whenever state lands on disk — delegate to `atomicwrites` (fsync + parent-dir fsync added in our wrapper)
- **Prefer standard or well-known, well-maintained crates over custom hand-rolled implementations for common operations** (e.g. `atomicwrites` for atomic writes, `which` for PATH lookup). Hand-roll only what no sane crate covers (e.g. the D9 symlink flip is plain std — no crate does it); if deviating, document the reason in the code.

## Hard rules

- **No QML, no reimplementation**: shell out to upstream `bin/omarchy` + helpers; see CONCEPT §5 anti-duplication policy.
- **Pin discipline**: upstream checkout is immutable; updates = new dir + symlink flip (D9). Never carry private patches to the checkout.
- **Never touch the live system** (`~/.config/hypr`, `~/.config/omarchy`, `~/.local/state`, user sessions, installed packages) unless explicitly asked. Scratch files → `/tmp/opencode`.
- **Contracts over READMEs**: derive upstream behavior from code; shipped READMEs have documented nonexistent features before (RESEARCH §1).
- One roadmap phase at a time; do not start phase N+1 before N's acceptance test passes.
- Verify before done: run the relevant tests/acceptance commands, never mark work complete on intent alone.

## Commands (Fill here as the list grows with the project)

```
cargo build
cargo test
cargo clippy   # must stay clean
```
