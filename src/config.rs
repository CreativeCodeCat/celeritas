//! User configuration, read from a Lua file.
//!
//! The file lives at `$XDG_CONFIG_HOME/celeritas/celeritas.lua` (or
//! `~/.config/celeritas/celeritas.lua`). It must return a table with the same
//! layout as Alacritty's TOML config. Anything left out uses the built-in default.
//! Note that the file is Lua code, so it runs with your user's permissions.

use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use mlua::{Lua, Table};

/// The config written on first launch. It is the same as `Config::default()`.
const DEFAULT_CONFIG: &str = include_str!("../dist/celeritas.lua");

/// An 8-bit RGB colour.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Clone, PartialEq, Debug)]
pub struct Colors {
    pub foreground: Rgb,
    pub background: Rgb,
    /// The eight basic ANSI colours (black, red, green, yellow, blue, magenta, cyan, white).
    pub normal: [Rgb; 8],
    /// The eight bright ANSI colours, in the same order.
    pub bright: [Rgb; 8],
    /// Show bold text in the bright colours.
    pub draw_bold_text_with_bright_colors: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Config {
    pub title: String,
    pub decorations: bool,
    pub columns: usize,
    pub lines: usize,
    pub font_size: f32,
    pub colors: Colors,
    /// Shell to run. `None` means `$SHELL`.
    pub shell: Option<String>,
    /// Directory the shell starts in. `None` means the directory celeritas was started from.
    pub working_directory: Option<PathBuf>,
    /// Extra environment variables for the shell.
    pub env: Vec<(String, String)>,
    /// Lines of scrollback kept above the screen. 0 turns scrollback off.
    pub scrollback: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            title: "celeritas".to_string(),
            decorations: true,
            columns: 80,
            lines: 24,
            font_size: 14.0,
            colors: Colors {
                foreground: Rgb(0xdd, 0xdd, 0xdd),
                background: Rgb(0x1e, 0x1e, 0x1e),
                normal: [
                    Rgb(0, 0, 0),
                    Rgb(205, 49, 49),
                    Rgb(13, 188, 121),
                    Rgb(229, 229, 16),
                    Rgb(36, 114, 200),
                    Rgb(188, 63, 188),
                    Rgb(17, 168, 205),
                    Rgb(229, 229, 229),
                ],
                bright: [
                    Rgb(102, 102, 102),
                    Rgb(241, 76, 76),
                    Rgb(35, 209, 139),
                    Rgb(245, 245, 67),
                    Rgb(59, 142, 234),
                    Rgb(214, 112, 214),
                    Rgb(41, 184, 219),
                    Rgb(255, 255, 255),
                ],
                draw_bold_text_with_bright_colors: true,
            },
            shell: None,
            working_directory: None,
            env: Vec::new(),
            scrollback: crate::screen::DEFAULT_HISTORY,
        }
    }
}

impl Config {
    /// Load the user's config. If there is none, write the default one first; a broken
    /// file prints an error and uses defaults, so the terminal always starts.
    pub fn load() -> Config {
        let Some(path) = config_path() else {
            return Config::default();
        };
        if !path.exists() {
            if let Err(e) = write_default(&path) {
                eprintln!("celeritas: could not create {}: {e}", path.display());
            }
            return Config::default();
        }
        match load_file(&path) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("celeritas: could not load {}: {e}\nusing default settings", path.display());
                Config::default()
            }
        }
    }
}

/// `$XDG_CONFIG_HOME/celeritas/celeritas.lua`, falling back to `~/.config/...`.
fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("celeritas").join("celeritas.lua"))
}

/// Write the default config, without overwriting anything already there.
fn write_default(path: &PathBuf) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(DEFAULT_CONFIG.as_bytes())
}

fn load_file(path: &PathBuf) -> Result<Config, Box<dyn Error>> {
    parse(&fs::read_to_string(path)?)
}

/// Run the config source and read its settings.
fn parse(source: &str) -> Result<Config, Box<dyn Error>> {
    let lua = Lua::new();
    let root: Table = lua.load(source).eval()?;

    let mut config = Config::default();

    if let Some(env) = root.get::<Option<Table>>("env")? {
        for pair in env.pairs::<String, String>() {
            config.env.push(pair?);
        }
    }

    if let Some(window) = root.get::<Option<Table>>("window")? {
        if let Some(title) = window.get::<Option<String>>("title")? {
            config.title = title;
        }
        if let Some(decorations) = window.get::<Option<bool>>("decorations")? {
            config.decorations = decorations;
        }
        if let Some(dims) = window.get::<Option<Table>>("dimensions")? {
            if let Some(columns) = dims.get::<Option<usize>>("columns")? {
                config.columns = columns.max(1);
            }
            if let Some(lines) = dims.get::<Option<usize>>("lines")? {
                config.lines = lines.max(1);
            }
        }
    }

    if let Some(font) = root.get::<Option<Table>>("font")? {
        if let Some(size) = font.get::<Option<f32>>("size")? {
            config.font_size = size.max(1.0);
        }
    }

    if let Some(colors) = root.get::<Option<Table>>("colors")? {
        apply_colors(&colors, &mut config.colors)?;
    }

    if let Some(scrolling) = root.get::<Option<Table>>("scrolling")? {
        if let Some(history) = scrolling.get::<Option<usize>>("history")? {
            config.scrollback = history;
        }
    }

    if let Some(general) = root.get::<Option<Table>>("general")? {
        if let Some(dir) = general.get::<Option<String>>("working_directory")? {
            config.working_directory = Some(PathBuf::from(dir));
        }
    }

    if let Some(shell) = root.get::<Option<String>>("shell")? {
        config.shell = Some(shell);
    }

    Ok(config)
}

fn apply_colors(colors: &Table, out: &mut Colors) -> Result<(), Box<dyn Error>> {
    if let Some(primary) = colors.get::<Option<Table>>("primary")? {
        if let Some(bg) = primary.get::<Option<String>>("background")? {
            out.background = parse_hex(&bg)?;
        }
        if let Some(fg) = primary.get::<Option<String>>("foreground")? {
            out.foreground = parse_hex(&fg)?;
        }
    }
    for (section, slots) in [("normal", &mut out.normal), ("bright", &mut out.bright)] {
        let Some(table) = colors.get::<Option<Table>>(section)? else {
            continue;
        };
        const NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
        for (i, name) in NAMES.iter().enumerate() {
            if let Some(hex) = table.get::<Option<String>>(*name)? {
                slots[i] = parse_hex(&hex)?;
            }
        }
    }
    if let Some(flag) = colors.get::<Option<bool>>("draw_bold_text_with_bright_colors")? {
        out.draw_bold_text_with_bright_colors = flag;
    }
    Ok(())
}

/// Parse `0xRRGGBB` or `#RRGGBB`.
fn parse_hex(s: &str) -> Result<Rgb, Box<dyn Error>> {
    let digits = s.strip_prefix("0x").or_else(|| s.strip_prefix('#')).unwrap_or(s);
    if digits.len() != 6 {
        return Err(format!("invalid colour {s:?}, expected 0xRRGGBB or #RRGGBB").into());
    }
    let n = u32::from_str_radix(digits, 16).map_err(|_| format!("invalid colour {s:?}"))?;
    Ok(Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_matches_builtin_defaults() {
        assert_eq!(parse(DEFAULT_CONFIG).unwrap(), Config::default());
    }

    #[test]
    fn empty_table_gives_defaults() {
        assert_eq!(parse("return {}").unwrap(), Config::default());
    }

    #[test]
    fn reads_alacritty_style_layout() {
        let config = parse(
            r##"
            local nord = { bg = "0x2E3440" }
            return {
              env = { TERM = "xterm-256color" },
              shell = "/bin/zsh",
              general = { working_directory = "/tmp" },
              window = { title = "mine", decorations = false, dimensions = { columns = 100, lines = 30 } },
              font = { size = 10 },
              scrolling = { history = 500 },
              colors = {
                primary = { background = nord.bg, foreground = "#D8DEE9" },
                normal = { red = "0xBF616A" },
                bright = { cyan = "0x8FBCBB" },
                draw_bold_text_with_bright_colors = false,
              },
            }
            "##,
        )
        .unwrap();
        assert_eq!(config.title, "mine");
        assert_eq!(config.scrollback, 500);
        assert!(!config.decorations);
        assert_eq!((config.columns, config.lines), (100, 30));
        assert_eq!(config.font_size, 10.0);
        assert_eq!(config.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(config.working_directory, Some(PathBuf::from("/tmp")));
        assert_eq!(config.env, vec![("TERM".to_string(), "xterm-256color".to_string())]);
        assert_eq!(config.colors.background, Rgb(0x2E, 0x34, 0x40));
        assert_eq!(config.colors.foreground, Rgb(0xD8, 0xDE, 0xE9));
        assert_eq!(config.colors.normal[1], Rgb(0xBF, 0x61, 0x6A));
        assert_eq!(config.colors.bright[6], Rgb(0x8F, 0xBC, 0xBB));
        assert!(!config.colors.draw_bold_text_with_bright_colors);
    }

    #[test]
    fn bad_colour_is_an_error() {
        assert!(parse(r#"return { colors = { primary = { background = "blue" } } }"#).is_err());
    }

    #[test]
    fn syntax_error_is_an_error() {
        assert!(parse("return {").is_err());
    }
}
