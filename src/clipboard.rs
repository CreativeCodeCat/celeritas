//! The system clipboard, via copypasta.

use copypasta::x11_clipboard::{Primary, X11ClipboardContext};
use copypasta::{ClipboardContext, ClipboardProvider};

pub struct Clipboard {
    provider: Box<dyn ClipboardProvider>,
    /// The X11 primary selection: set by selecting text, pasted with the middle button.
    primary: Option<Box<dyn ClipboardProvider>>,
}

impl Clipboard {
    /// Connect to the clipboard. `None` if no clipboard is available.
    ///
    /// The X11 clipboard is used everywhere on Linux; XWayland makes it visible to Wayland
    /// apps. (copypasta's Wayland clipboard takes the window's display connection and was
    /// observed to crash on exit, so it is not used.)
    pub fn new() -> Option<Clipboard> {
        let provider = ClipboardContext::new().ok()?;
        let primary = X11ClipboardContext::<Primary>::new()
            .ok()
            .map(|p| Box::new(p) as Box<dyn ClipboardProvider>);
        Some(Clipboard { provider: Box::new(provider), primary })
    }

    pub fn get(&mut self) -> Option<String> {
        self.provider.get_contents().ok()
    }

    pub fn set(&mut self, text: String) {
        let _ = self.provider.set_contents(text);
    }

    /// The text of the primary selection, if any.
    pub fn get_primary(&mut self) -> Option<String> {
        self.primary.as_mut()?.get_contents().ok()
    }

    /// Make `text` the primary selection.
    pub fn set_primary(&mut self, text: String) {
        if let Some(primary) = self.primary.as_mut() {
            let _ = primary.set_contents(text);
        }
    }
}
