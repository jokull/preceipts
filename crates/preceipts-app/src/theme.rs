//! Colors for the diff surface.
//!
//! GPUI draws its own widgets, so there is no system palette to inherit —
//! every color here is a decision. Kept in one place so the theme pass has a
//! single seam to work through rather than a hunt through render code.

use gpui::{rgb, Hsla};
use preceipts_core::highlight::HIGHLIGHT_NAMES;

pub struct Theme {
    pub background: Hsla,
    pub surface: Hsla,
    pub text: Hsla,
    pub text_dim: Hsla,
    pub gutter: Hsla,
    pub border: Hsla,
    pub added_bg: Hsla,
    pub removed_bg: Hsla,
    pub added_emphasis: Hsla,
    pub removed_emphasis: Hsla,
    pub header_bg: Hsla,
    /// Behind selected text. Replaces a row's added/removed tint rather than
    /// blending with it — two translucent layers over syntax colours is how a
    /// selection stops being legible.
    pub selection: Hsla,
    /// For a check that has not run. Deliberately not red: an unrecorded
    /// check and a failing one are different facts, and this project's whole
    /// claim is that a receipt means something.
    pub pending: Hsla,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: rgb(0x1c1c1e).into(),
            surface: rgb(0x232326).into(),
            text: rgb(0xe4e4e7).into(),
            text_dim: rgb(0x71717a).into(),
            gutter: rgb(0x52525b).into(),
            border: rgb(0x333338).into(),
            added_bg: rgb(0x14301f).into(),
            removed_bg: rgb(0x3a1a1e).into(),
            added_emphasis: rgb(0x1f5c37).into(),
            removed_emphasis: rgb(0x6e2229).into(),
            header_bg: rgb(0x2a2a2e).into(),
            pending: rgb(0x3f3f46).into(),
            selection: rgb(0x2f4f7f).into(),
        }
    }
}

impl Theme {
    /// Map a highlight capture kind to a color, resolving by dot-separated
    /// family so an unknown leaf (`function.macro`) still themes off its root
    /// (`function`) instead of falling back to plain text.
    pub fn syntax(&self, kind: Option<u8>) -> Hsla {
        let Some(name) = kind.and_then(|k| HIGHLIGHT_NAMES.get(k as usize).copied()) else {
            return self.text;
        };
        let family = name.split('.').next().unwrap_or(name);
        match family {
            "comment" => rgb(0x6b7280).into(),
            "keyword" => rgb(0xc084fc).into(),
            "string" => rgb(0x86efac).into(),
            "number" | "constant" => rgb(0xfbbf24).into(),
            "function" | "constructor" => rgb(0x60a5fa).into(),
            "type" => rgb(0x5eead4).into(),
            "property" | "variable" | "label" | "module" => rgb(0xe4e4e7).into(),
            "attribute" | "tag" => rgb(0xf9a8d4).into(),
            "operator" | "punctuation" => rgb(0x9ca3af).into(),
            _ => self.text,
        }
    }
}

/// The monospace family to render code in, chosen from what is installed.
///
/// This is not a style preference. `SF Mono` is the obvious name to ask for
/// and it is the one name that does not work: macOS ships it as the *private*
/// family `.SF NS Mono`, so `font_family("SF Mono")` matches nothing and the
/// text system quietly falls back to a proportional face. A diff set in body
/// text is the single loudest thing wrong with the window, and nothing in the
/// code says so — the name looks right.
///
/// So ask the text system what exists and take the first we like. `Menlo`
/// ships with every macOS, which is why it anchors the list rather than
/// sitting in it as one more hopeful guess.
pub fn code_font(cx: &gpui::App) -> gpui::SharedString {
    const PREFERRED: [&str; 5] = [
        "JetBrains Mono",
        "IBM Plex Mono",
        "SF Mono",
        "Menlo",
        "Monaco",
    ];

    // Cached, and the caching is not an optimization — it is the difference
    // between a window that draws and one that does not.
    //
    // `all_font_names()` enumerates every font installed on the machine.
    // Measured on this one: **317–344ms per call**. Called from a `render`, as
    // the lab panel's log pane did, that is three frames a second — which is
    // exactly what "the GUI is super slow" looks like from the outside. The
    // answer cannot change while the process runs, so ask once.
    static CHOSEN: std::sync::OnceLock<gpui::SharedString> = std::sync::OnceLock::new();
    CHOSEN
        .get_or_init(|| {
            let available = cx.text_system().all_font_names();
            for name in PREFERRED {
                if available.iter().any(|found| found == name) {
                    return name.into();
                }
            }
            "monospace".into()
        })
        .clone()
}
