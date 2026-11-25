//! The system clipboard.
//!
//! On Wayland the Wayland clipboard is used, because XWayland does not reliably copy
//! Wayland clipboard contents into the X11 clipboard. Text copied in a native Wayland
//! app (a browser, for example) was invisible to an X11-only paste. The X11 clipboard is
//! still set as well, so X11 applications see what Celeritas copies, and it is the
//! fallback when there is no Wayland display.

use std::io::Read;

use copypasta::x11_clipboard::{Primary, X11ClipboardContext};
use copypasta::{ClipboardContext, ClipboardProvider};
use wl_clipboard_rs::copy::{ClipboardType as CopyType, MimeType as CopyMime, Options, Source};
use wl_clipboard_rs::paste::{self, ClipboardType as PasteType, MimeType, Seat};

pub struct Clipboard {
    provider: Box<dyn ClipboardProvider>,
    /// The X11 primary selection: set by selecting text, pasted with the middle button.
    primary: Option<Box<dyn ClipboardProvider>>,
    /// Whether a Wayland display is in use, so the Wayland clipboard comes first.
    wayland: bool,
}

impl Clipboard {
    /// Connect to the clipboard. `None` if no clipboard is available.
    pub fn new() -> Option<Clipboard> {
        let provider = ClipboardContext::new().ok()?;
        let primary = X11ClipboardContext::<Primary>::new()
            .ok()
            .map(|p| Box::new(p) as Box<dyn ClipboardProvider>);
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        Some(Clipboard { provider: Box::new(provider), primary, wayland })
    }

    pub fn get(&mut self) -> Option<String> {
        if self.wayland {
            if let Some(text) = wayland_get(PasteType::Regular) {
                return Some(text);
            }
        }
        self.provider.get_contents().ok()
    }

    pub fn set(&mut self, text: String) {
        if self.wayland {
            wayland_set(PasteType::Regular, text.clone());
        }
        let _ = self.provider.set_contents(text);
    }

    /// The text of the primary selection, if any.
    pub fn get_primary(&mut self) -> Option<String> {
        if self.wayland {
            if let Some(text) = wayland_get(PasteType::Primary) {
                return Some(text);
            }
        }
        self.primary.as_mut()?.get_contents().ok()
    }

    /// Make `text` the primary selection.
    pub fn set_primary(&mut self, text: String) {
        if self.wayland {
            wayland_set(PasteType::Primary, text.clone());
        }
        if let Some(primary) = self.primary.as_mut() {
            let _ = primary.set_contents(text);
        }
    }
}

/// The text of a Wayland clipboard, or `None` if it is empty or unavailable.
fn wayland_get(clipboard: PasteType) -> Option<String> {
    let (mut pipe, _mime) = paste::get_contents(clipboard, Seat::Unspecified, MimeType::Text).ok()?;
    let mut text = String::new();
    pipe.read_to_string(&mut text).ok()?;
    Some(text)
}

/// Offer `text` on a Wayland clipboard. Another thread serves the contents until another
/// application takes the selection, so the terminal does not have to keep answering.
fn wayland_set(clipboard: PasteType, text: String) {
    let copy_type = match clipboard {
        PasteType::Regular => CopyType::Regular,
        PasteType::Primary => CopyType::Primary,
    };
    std::thread::spawn(move || {
        let mut options = Options::new();
        options.clipboard(copy_type).foreground(true);
        let source = Source::Bytes(text.into_bytes().into_boxed_slice());
        let _ = options.copy(source, CopyMime::Text);
    });
}
