# celeritas

A minimal terminal emulator in Rust, with its own window (no host terminal needed).

How it works:

1. **PTY** (`portable-pty`): spawns your `$SHELL` inside a pseudo-terminal so it behaves as if it were attached to a real terminal.
2. **Reader thread**: reads the shell's output and sends it over a channel.
3. **Parser** (`vte`): decodes ANSI/VT escape sequences (cursor moves, colours, erase, ...).
4. **Screen** (`src/screen.rs`): a grid of cells + cursor that the parser writes into. It also answers the shell's terminal queries (device attributes, cursor position).
5. **Window** (`eframe`/`egui`, `src/gui.rs`): draws the grid every frame and translates key presses and clipboard paste into bytes written back to the PTY. Ctrl-C copies nothing; it sends the interrupt byte to the shell.

## Run

    cargo run --release

Or install it (targets are listed in the Makefile header):

    make install-desktop

Close the window to quit.

## Configuration

Settings live in `~/.config/celeritas/celeritas.lua` (or `$XDG_CONFIG_HOME/celeritas/`). The file is Lua and returns a table laid out like Alacritty's config. Anything missing uses the built-in default. Supported settings:

- `env`: extra environment variables for the shell
- `shell`: the shell to run (default `$SHELL`)
- `window`: `title`, `decorations`, `dimensions.columns`/`lines` (the starting size; a tiling window manager may resize it)
- `font.size`
- `colors`: `primary.background`/`foreground`, `normal`/`bright` ANSI colours, `draw_bold_text_with_bright_colors`
- `general.working_directory`

Other Alacritty options (scrollback, key bindings, cursor style, opacity, font families) are not supported.

## Limitations

This is a learning-sized emulator, not a replacement for kitty/alacritty:

- No scrollback, no scroll regions, no mouse support, no text selection
- Bold is shown as bright colours, not a bold font
- Fonts come from egui's built-in set, so some symbol glyphs (e.g. Nerd Font icons) show as boxes
- Wide (CJK/emoji) characters are treated as one column wide
- The alternate screen is simulated (cleared), not kept as a separate buffer
- Only the common SGR colour/style codes are supported
