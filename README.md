# Penumbra

Display control for Linux and Wayland, in the tray: a BetterDisplay-style utility.

| BetterDisplay | Penumbra |
|---|---|
| Menu bar app, no Dock icon | Tray icon (StatusNotifierItem) + a panel that drops from the corner |
| DDC brightness / contrast / volume / input / power | Same, through `ddcutil` (DDC/CI) |
| Combined hardware + software brightness | One slider: hardware down to its minimum, then software dimming (gamma ramps) |
| Colour temperature, gamma | Software, per display (`wlr-gamma-control`) |
| Resolution, refresh rate, HiDPI scaling | Mode list, "Looks like" scale list, custom `WxH@Hz` (`wlr-output-management`) |
| Rotation, arrangement, enable/disable | Rotation chips, drag-to-arrange canvas with magnetic edges, turn outputs on/off |
| Virtual screens | Virtual outputs (Umbriel `output-create`), recreated at start |
| Configuration profiles | Profiles: save/apply the whole setup of every display |
| Brightness keys, OSD | `penumbra brightness +5` with an on-screen bar; scroll on the tray icon |
| Built-in display | Backlight through sysfs / logind, synced to external monitors if wanted |

## Install

```
scripts/install.sh          # builds, installs ~/.local/bin/penumbra and a launcher entry
penumbra                    # start (lives in the tray)
penumbra autostart on       # start with the session
```

Needs: `ddcutil` (monitors), `gtk4`, `libadwaita`, `gtk4-layer-shell`, a Wayland compositor with
`wlr-output-management`, `wlr-gamma-control` and `wlr-layer-shell` (tested on Umbriel), and a tray
(StatusNotifierItem watcher: Noctalia, Waybar, KDE...). For DDC, the user needs access to
`/dev/i2c-*` (`i2c-dev` module + the `i2c` group or a udev `uaccess` rule) and DDC/CI switched on
in the monitor's own menu.

## Command line (also what keyboard shortcuts call)

```
penumbra list [--json]
penumbra brightness 50 | +5 | -5 [-d focused|all|internal|external|HDMI-A-1|2|name]
penumbra set contrast 60 | volume +10 | input hdmi2 | mute on | preset srgb | power off | 0x1b 5
penumbra temperature 4500 / gamma 1.1 / reset-colour
penumbra mode 2560x1440@144 / scale 1.5 / rotate 90 / position 1920 0 / vrr on / enable / disable
penumbra profile save Desk | apply Desk | delete Desk | list
penumbra virtual create Stream 2560x1440@60 | destroy Stream
penumbra toggle | show | hide | settings [page] | quit
```

Umbriel keybinds:

```toml
"XF86MonBrightnessUp"   = "spawn:penumbra brightness +5"
"XF86MonBrightnessDown" = "spawn:penumbra brightness -5"
"Mod+D"                 = "spawn:penumbra toggle"
```

Commands go through the running app (D-Bus `dev.alecsh.Penumbra`) when there is one; otherwise a
short-lived instance does the work. Software dimming and colour only last while the app runs.

## Development

```
cargo test                                   # unit tests + a live test on a virtual output (needs Umbriel)
umbriel output-create PMOCK-1 2560x1440@60
PENUMBRA_MOCK=PMOCK-1 PENUMBRA_CONFIG=/tmp/p.json PENUMBRA_EXPAND=1 cargo run   # a fake DDC monitor on it
```

Layout: `src/wl.rs` (Wayland thread), `src/ddc.rs` (ddcutil + mock), `src/hub.rs` (the model all
parts use), `src/commands.rs` (CLI/D-Bus language), `src/ui/` (GTK4/libadwaita).
