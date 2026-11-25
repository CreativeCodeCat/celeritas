# celeritas

A minimal terminal emulator written in Rust. It opens its own window and runs your shell inside it, so it doesn't need another terminal.

## Features

- Runs your `$SHELL` in a pseudo-terminal (PTY) and draws its output in a window
- Handles the common ANSI/VT escape codes: cursor movement, erase, 16 and 256 colours, truecolour, underline and strikethrough
- Scrollback history (mouse wheel, or Shift+PageUp/PageDown) and a separate alternate screen for full-screen programs
- Mouse selection, copied with Ctrl+Shift+C
- Wide characters (CJK and most emoji) take two columns
- Answers the terminal queries shells send at startup (for example, fish's device-attributes probe)
- Paste with Ctrl-V; Ctrl-C is sent to the shell as an interrupt
- Configured with a Lua file: window size and title, font size, colours, shell, working directory and environment variables

## How it works

1. **PTY** (`portable-pty`, `src/main.rs`): spawns your shell attached to a pseudo-terminal.
2. **Reader thread** (`src/main.rs`): reads the shell's output and sends it over a channel to the UI.
3. **Parser** (`vte`, `src/screen.rs`): decodes escape sequences and updates the screen model.
4. **Screen** (`src/screen.rs`): a grid of cells plus the cursor. It also builds replies to the shell's queries.
5. **Window and input** (`winit` + `glutin`, `src/app.rs`, `src/input.rs`): creates the window and OpenGL context, turns key presses, mouse input and clipboard shortcuts into bytes for the PTY, and handles selection and scrolling.
6. **Renderer** (`glow` / OpenGL, `src/renderer.rs`): draws the grid each frame from one vertex list, with glyphs in a texture atlas.
7. **Fonts** (`crossfont`, `src/font.rs`): loads the four faces (normal, bold, italic, bold italic) through fontconfig and rasterises glyphs. Missing characters fall back to other installed fonts, which covers Nerd Font icons.
8. **Clipboard** (`copypasta`, `src/clipboard.rs`): the X11 clipboard, which XWayland shares with Wayland apps.
9. **Config** (`mlua`, `src/config.rs`): reads the Lua configuration at startup.

## Requirements

- Linux with a Wayland or X11 desktop and OpenGL 3.3 support
- A Rust toolchain (`cargo`), from [rustup](https://rustup.rs)
- A C compiler (the Lua interpreter is built from source)
- The FreeType and fontconfig development libraries (used by crossfont)
- `make` (optional, for the install targets)

## Build and run

    cargo run --release

Close the window to quit.

## Install

    make install               # binary to ~/.local/bin/celeritas
    make install-desktop       # also adds an app-menu launcher
    make uninstall             # removes both
    make install PREFIX=/usr/local   # system-wide (may need sudo)

## Configuration

Settings are read from `~/.config/celeritas/celeritas.lua` (or `$XDG_CONFIG_HOME/celeritas/celeritas.lua`). If the file doesn't exist, celeritas creates it with the default settings on first launch. The file is Lua and returns a table laid out like Alacritty's config. Anything you leave out uses the built-in default. A broken file prints an error and falls back to the defaults.

Supported settings:

- `env`: extra environment variables for the shell
- `scrolling.history`: lines of scrollback kept (default 10000; 0 turns it off)
- `shell`: the shell to run (default `$SHELL`)
- `window`: `title`, `decorations`, and `dimensions.columns` / `lines` (the starting size; a tiling window manager may resize the window)
- `font.size`
- `colors`: `primary.background` / `foreground`, the `normal` and `bright` ANSI colours, and `draw_bold_text_with_bright_colors`
- `general.working_directory`

Colours are written as `0xRRGGBB` or `#RRGGBB`. Because the file is Lua code, it runs with your user's permissions.

The default configuration is in [`dist/celeritas.lua`](dist/celeritas.lua).

## Limitations

This is a learning-sized emulator, not a replacement for kitty or alacritty:

- No scroll regions, and no mouse reporting to programs (the mouse only scrolls and selects)
- Selection is by character cells only (no word or line selection); wrapped lines are copied with a newline
- Bold text is drawn in the bright colours, not with a bold font
- Italic and dim text are not drawn differently; combining marks are dropped
- Unsupported Alacritty options (key bindings, cursor style, opacity, font families) are ignored

## License

celeritas is free software, released under the [GNU General Public License v3.0 or later](LICENSE). You can redistribute it and/or modify it under those terms. It comes with no warranty.
