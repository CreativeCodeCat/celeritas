//! The shell running in a pseudo-terminal, and the thread that reads its output.

use std::error::Error;
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use winit::event_loop::EventLoopProxy;

use crate::config::Config;

/// What the reader thread sends back.
pub enum PtyMsg {
    Data(Vec<u8>),
    Closed,
}

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<PtyMsg>,
}

impl Pty {
    /// Start the shell. `wake` is poked whenever output arrives, so the event loop redraws.
    pub fn spawn(config: &Config, cols: u16, rows: u16, wake: EventLoopProxy<()>) -> Result<Pty, Box<dyn Error>> {
        let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;

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

        let mut reader = pair.master.try_clone_reader()?;
        let (tx, rx) = mpsc::channel::<PtyMsg>();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                let msg = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => PtyMsg::Closed,
                    Ok(n) => PtyMsg::Data(buf[..n].to_vec()),
                };
                let closed = matches!(msg, PtyMsg::Closed);
                if tx.send(msg).is_err() {
                    break;
                }
                let _ = wake.send_event(());
                if closed {
                    break;
                }
            }
        });

        let writer = pair.master.take_writer()?;
        Ok(Pty { master: pair.master, writer, child, rx })
    }

    /// The next message from the shell, if one is waiting.
    pub fn try_recv(&self) -> Result<PtyMsg, TryRecvError> {
        self.rx.try_recv()
    }

    /// Send keystrokes or replies to the shell.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
