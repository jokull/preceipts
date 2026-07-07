//! v0 theme: One Dark-adjacent, hardcoded. A theme system comes later.

use preceipts_core::HIGHLIGHT_NAMES;

pub const BG: u32 = 0x1e2227;
pub const BG_PANEL: u32 = 0x23272e;
pub const BG_HEADER: u32 = 0x2c313a;
pub const FG: u32 = 0xc8ccd4;
pub const FG_MUTED: u32 = 0x7f848e;
pub const ADD_BG: u32 = 0x203527;
pub const ADD_BG_WORD: u32 = 0x2e5738;
pub const DEL_BG: u32 = 0x3a262a;
pub const DEL_BG_WORD: u32 = 0x5c3238;
pub const EMPTY_BG: u32 = 0x22262b;
pub const ACCENT: u32 = 0x61afef;
pub const GREEN: u32 = 0x98c379;
pub const RED: u32 = 0xe06c75;
pub const YELLOW: u32 = 0xe5c07b;
pub const FIND_BG: u32 = 0x4d3d14;
pub const FIND_CURRENT_BG: u32 = 0x6b5410;
pub const BORDER: u32 = 0x181b1f;

/// Map a tree-sitter highlight kind (index into HIGHLIGHT_NAMES) to a
/// foreground color.
pub fn syntax_color(kind: Option<u8>) -> u32 {
    let Some(name) = kind.and_then(|k| HIGHLIGHT_NAMES.get(k as usize)) else {
        return FG;
    };
    let family = name.split('.').next().unwrap_or(name);
    match family {
        "keyword" => 0xc678dd,
        "string" => 0x98c379,
        "comment" => 0x5c6370,
        "function" | "constructor" => 0x61afef,
        "type" | "module" | "label" => 0xe5c07b,
        "number" | "constant" => 0xd19a66,
        "property" | "attribute" | "tag" => 0xe06c75,
        "operator" => 0x56b6c2,
        "punctuation" => 0x9da5b4,
        "variable" => match *name {
            "variable.builtin" => 0xe06c75,
            "variable.parameter" | "variable.member" => 0xd19a66,
            _ => FG,
        },
        _ => FG,
    }
}
