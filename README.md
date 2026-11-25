# Celeritas

A minimal terminal emulator written in Rust. It opens its own window and runs your shell in it, so it doesn't need another terminal to run in.

The full reference for command-line options, configuration and key bindings is the man page: `man celeritas` (see [Install](#install)).

## Features

- Runs your `$SHELL` in a pseudo-terminal and draws its output with OpenGL
- Common escape codes: cursor movement, erase, 16 and 256 colours, truecolour, underline and strikethrough
- Scrollback (mouse wheel, or Shift+PageUp/PageDown) and an alternate screen for full-screen programs
- Mouse selection: double-click a word, triple-click a line, Alt+drag for a rectangle. Copy with Ctrl+Shift+C; paste with Ctrl+V
- Vi mode (Ctrl+Shift+Space) for moving and selecting over the scrollback without the mouse
- Mouse reporting for programs such as vim and less (modes 1000, 1002, 1003 with the 1005, 1006 and 1015 encodings). Hold Shift to select text instead
- Bracketed paste, application cursor keys, focus reporting and cursor visibility, for programs that use them
- Wide characters (CJK and most emoji), combining marks, and italics from the font's own italic face
- Answers the terminal queries that shells such as fish send at startup, so they don't time out
- Configured with a Lua file, which is created with the defaults on first launch

## Requirements

- Linux with a Wayland or X11 desktop and OpenGL 3.3
- A Rust toolchain, from [rustup](https://rustup.rs)
- A C compiler (the Lua interpreter is built from source)
- The FreeType and fontconfig development libraries
- `make` (optional, for the install targets)

## Install

    make install                     # binary, man page and app-menu launcher
    make uninstall                   # removes all of the above
    make install PREFIX=/usr/local   # system-wide (may need sudo)

The binary goes to `$PREFIX/bin` (default `~/.local/bin`), the man page to `$PREFIX/share/man/man1` and the launcher to `$PREFIX/share/applications`. `make install-man` and `make install-desktop` install just one of those.

To build without installing, run `cargo run --release`.

Close the window to quit.

## Usage

    celeritas [--title=NAME] [--class=INSTANCE,GENERAL] [-e PROGRAM [ARGS...]]

`--title` sets the window title. `--class` sets the window class, which window managers match rules against. `-e` runs a program instead of the shell and must come last. Command-line options override the configuration file.

## Configuration

Settings are read from `~/.config/celeritas/celeritas.lua` (or `$XDG_CONFIG_HOME/celeritas/celeritas.lua`). If the file doesn't exist, Celeritas creates it with the defaults. It is Lua, and returns a table of settings grouped into sections:

```lua
return {
    window = { opacity = 0.9, padding = { x = 8, y = 8 } },
    font = { size = 12 },
    colors = { primary = { background = "0x1E1E1E", foreground = "0xDDDDDD" } },
    scrolling = { history = 10000 },
}
```

Anything you leave out uses the built-in default. A broken file prints an error and falls back to the defaults. Changes are picked up within a second while Celeritas runs.

The default configuration is in [`dist/celeritas.lua`](dist/celeritas.lua). The man page lists every setting.

## How it works

1. **PTY** (`portable-pty`, `src/pty.rs`): starts your shell on a pseudo-terminal, and a reader thread passes its output to the window.
2. **Parser and screen** (`vte`, `src/screen.rs`): decodes escape sequences into a grid of cells, with scrollback and the cursor. It also builds replies to the shell's queries.
3. **Window and input** (`winit` and `glutin`, `src/app.rs`, `src/input.rs`): creates the window and OpenGL context, and turns key presses and mouse input into bytes for the shell.
4. **Renderer** (`glow`, `src/renderer.rs`): draws the grid each frame from one vertex list, with glyphs in a texture atlas.
5. **Fonts** (`crossfont`, `src/font.rs`): loads the four faces through fontconfig. Characters missing from the font fall back to other installed fonts, which covers Nerd Font icons.
6. **Clipboard** (`copypasta`, `src/clipboard.rs`): the X11 clipboard, which XWayland shares with Wayland applications.
7. **Configuration** (`mlua`, `src/config.rs`): reads the Lua file at startup and again when it changes.

## License

Celeritas is free software, released under the [GNU General Public License v3.0 or later](LICENSE). It comes with no warranty.
