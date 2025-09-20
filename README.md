# slarti

A Zed-inspired infra manager GUI.

- **slarti**: GPUI-based UI with a simple split layout (scrolling terminal pane). Terminal uses `portable-pty` + `alacritty_terminal` (engine available; demo renders raw text from PTY).
- **slarti-remote**: headless JSON-over-stdio daemon for directory listing to run over SSH.
- **slarti-ssh**: client that launches `slarti-remote` over `ssh -T` and exchanges JSON.
- **slarti-proto**: shared protocol types.

## Build

```bash
cargo build --workspace
```

> If GPUI API changes, pin a specific commit for `gpui` and `alacritty_terminal` in `Cargo.toml`.
