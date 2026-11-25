//! celeritas: a small terminal emulator with its own window.
//!
//! The user's shell runs inside a pseudo-terminal (PTY). Its output is parsed
//! by `vte` and drawn into a virtual `Screen`, which is shown in a window with
//! `egui`. Keystrokes from the window are forwarded back into the PTY.

mod config;
mod gui;
mod screen;

use std::io::Read;
use std::sync::mpsc;
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

/// Messages from the PTY reader thread to the UI.
pub enum PtyMsg {
    Data(Vec<u8>),
    Closed,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = config::Config::load();

    // 1. Create the pseudo-terminal pair. The window resizes it to fit on the first frame.
    let pty = native_pty_system();
    let size = PtySize {
        rows: config.lines as u16,
        cols: config.columns as u16,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = pty.openpty(size)?;

    // 2. Start the user's shell attached to the PTY slave side.
    let shell = config
        .shell
        .clone()
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".to_string());
    let mut cmd = CommandBuilder::new(shell);
    cmd.env("TERM", "xterm-256color");
    for (key, value) in &config.env {
        cmd.env(key, value);
    }
    match &config.working_directory {
        Some(dir) => cmd.cwd(dir),
        None => {
            if let Ok(cwd) = std::env::current_dir() {
                cmd.cwd(cwd);
            }
        }
    }
    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave); // must be dropped so we see EOF when the shell exits

    // 3. Reader thread: PTY output -> channel (blocking reads stay off the UI thread).
    let mut reader = pair.master.try_clone_reader()?;
    let (tx, rx) = mpsc::channel::<PtyMsg>();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(PtyMsg::Closed);
                    break;
                }
                Ok(n) => {
                    if tx.send(PtyMsg::Data(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // 4. Writer for keystrokes going into the shell.
    let writer = pair.master.take_writer()?;

    // 5. Open the window and run the UI until it is closed or the shell exits.
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(config.title.clone())
            .with_app_id("celeritas")
            .with_decorations(config.decorations)
            .with_inner_size([800.0, 500.0]), // resized to the configured grid on the first frame
        ..Default::default()
    };
    let master = pair.master;
    eframe::run_native(
        "celeritas",
        options,
        Box::new(move |_cc| Ok(Box::new(gui::Terminal::new(config, rx, master, writer, child)))),
    )?;
    Ok(())
}
