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
