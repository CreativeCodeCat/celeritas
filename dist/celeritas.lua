-- celeritas configuration.
--
-- This file is Lua: it must return a table. Anything you leave out uses the
-- built-in default. Colours are "0xRRGGBB" or "#RRGGBB".
--
-- Note: this is code, so it runs with your user's permissions.

return {
  env = {},

  -- Shell to run (default: $SHELL).
  -- shell = "/usr/bin/fish",

  window = {
    title = "Celeritas",
    decorations = true,
    dimensions = {
      columns = 80,
      lines = 24,
    },
    -- Window opacity, from 0.0 (transparent) to 1.0 (opaque).
    opacity = 1.0,
  },

  cursor = {
    -- "Block", "Underline" or "Beam".
    style = "Block",
  },

  font = {
    size = 14,
  },

  -- Key bindings. An entry either runs an action or sends characters (chars).
  -- Actions: Copy, Paste, ScrollPageUp, ScrollPageDown, ScrollLineUp,
  -- ScrollLineDown, ScrollToTop, ScrollToBottom, None (swallow the key).
  -- Modifiers: Control, Shift, Alt, joined with "|". Later entries win.
  -- keyboard = {
  --   bindings = {
  --     { key = "V", mods = "Control|Shift", action = "Paste" },
  --     { key = "L", mods = "Control", chars = "\\f" },
  --   },
  -- },

  scrolling = {
    -- Lines of scrollback kept above the screen (0 turns it off).
    history = 10000,
  },

  colors = {
    primary = {
      background = "0x1E1E1E",
      foreground = "0xDDDDDD",
    },

    normal = {
      black   = "0x000000",
      red     = "0xCD3131",
      green   = "0x0DBC79",
      yellow  = "0xE5E510",
      blue    = "0x2472C8",
      magenta = "0xBC3FBC",
      cyan    = "0x11A8CD",
      white   = "0xE5E5E5",
    },

    bright = {
      black   = "0x666666",
      red     = "0xF14C4C",
      green   = "0x23D18B",
      yellow  = "0xF5F543",
      blue    = "0x3B8EEA",
      magenta = "0xD670D6",
      cyan    = "0x29B8DB",
      white   = "0xFFFFFF",
    },

    draw_bold_text_with_bright_colors = true,
  },

  general = {
    -- Directory the shell starts in (default: where celeritas was started).
    working_directory = nil,
  },
}
