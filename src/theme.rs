//! Color themes: the four Catppuccin flavors (https://catppuccin.com/palette).
//!
//! Each flavor is a palette of named colors; [`Colors`] maps them to what the
//! UI paints, so drawing code never names a palette color directly.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Latte,
    Frappe,
    Macchiato,
    #[default]
    Mocha,
}

impl Theme {
    /// Lightest to darkest, the order Catppuccin lists them in.
    pub const ALL: [Theme; 4] = [Theme::Latte, Theme::Frappe, Theme::Macchiato, Theme::Mocha];

    pub fn label(self) -> &'static str {
        match self {
            Theme::Latte => "Catppuccin Latte",
            Theme::Frappe => "Catppuccin Frappé",
            Theme::Macchiato => "Catppuccin Macchiato",
            Theme::Mocha => "Catppuccin Mocha",
        }
    }

    pub fn colors(self) -> Colors {
        let p = match self {
            Theme::Latte => &LATTE,
            Theme::Frappe => &FRAPPE,
            Theme::Macchiato => &MACCHIATO,
            Theme::Mocha => &MOCHA,
        };
        // Own bubbles are the background tinted blue, like Telegram's. Less
        // tint on the light flavor keeps dark text readable on it.
        let tint = if p.dark { 0.3 } else { 0.2 };
        let own_bubble = mix(p.base, p.blue, tint);
        Colors {
            bg: p.base,
            fg: p.text,
            subtle: p.subtext0,
            muted: p.overlay1,
            border: p.overlay0,
            accent: p.lavender,
            selection: p.surface0,
            popup_bg: p.mantle,
            primary: p.blue,
            highlighted: p.peach,
            insert: p.green,
            error: p.red,
            warning: p.yellow,
            search: p.yellow,
            command: p.mauve,
            reply: p.teal,
            activity: p.blue,
            edit: p.peach,
            attach: p.blue,
            success: p.green,
            own_bubble,
            own_meta: mix(p.text, p.blue, 0.5),
            other_bubble: p.surface0,
            other_meta: p.subtext0,
            // Reactions are pills a shade off their bubble; yours are filled
            // in, as in Telegram.
            own_reaction: mix(own_bubble, p.text, 0.15),
            other_reaction: mix(p.surface0, p.text, 0.15),
            your_reaction: p.blue,
            // Telegram's seven name colors, in Catppuccin's shades.
            names: [p.red, p.peach, p.mauve, p.green, p.teal, p.blue, p.pink],
            // Dark on light in every flavor: not every scanner reads an
            // inverted code.
            qr_dark: MOCHA.mantle,
            qr_light: LATTE.base,
        }
    }
}

/// What the UI paints, by role.
#[derive(Clone, Copy)]
pub struct Colors {
    pub bg: Color,
    pub fg: Color,
    /// Secondary text: chat previews, photo placeholders.
    pub subtle: Color,
    /// Least important text: key hints, date separators, placeholders.
    pub muted: Color,
    /// Unfocused pane borders.
    pub border: Color,
    /// Focused borders, cursors, popups.
    pub accent: Color,
    /// Background of the highlighted row in a list.
    pub selection: Color,
    pub popup_bg: Color,
    /// Unread badges, the NORMAL label, the Saved Messages title.
    pub primary: Color,
    /// Titles of chats you highlighted with `H`.
    pub highlighted: Color,
    /// The INSERT label.
    pub insert: Color,
    pub error: Color,
    pub warning: Color,
    /// Behind text matching a `/` search, and the SEARCH label.
    pub search: Color,
    /// The COMMAND label.
    pub command: Color,
    /// The reply bar over the composer, and the marker on the message it answers.
    pub reply: Color,
    /// "typing…" and the like, in the chat list and the chat's title.
    pub activity: Color,
    /// The "Edit message" bar over the composer, and the marker on the
    /// message being edited.
    pub edit: Color,
    /// Files waiting in the composer to be sent, and the ATTACH label.
    pub attach: Color,
    /// Toasts saying something worked.
    pub success: Color,
    pub own_bubble: Color,
    /// Time and send status on own bubbles.
    pub own_meta: Color,
    pub other_bubble: Color,
    pub other_meta: Color,
    /// Behind a reaction on own bubbles, and on others'.
    pub own_reaction: Color,
    pub other_reaction: Color,
    /// Behind a reaction you added, and the emoji you added in the `R` popup.
    pub your_reaction: Color,
    /// Sender names in groups, picked by sender id, and the squares standing
    /// in for missing chat photos, by Telegram's accent color id (0 red … 6 pink).
    pub names: [Color; 7],
    /// The QR code on the login screen.
    pub qr_dark: Color,
    pub qr_light: Color,
}

/// The part of a Catppuccin palette the UI uses.
struct Palette {
    dark: bool,
    red: Color,
    peach: Color,
    yellow: Color,
    green: Color,
    teal: Color,
    blue: Color,
    lavender: Color,
    mauve: Color,
    pink: Color,
    text: Color,
    subtext0: Color,
    overlay1: Color,
    overlay0: Color,
    surface0: Color,
    base: Color,
    mantle: Color,
}

const LATTE: Palette = Palette {
    dark: false,
    red: rgb(0xd20f39),
    peach: rgb(0xfe640b),
    yellow: rgb(0xdf8e1d),
    green: rgb(0x40a02b),
    teal: rgb(0x179299),
    blue: rgb(0x1e66f5),
    lavender: rgb(0x7287fd),
    mauve: rgb(0x8839ef),
    pink: rgb(0xea76cb),
    text: rgb(0x4c4f69),
    subtext0: rgb(0x6c6f85),
    overlay1: rgb(0x8c8fa1),
    overlay0: rgb(0x9ca0b0),
    surface0: rgb(0xccd0da),
    base: rgb(0xeff1f5),
    mantle: rgb(0xe6e9ef),
};

const FRAPPE: Palette = Palette {
    dark: true,
    red: rgb(0xe78284),
    peach: rgb(0xef9f76),
    yellow: rgb(0xe5c890),
    green: rgb(0xa6d189),
    teal: rgb(0x81c8be),
    blue: rgb(0x8caaee),
    lavender: rgb(0xbabbf1),
    mauve: rgb(0xca9ee6),
    pink: rgb(0xf4b8e4),
    text: rgb(0xc6d0f5),
    subtext0: rgb(0xa5adce),
    overlay1: rgb(0x838ba7),
    overlay0: rgb(0x737994),
    surface0: rgb(0x414559),
    base: rgb(0x303446),
    mantle: rgb(0x292c3c),
};

const MACCHIATO: Palette = Palette {
    dark: true,
    red: rgb(0xed8796),
    peach: rgb(0xf5a97f),
    yellow: rgb(0xeed49f),
    green: rgb(0xa6da95),
    teal: rgb(0x8bd5ca),
    blue: rgb(0x8aadf4),
    lavender: rgb(0xb7bdf8),
    mauve: rgb(0xc6a0f6),
    pink: rgb(0xf5bde6),
    text: rgb(0xcad3f5),
    subtext0: rgb(0xa5adcb),
    overlay1: rgb(0x8087a2),
    overlay0: rgb(0x6e738d),
    surface0: rgb(0x363a4f),
    base: rgb(0x24273a),
    mantle: rgb(0x1e2030),
};

const MOCHA: Palette = Palette {
    dark: true,
    red: rgb(0xf38ba8),
    peach: rgb(0xfab387),
    yellow: rgb(0xf9e2af),
    green: rgb(0xa6e3a1),
    teal: rgb(0x94e2d5),
    blue: rgb(0x89b4fa),
    lavender: rgb(0xb4befe),
    mauve: rgb(0xcba6f7),
    pink: rgb(0xf5c2e7),
    text: rgb(0xcdd6f4),
    subtext0: rgb(0xa6adc8),
    overlay1: rgb(0x7f849c),
    overlay0: rgb(0x6c7086),
    surface0: rgb(0x313244),
    base: rgb(0x1e1e2e),
    mantle: rgb(0x181825),
};

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// `a` moved `amount` (0 to 1) of the way towards `b`.
fn mix(a: Color, b: Color, amount: f32) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return a;
    };
    let channel =
        |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * amount).round() as u8;
    Color::Rgb(channel(ar, br), channel(ag, bg), channel(ab, bb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mocha_is_the_default() {
        assert_eq!(Theme::default(), Theme::Mocha);
        assert_eq!(Theme::Mocha.colors().bg, Color::Rgb(0x1e, 0x1e, 0x2e));
    }

    #[test]
    fn own_bubbles_are_tinted_between_base_and_blue() {
        let bubble = Theme::Mocha.colors().own_bubble;
        // 30% of the way from base #1e1e2e to blue #89b4fa.
        assert_eq!(bubble, Color::Rgb(0x3e, 0x4b, 0x6b));
    }
}
