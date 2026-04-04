# mousefold

`mousefold` is a Linux daemon that grabs one physical mouse, remaps selected mouse button events to a virtual keyboard, and forwards all other mouse movement, wheel, and button events to a virtual mouse.

## Features

- Single-device mouse capture with `grab`
- YAML-based remap rules
- Bluetooth device selection with BlueZ-backed pair / trust / connect
- Separate virtual mouse and virtual keyboard outputs
- Manual `reload` with rollback on invalid config
- Foreground operation for systemd-managed deployments

## Usage

```bash
cargo run -- --config ./config.example.yaml
```

Validate a config without starting the daemon:

```bash
cargo run -- check --config ./config.example.yaml
```

Monitor normalized events without grabbing the device:

```bash
cargo run -- monitor --config ./config.example.yaml
```

Request a running daemon to reload its config:

```bash
cargo run -- reload --config ./config.example.yaml
```

## Requirements

- Linux with `evdev` and `uinput`
- BlueZ / D-Bus when `transport: bluetooth` is used
- Root privileges
- A systemd-based environment for service deployment

## Service

An example unit file is available at `mousefold.service`.
It validates the config with `ExecStartPre`, starts after `bluetooth.service` / `dbus.service`,
and uses `ExecReload` for manual config reload.
