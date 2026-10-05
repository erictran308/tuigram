//! Color themes. A theme is a TOML file: a palette of sixteen colors, from
//! which every color the UI paints is worked out, and optionally colors for
//! single things (`[colors]`) and a theme to start from (`inherits`). The
//! built-in ones are in `themes/` in the repository; the user's go in
//! `themes/` in the data directory, where a file named like a built-in theme
//! replaces it.
//!
//! [`Colors`] maps a palette to what the UI paints, by role, so drawing code
//! never names a palette color directly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ratatui::style::Color;
use serde::Deserialize;

use crate::text;

/// The theme used when the settings name none, or one that can't be used.
pub const DEFAULT: &str = "mocha";

/// The built-in themes by name, in the order the `?` popup lists them:
/// Catppuccin's flavors from lightest to darkest, then the others.
const BUILT_IN: [(&str, &str); 9] = [
    ("latte", include_str!("../themes/latte.toml")),
    ("frappe", include_str!("../themes/frappe.toml")),
    ("macchiato", include_str!("../themes/macchiato.toml")),
    ("mocha", include_str!("../themes/mocha.toml")),
    ("tokyonight", include_str!("../themes/tokyonight.toml")),
    ("dracula", include_str!("../themes/dracula.toml")),
    ("gruvbox", include_str!("../themes/gruvbox.toml")),
    ("nord", include_str!("../themes/nord.toml")),
    ("rose-pine", include_str!("../themes/rose-pine.toml")),
];

/// The palette's colors. A theme that inherits none sets them all.
const PALETTE: [&str; 16] = [
    "bg", "bg_alt", "surface", "overlay", "comment", "subtext", "fg", "red", "orange", "yellow",
    "green", "cyan", "blue", "purple", "pink", "accent",
];

/// How many themes deep `inherits` may go, so a circle of them ends.
const MAX_INHERITS: usize = 8;

/// The QR code on the login screen is dark on light in every theme: not
/// every scanner reads an inverted code.
const QR_DARK: Color = rgb(0x181825);
const QR_LIGHT: Color = rgb(0xeff1f5);

/// A theme file as written.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    /// What the `?` popup calls the theme; its file name otherwise.
    name: Option<String>,
    /// The theme this one starts from, so it lists only what's different.
    inherits: Option<String>,
    #[serde(default)]
    palette: BTreeMap<String, String>,
    #[serde(default)]
    colors: toml::Table,
}

/// A theme to pick in the `?` popup.
pub struct Theme {
    /// What `settings.toml` calls it: the file name without `.toml`.
    pub id: String,
    pub label: String,
    /// Its colors, or why the file can't be used.
    pub colors: Result<Colors, String>,
}

/// Every theme: the built-in ones, then the user's by label.
pub struct Themes {
    pub list: Vec<Theme>,
    /// Where the user's themes were read from.
    pub dir: Option<PathBuf>,
}

impl Themes {
    /// Only the built-in themes.
    pub fn built_in() -> Self {
        Self::new(BTreeMap::new(), None)
    }

    /// The built-in themes and the `.toml` files in `dir`. A file that can't
    /// be used is listed too, with why.
    pub fn load(dir: &Path) -> Self {
        let mut user = BTreeMap::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let file = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| parse(&text));
            user.insert(id.to_string(), file);
        }
        Self::new(user, Some(dir.to_path_buf()))
    }

    fn new(user: BTreeMap<String, Result<ThemeFile, String>>, dir: Option<PathBuf>) -> Self {
        let built_in = BUILT_IN.map(|(id, text)| {
            let file = parse(text).unwrap_or_else(|e| panic!("themes/{id}.toml: {e}"));
            (id, file)
        });
        let files = Files { built_in, user };
        // A file named like a built-in theme takes its place in the list,
        // and its name unless it has one of its own.
        let mut list: Vec<Theme> = files
            .built_in
            .iter()
            .map(|(id, file)| {
                let own = files.user.get(*id);
                let label = own
                    .and_then(|own| own.as_ref().ok()?.name.as_ref())
                    .or(file.name.as_ref());
                files.theme(id, label, own.is_some())
            })
            .collect();
        let mut own: Vec<Theme> = files
            .user
            .iter()
            .filter(|(id, _)| !BUILT_IN.iter().any(|(built_in, _)| built_in == id))
            .map(|(id, file)| {
                let label = file.as_ref().ok().and_then(|file| file.name.as_ref());
                files.theme(id, label, true)
            })
            .collect();
        own.sort_by_key(|theme| theme.label.to_lowercase());
        list.extend(own);
        Self { list, dir }
    }

    /// The colors of the theme called `id`, or why they can't be used.
    pub fn colors(&self, id: &str) -> Result<Colors, String> {
        match self.list.iter().find(|theme| theme.id == id) {
            Some(theme) => theme.colors.clone(),
            None => Err(format!("there is no theme called {:?}", text::clean(id))),
        }
    }
}

/// The theme files, before `inherits` is followed.
struct Files {
    built_in: [(&'static str, ThemeFile); BUILT_IN.len()],
    /// The user's by file name without `.toml`, or why they can't be read.
    user: BTreeMap<String, Result<ThemeFile, String>>,
}

impl Files {
    /// The theme called `id`, from the user's file if `user`.
    fn theme(&self, id: &str, label: Option<&String>, user: bool) -> Theme {
        let colors = self
            .merged(id, user, 0)
            .and_then(|file| colors(&file).map_err(|e| format!("{id}.toml: {e}")));
        // The user's file names and errors quoting their files are shown.
        Theme {
            id: id.to_string(),
            label: text::clean(label.map_or(id, String::as_str)),
            colors: colors.map_err(|e| text::clean(&e)),
        }
    }

    /// The file for the theme called `id`: the user's if `user` and there is
    /// one, else the built-in one.
    fn file(&self, id: &str, user: bool) -> Option<Result<&ThemeFile, String>> {
        if user && let Some(file) = self.user.get(id) {
            return Some(file.as_ref().map_err(|e| format!("{id}.toml: {e}")));
        }
        let (_, file) = self.built_in.iter().find(|(built_in, _)| *built_in == id)?;
        Some(Ok(file))
    }

    /// The theme called `id` with what it inherits filled in. `user` is false
    /// to skip the user's files, so a file can build on the built-in theme
    /// it replaces.
    fn merged(&self, id: &str, user: bool, depth: usize) -> Result<ThemeFile, String> {
        let Some(file) = self.file(id, user) else {
            return Err(format!("there is no theme called {id:?}"));
        };
        let file = file?;
        let Some(parent) = &file.inherits else {
            return Ok(file.clone());
        };
        let user = user && parent != id;
        if self.file(parent, user).is_none() {
            return Err(format!(
                "{id}.toml: there is no theme called {parent:?} to inherit"
            ));
        }
        if depth == MAX_INHERITS {
            return Err(format!("{id}.toml: inherits goes round in a circle"));
        }
        let mut merged = self.merged(parent, user, depth + 1)?;
        merged.palette.extend(file.palette.clone());
        merged.colors.extend(file.colors.clone());
        Ok(merged)
    }
}

/// Reads a theme file, or says what's wrong with it and on which line.
fn parse(text: &str) -> Result<ThemeFile, String> {
    toml::from_str(text).map_err(|e: toml::de::Error| {
        let line = e
            .span()
            .and_then(|span| text.get(..span.start))
            .map(|before| before.matches('\n').count() + 1);
        match line {
            Some(line) => format!("line {line}: {}", e.message()),
            None => e.message().to_string(),
        }
    })
}

/// Works out every color the UI paints from a theme file, with what it
/// inherits filled in.
fn colors(file: &ThemeFile) -> Result<Colors, String> {
    let mut palette = BTreeMap::new();
    for (name, value) in &file.palette {
        if !PALETTE.contains(&name.as_str()) {
            return Err(format!("the palette has no color called {name:?}"));
        }
        let color = hex(value)
            .ok_or_else(|| format!("palette.{name} is {value:?}, not a color like \"#1e1e2e\""))?;
        palette.insert(name.as_str(), color);
    }
    let missing: Vec<&str> = PALETTE
        .into_iter()
        .filter(|name| !palette.contains_key(name))
        .collect();
    if !missing.is_empty() {
        return Err(format!("the palette needs {}", missing.join(", ")));
    }
    let p = |name: &str| palette[name];
    // What `[colors]` sets: a palette color, a "#rrggbb" one, or the
    // terminal's own.
    let pick = |name: &str, value: &toml::Value| {
        let color = value.as_str().and_then(|value| match value {
            "reset" => Some(Color::Reset),
            _ => palette.get(value).copied().or_else(|| hex(value)),
        });
        color.ok_or_else(|| {
            format!("colors.{name} is {value}, not a palette color, \"#rrggbb\" or \"reset\"")
        })
    };
    let mut set = file.colors.clone();
    let names = match set.remove("names") {
        // Telegram's seven name colors.
        None => ["red", "orange", "purple", "green", "cyan", "blue", "pink"].map(p),
        Some(toml::Value::Array(values)) if values.len() == 7 => {
            let mut names = [Color::Reset; 7];
            for (color, value) in names.iter_mut().zip(&values) {
                *color = pick("names", value)?;
            }
            names
        }
        Some(_) => return Err("colors.names needs seven colors".into()),
    };
    let mut role = |name: &str, default: Color| match set.remove(name) {
        Some(value) => pick(name, &value),
        None => Ok(default),
    };
    // Own bubbles are the background tinted blue, like Telegram's. Less
    // tint on a light theme keeps dark text readable on it.
    let tint = if is_dark(p("bg")) { 0.3 } else { 0.2 };
    let own_bubble = role("own_bubble", mix(p("bg"), p("blue"), tint))?;
    let other_bubble = role("other_bubble", p("surface"))?;
    let colors = Colors {
        bg: role("bg", p("bg"))?,
        fg: role("fg", p("fg"))?,
        subtle: role("subtle", p("subtext"))?,
        muted: role("muted", p("comment"))?,
        border: role("border", p("overlay"))?,
        accent: role("accent", p("accent"))?,
        selection: role("selection", p("surface"))?,
        popup_bg: role("popup_bg", p("bg_alt"))?,
        primary: role("primary", p("blue"))?,
        highlighted: role("highlighted", p("orange"))?,
        insert: role("insert", p("green"))?,
        error: role("error", p("red"))?,
        warning: role("warning", p("yellow"))?,
        search: role("search", p("yellow"))?,
        command: role("command", p("purple"))?,
        reply: role("reply", p("cyan"))?,
        activity: role("activity", p("blue"))?,
        edit: role("edit", p("orange"))?,
        attach: role("attach", p("blue"))?,
        success: role("success", p("green"))?,
        own_bubble,
        own_meta: role("own_meta", mix(p("fg"), p("blue"), 0.5))?,
        other_bubble,
        other_meta: role("other_meta", p("subtext"))?,
        // Reactions are pills a shade off their bubble; yours are filled
        // in, as in Telegram.
        own_reaction: role("own_reaction", mix(own_bubble, p("fg"), 0.15))?,
        other_reaction: role("other_reaction", mix(other_bubble, p("fg"), 0.15))?,
        your_reaction: role("your_reaction", p("blue"))?,
        names,
        qr_dark: role("qr_dark", QR_DARK)?,
        qr_light: role("qr_light", QR_LIGHT)?,
    };
    match set.keys().next() {
        Some(name) => Err(format!("there is no color called {name:?} to set")),
        None => Ok(colors),
    }
}

/// What the UI paints, by role.
#[derive(Clone, Copy, Debug, PartialEq)]
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

impl Default for Colors {
    /// The default theme's.
    fn default() -> Self {
        Themes::built_in()
            .colors(DEFAULT)
            .expect("the default theme works")
    }
}

/// A color written "#rrggbb".
fn hex(value: &str) -> Option<Color> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok().map(rgb)
}

/// Whether `bg` is dark enough for light text, by how bright it looks.
fn is_dark(bg: Color) -> bool {
    let Color::Rgb(r, g, b) = bg else {
        return true;
    };
    0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b) < 128.0
}

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

    /// The built-in themes and these files, as if in the themes folder.
    fn with_files(files: &[(&str, &str)]) -> Themes {
        let user = files
            .iter()
            .map(|(id, text)| (id.to_string(), parse(text)))
            .collect();
        Themes::new(user, None)
    }

    #[test]
    fn every_built_in_theme_works_and_mocha_is_the_default() {
        let themes = Themes::built_in();
        assert_eq!(themes.list.len(), BUILT_IN.len());
        for theme in &themes.list {
            assert!(theme.colors.is_ok(), "{}: {:?}", theme.id, theme.colors);
            assert_ne!(theme.label, theme.id, "{} has a name", theme.id);
        }
        assert_eq!(themes.list[3].label, "Catppuccin Mocha");
        assert_eq!(Colors::default().bg, Color::Rgb(0x1e, 0x1e, 0x2e));
    }

    #[test]
    fn own_bubbles_are_tinted_between_base_and_blue() {
        // 30% of the way from base #1e1e2e to blue #89b4fa.
        assert_eq!(Colors::default().own_bubble, Color::Rgb(0x3e, 0x4b, 0x6b));
        // Only 20% on a light background: #eff1f5 to #1e66f5.
        let latte = Themes::built_in().colors("latte").unwrap();
        assert_eq!(latte.own_bubble, Color::Rgb(197, 213, 245));
    }

    #[test]
    fn a_theme_can_inherit_another_and_change_a_few_colors() {
        let themes = with_files(&[(
            "mine",
            "inherits = \"mocha\"\n\
             [palette]\nblue = \"#0000ff\"\n\
             [colors]\nsearch = \"red\"\nbg = \"reset\"\nborder = \"#123456\"",
        )]);
        let mocha = Colors::default();
        let mine = themes.colors("mine").unwrap();
        assert_eq!(mine.primary, Color::Rgb(0, 0, 0xff));
        assert_ne!(mine.own_bubble, mocha.own_bubble, "from the new blue");
        assert_eq!(mine.search, mocha.error, "by its palette name");
        assert_eq!(mine.border, Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(mine.bg, Color::Reset, "the terminal's own");
        assert_eq!(mine.popup_bg, mocha.popup_bg, "the rest is Mocha's");
        let last = themes.list.last().unwrap();
        assert_eq!((last.id.as_str(), last.label.as_str()), ("mine", "mine"));
    }

    #[test]
    fn a_file_named_like_a_built_in_theme_replaces_it_and_can_build_on_it() {
        let themes = with_files(&[(
            "mocha",
            "name = \"My Mocha\"\ninherits = \"mocha\"\n[colors]\nborder = \"#123456\"",
        )]);
        assert_eq!(themes.list.len(), BUILT_IN.len());
        assert_eq!(themes.list[3].id, "mocha");
        assert_eq!(themes.list[3].label, "My Mocha");
        let mine = themes.colors("mocha").unwrap();
        assert_eq!(mine.border, Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(mine.bg, Colors::default().bg);
    }

    #[test]
    fn the_users_themes_come_after_the_built_in_ones_by_name() {
        let themes = with_files(&[
            ("b", "name = \"Zed\"\ninherits = \"nord\""),
            ("a", "name = \"alpha\"\ninherits = \"b\""),
        ]);
        let labels: Vec<&str> = themes.list.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels[BUILT_IN.len()..], ["alpha", "Zed"]);
        let nord = themes.colors("nord").unwrap();
        assert_eq!(themes.colors("a").unwrap(), nord, "through b");
    }

    #[test]
    fn a_theme_that_cant_be_used_says_why() {
        let error = |text: &str| {
            let error = with_files(&[("bad", text)]).colors("bad").unwrap_err();
            assert!(error.starts_with("bad.toml: "), "{error}");
            error
        };
        let mocha = "inherits = \"mocha\"\n";
        let cases = [
            ("[palette]\nbg = \"#000000\"", "needs bg_alt, surface,"),
            (&format!("{mocha}[palette]\nbgg = \"#000000\""), "\"bgg\""),
            (&format!("{mocha}[palette]\nbg = \"black\""), "palette.bg"),
            (&format!("{mocha}[palette]\nbg = \"#+12345\""), "palette.bg"),
            (&format!("{mocha}[colors]\nbordr = \"red\""), "\"bordr\""),
            (
                &format!("{mocha}[colors]\nborder = \"nope\""),
                "colors.border",
            ),
            (&format!("{mocha}[colors]\nborder = 3"), "colors.border"),
            (&format!("{mocha}[colors]\nnames = [\"red\"]"), "seven"),
            ("inherits = \"nope\"", "\"nope\""),
            (&format!("{mocha}pallete = 1"), "line 2"),
        ];
        for (text, says) in cases {
            let error = error(text);
            assert!(error.contains(says), "{text:?} gave {error:?}");
        }
    }

    #[test]
    fn inheriting_in_a_circle_is_an_error_not_a_hang() {
        let themes = with_files(&[("a", "inherits = \"b\""), ("b", "inherits = \"a\"")]);
        assert!(themes.colors("a").unwrap_err().contains("circle"));
        let error = Themes::built_in().colors("nope").unwrap_err();
        assert!(error.contains("\"nope\""), "{error}");
    }

    #[test]
    fn the_users_themes_are_the_toml_files_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("tuigram-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mine.toml"), "inherits = \"nord\"").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();
        let themes = Themes::load(&dir);
        let ids: Vec<&str> = themes.list.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids[BUILT_IN.len()..], ["mine"]);
        assert_eq!(themes.dir.as_deref(), Some(dir.as_path()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(Themes::load(&dir).list.len(), BUILT_IN.len(), "no folder");
    }
}
