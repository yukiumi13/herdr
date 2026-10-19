# Herdr minimap fork

Last updated on 2026-10-19 at commit `12c7fa51` (the accepted-frame implementation).

This independent Linux alpha extends Herdr v0.8.2 with a configurable command-output section
between Spaces and Agents. It supports ANSI colours, wheel zoom, drag, popup clicks, and an
accepted-frame identifier for providers such as [PR Map](https://github.com/yukiumi13/pr-map).

## Install

Build from the `pr-minimap` branch. Requirements: Rust 1.96.1, a C toolchain, and Zig **0.15.2**
on PATH (or set `ZIG` to its executable). The repository includes its terminal-library source.

```sh
git clone --branch pr-minimap https://github.com/yukiumi13/herdr.git herdr-minimap
cd herdr-minimap
cargo build --release --locked
install -m 755 target/release/herdr "$HOME/.local/bin/herdr-minimap"
herdr-minimap --version
```

The version is `0.8.2-minimap.alpha1`. The official `herdr update` mechanism is disabled in this
fork, including background binary checks. To update, pull this branch and rebuild. To return to
official Herdr, install its binary separately and remove the sidebar panel section from its config.
Installing this fork under a different name preserves an existing official binary. It still uses
the normal Herdr configuration/session locations; test in a separate named session first.

## Configure a panel

Merge this section into the Herdr config. This example uses fictional offline PRs and agents:

```toml
[ui.sidebar.panel]
title = "pr map"
command = "pr-map --demo --minimap"
rows = 12
interval_seconds = 5
timeout_seconds = 5
click_command = "pr-map --demo --popup"
click_width = "95%"
click_height = "90%"
zoom_levels = 3
zoom = 1
```

Install PR Map first; its README covers live workspace configuration and full-map controls.
Provider commands receive `COLUMNS`, `LINES`, `HERDR_ACTIVE_PANE_ID`, `HERDR_PANEL_ZOOM`,
`HERDR_PANEL_DRAG_X/Y`, and a unique `HERDR_PANEL_FRAME_ID`. A hidden panel does not run or
schedule a busy-loop deadline. Output is capped at 64 KiB. A failed command retains the old
output and marks the section `stale`.

When Herdr accepts successful output, it retains its frame id and dimensions. A popup click gets
that id and `HERDR_PANEL_COLUMN/LINE/WIDTH/HEIGHT`. Providers should save immutable hit data under
the frame id. A failed refresh retains the previous id. After resizing, coordinates are omitted
until a frame matching the new body geometry is accepted. The header opens the generic popup.

## Validation scope

The sidebar regression suite covers hidden-panel scheduling, failure retention, accepted frame
identity, SGR output, layout, scrolling, zoom and drag. A disposable native terminal test verifies
that a real click receives the displayed frame id and resolves the expected fictional station.
The PR Map repository tests cross-view click isolation and full-screen navigation.

The fork CI checks formatting, clippy, sidebar/command regressions and config/UI architecture on
Linux. This alpha does not claim stable-release performance or macOS/Windows qualification.
Upstream Herdr's architecture, license, contribution rules and documentation remain available;
this branch is independently maintained and is not an upstream release.
