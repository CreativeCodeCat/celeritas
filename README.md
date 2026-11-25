# celeritas

A minimal terminal emulator written in Rust. It opens its own window and runs your shell inside it, so it doesn't need another terminal.

## Features

- Runs your `$SHELL` in a pseudo-terminal (PTY) and draws its output in a window
- Handles the common ANSI/VT escape codes: cursor movement, erase, 16 and 256 colours, truecolour, underline and strikethrough
- Scrollback history (mouse wheel, or Shift+PageUp/PageDown) and a separate alternate screen for full-screen programs
- Mouse selection, copied with Ctrl+Shift+C; double-click selects a word, triple-click a line, Alt+drag selects a rectangle
- Scroll regions (`CSI r`), insert and delete line, and reverse index
- Mouse reporting for programs (vim, less): the 1000, 1002 and 1003 modes, with legacy, UTF-8 (1005), SGR (1006) or URXVT (1015) encoding. Hold Shift to select text instead
- Terminal modes for programs: focus reporting (1004), bracketed paste (2004), application cursor keys (1) and cursor visibility (25)
- Combining marks (accents, up to two per character) drawn over their base character
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

Command-line options override the configuration file. The option names are kept stable,
so window-manager rules that match on the window class or title keep working:

    celeritas --title=NAME                  # window title
    celeritas --class=INSTANCE,GENERAL      # window class (X11) / app id (Wayland)
    celeritas -e btop --utf8                # run a program instead of the shell (must be last)

## Install

    make install               # binary to ~/.local/bin/celeritas
    make install-desktop       # also adds an app-menu launcher
    make uninstall             # removes both
    make install PREFIX=/usr/local   # system-wide (may need sudo)

## Configuration

Settings are read from `~/.config/celeritas/celeritas.lua` (or `$XDG_CONFIG_HOME/celeritas/celeritas.lua`). If the file doesn't exist, celeritas creates it with the default settings on first launch. The file is Lua and returns a table of settings grouped into sections (`window`, `font`, `colors`, and so on). Anything you leave out uses the built-in default. A broken file prints an error and falls back to the defaults.

Supported settings:

- `env`: extra environment variables for the shell
- `scrolling.history`: lines of scrollback kept (default 10000; 0 turns it off)
- `scrolling.multiplier`: lines scrolled per mouse-wheel notch (default 3)
- `shell`: the shell to run (default `$SHELL`)
- `window`: `title`, `decorations`, `decorations_theme_variant` (`"Dark"` or `"Light"`), `opacity` (0.0 to 1.0; only the default background becomes see-through), `class` (`instance` and `general`, the names window-manager rules match), and `dimensions.columns` / `lines` (the starting size; a tiling window manager may resize the window)
- `mouse.hide_when_typing`: hide the mouse pointer while typing, and show it again when the mouse moves
- `mouse.bindings`: the middle button, as `{ mouse = "Middle", action = ... }` with action `PasteSelection` (the default), `Paste` or `None`. Selecting text by dragging sets the primary selection
- `selection.save_to_clipboard`: copy a selection to the clipboard as soon as it is dragged out
- `selection.semantic_escape_chars`: characters that end a word for double-click selection (by default, common punctuation and brackets, plus space and tab)
- `cursor.thickness`: the underline and beam thickness as a fraction of the cell (default 0.15)
- `cursor.unfocused_hollow`: draw a block cursor as an outline while the window is unfocused (default true)
- `cursor.style`: `"Block"`, `"Underline"` or `"Beam"`
- `keyboard.bindings`: a list of `{ key, mods, action }` or `{ key, mods, chars }` entries. Actions are Copy, Paste, ScrollPageUp, ScrollPageDown, ScrollLineUp, ScrollLineDown, ScrollToTop, ScrollToBottom and None (swallow the key). `chars` sends the given text. Later entries take precedence over the built-in keys
- `font.size`
- `colors`: `primary.background` / `foreground`, the `normal` and `bright` ANSI colours, and `draw_bold_text_with_bright_colors`
- `general.working_directory`
- `general.live_config_reload`: when the file changes, it is read again within a second and most settings apply at once (default `true`). The font, the window size and whether the window is transparent need a restart. `shell`, `env` and `working_directory` apply to the next shell started

Colours are written as `0xRRGGBB` or `#RRGGBB`. Because the file is Lua code, it runs with your user's permissions.

The default configuration is in [`dist/celeritas.lua`](dist/celeritas.lua).

## Limitations

This is a learning-sized emulator, not a full-featured terminal:

- Italic text uses a real italic face from the system: the normal font's own, or else the first monospace font that has one. Only if the system has no italic monospace face is italic text slanted in software. Set `font.italic` to choose a specific face
- `window.dynamic_padding` is not supported yet, because celeritas has no window padding. It is ignored
- `cursor.vi_mode_style` has no effect, because celeritas has no vi mode
- Other terminal modes are accepted and ignored

## License

celeritas is free software, released under the [GNU General Public License v3.0 or later](LICENSE). You can redistribute it and/or modify it under those terms. It comes with no warranty.
