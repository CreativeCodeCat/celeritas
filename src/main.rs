//! celeritas: a small terminal emulator with its own window.
//!
//! The user's shell runs inside a pseudo-terminal (PTY). Its output is parsed by `vte`
//! into a virtual `Screen`, which is drawn with OpenGL (`renderer`) in a window made by
//! `winit`. Keystrokes and mouse input from the window are forwarded back to the shell.

mod app;
mod clipboard;
mod config;
mod font;
mod input;
mod pty;
mod renderer;
mod screen;

use std::error::Error;
use std::path::PathBuf;

use winit::event_loop::EventLoop;

use app::App;
use config::Config;
use pty::Pty;

fn main() -> Result<(), Box<dyn Error>> {
    let mut config = Config::load();
    if let Err(e) = config.apply_args(std::env::args().skip(1)) {
        eprintln!("celeritas: {e}");
        std::process::exit(2);
    }
    let event_loop = EventLoop::<()>::with_user_event().build()?;
    let pty = Pty::spawn(&config, config.columns as u16, config.lines as u16, event_loop.create_proxy())?;
    // Developer aid: save one frame to this path (binary PPM) and exit.
    let screenshot = std::env::var_os("CELERITAS_SCREENSHOT").map(PathBuf::from);

    let mut app = App::new(config, pty, screenshot);
    event_loop.run_app(&mut app)?;
    Ok(())
}
