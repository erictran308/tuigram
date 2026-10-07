//! Bot buttons: the ones a bot puts under its message (inline buttons), and
//! the ones it offers in place of the phone's keyboard (reply buttons). The
//! bubble shows them, and Enter on it lists them to press.

use tdlib_rs::enums::{InlineKeyboardButtonType, KeyboardButtonType, ReplyMarkup};
use tokio::time::Instant;

use crate::messages::{Link, MediaFile, link_host, one_line, web_url};
use crate::text;

/// What pressing a button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Press {
    /// Sends this data to the bot, which may answer with a note, an alert
    /// or a link, and often changes its message.
    Callback(String),
    /// Opens a web link, asking first: a button's words are cut to fit, so
    /// even ones that look like the address may not be all of it.
    Open(Link),
    /// Opens a `tg:` link in tuigram, if it leads to a chat.
    Telegram(String),
    /// Opens a chat with this person.
    User(i64),
    /// Copies this text.
    Copy(String),
    /// Sends this text in the chat, as if you'd typed it: a reply button.
    Send(String),
    /// The message's file, listed after the buttons.
    File(MediaFile),
    /// Something only Telegram's own apps do; says what.
    Unsupported(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub press: Press,
}

impl Button {
    /// Enter on it does something in tuigram.
    pub fn works(&self) -> bool {
        !matches!(self.press, Press::Unsupported(_))
    }

    /// What Enter on it does, in full, for under the popup's buttons: their
    /// words are cut to fit, and a reply button sends all of its own.
    pub fn describe(&self) -> String {
        match &self.press {
            Press::Callback(_) => {
                format!("Presses \"{}\": the bot decides what happens", self.label)
            }
            Press::Open(link) => {
                let host = link_host(&link.url).unwrap_or_else(|| link.url.clone());
                format!("Opens {} in your browser, asking first", one_line(&host))
            }
            Press::Telegram(url) => format!("Opens {url} in tuigram"),
            Press::User(_) => format!("Opens your chat with {}", self.label),
            Press::Copy(text) => format!("Copies: {text}"),
            Press::Send(text) => format!("Sends as you: {text}"),
            Press::File(file) => format!("Opens {}", file.label),
            Press::Unsupported(why) => (*why).to_string(),
        }
    }
}

/// A message's buttons, in rows as the bot laid them out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyboard {
    pub rows: Vec<Vec<Button>>,
    /// Reply buttons, which send their words as your message, rather than
    /// act on the bot's.
    pub reply: bool,
}

/// What a button that tuigram can't press says instead.
const ELSEWHERE: &str = "Only Telegram's own apps can do this";

impl Keyboard {
    /// The buttons TDLib sent with a message, if any. Their words are the
    /// bot's, so they're cleaned and put on one line.
    pub fn of(markup: Option<&ReplyMarkup>) -> Option<Self> {
        let (rows, reply): (Vec<Vec<Button>>, bool) = match markup? {
            ReplyMarkup::InlineKeyboard(k) => {
                let rows = k.rows.iter().map(|row| {
                    row.iter()
                        .map(|b| inline_button(&one_line(&b.text), &b.r#type))
                        .collect()
                });
                (rows.collect(), false)
            }
            ReplyMarkup::ShowKeyboard(k) => {
                let rows = k.rows.iter().map(|row| {
                    row.iter()
                        .map(|b| reply_button(one_line(&b.text), &b.r#type))
                        .collect()
                });
                (rows.collect(), true)
            }
            ReplyMarkup::RemoveKeyboard(_) | ReplyMarkup::ForceReply(_) => return None,
        };
        let rows: Vec<Vec<Button>> = rows.into_iter().filter(|r| !r.is_empty()).collect();
        (!rows.is_empty()).then_some(Keyboard { rows, reply })
    }
}

fn inline_button(label: &str, kind: &InlineKeyboardButtonType) -> Button {
    use InlineKeyboardButtonType as T;
    let press = match kind {
        T::Callback(c) => Press::Callback(c.data.clone()),
        // A login link would sign you in to the site; tuigram only opens it.
        T::Url(u) => link(label, &u.url),
        T::LoginUrl(u) => link(label, &u.url),
        T::User(u) => Press::User(u.user_id),
        T::CopyText(c) => Press::Copy(one_line(&c.text)),
        T::WebApp(_) => Press::Unsupported("Web apps only open in Telegram's own apps"),
        T::CallbackGame => Press::Unsupported("Games only run in Telegram's own apps"),
        T::Buy => Press::Unsupported("Payments only work in Telegram's own apps"),
        T::CallbackWithPassword(_) => {
            Press::Unsupported("This button asks for your password: use Telegram's own apps")
        }
        T::SwitchInline(_) => Press::Unsupported(ELSEWHERE),
    };
    Button {
        label: label.to_string(),
        press,
    }
}

fn reply_button(label: String, kind: &KeyboardButtonType) -> Button {
    let press = match kind {
        KeyboardButtonType::Text => Press::Send(label.clone()),
        _ => Press::Unsupported(ELSEWHERE),
    };
    Button { label, press }
}

/// A button's link: a web link that asks first unless its words spell it
/// out, or a `tg:` link. Anything else (a local file, an app's scheme)
/// can't be pressed.
fn link(label: &str, url: &str) -> Press {
    let url = text::clean(url);
    let url = url.trim();
    if url.to_ascii_lowercase().starts_with("tg:") && !url.contains(char::is_whitespace) {
        return Press::Telegram(url.to_string());
    }
    match web_url(url) {
        Some(url) => Press::Open(Link {
            url,
            disguise: Some(label.to_string()),
        }),
        None => Press::Unsupported("This button's link isn't a web address"),
    }
}

/// Enter on a message with buttons: them, in their rows, then the message's
/// file and links, each on a row of its own, so Enter still opens those.
pub struct ButtonMenu {
    pub message_id: i64,
    /// The message on one line, so it's clear whose buttons they are.
    pub snippet: String,
    pub reply: bool,
    pub rows: Vec<Vec<Button>>,
    /// The cursor: a row, and a button in it.
    pub row: usize,
    pub col: usize,
    /// When it came up. Enter in the first moments was pressed twice, or
    /// held, so it doesn't press anything.
    pub shown: Instant,
}

impl ButtonMenu {
    pub fn new(
        message_id: i64,
        snippet: String,
        keyboard: &Keyboard,
        file: Option<&MediaFile>,
        links: &[Link],
    ) -> Self {
        let mut rows = keyboard.rows.clone();
        rows.extend(file.map(|f| {
            vec![Button {
                label: format!("Open {}", f.label),
                press: Press::File(f.clone()),
            }]
        }));
        rows.extend(links.iter().map(|l| {
            let host = link_host(&l.url).unwrap_or_else(|| l.url.clone());
            vec![Button {
                label: format!("Open link: {}", one_line(&host)),
                press: Press::Open(l.clone()),
            }]
        }));
        Self {
            message_id,
            snippet,
            reply: keyboard.reply,
            rows,
            row: 0,
            col: 0,
            shown: Instant::now(),
        }
    }

    pub fn current(&self) -> Option<&Button> {
        self.rows.get(self.row)?.get(self.col)
    }

    /// Up or down `delta` rows, staying as near the same column as the row
    /// has buttons.
    pub fn move_rows(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        self.row = self.row.saturating_add_signed(delta).min(last);
        self.clamp();
    }

    /// Left or right within the row.
    pub fn move_cols(&mut self, delta: isize) {
        self.col = self.col.saturating_add_signed(delta);
        self.clamp();
    }

    /// To the next or previous button, on to the next row at the end of one,
    /// round the end.
    pub fn move_by(&mut self, delta: isize) {
        let total = self.rows.iter().map(Vec::len).sum::<usize>();
        if total == 0 {
            return;
        }
        let at = self.index() as isize + delta;
        self.select(at.rem_euclid(total as isize) as usize);
    }

    /// The button under the cursor, counted through the rows from 0.
    pub fn index(&self) -> usize {
        self.rows[..self.row].iter().map(Vec::len).sum::<usize>() + self.col
    }

    /// Puts the cursor on button `index`, counted through the rows.
    fn select(&mut self, mut index: usize) {
        for (row, buttons) in self.rows.iter().enumerate() {
            if index < buttons.len() {
                (self.row, self.col) = (row, index);
                return;
            }
            index -= buttons.len();
        }
    }

    fn clamp(&mut self) {
        let len = self.rows.get(self.row).map_or(0, Vec::len);
        self.col = self.col.min(len.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use tdlib_rs::enums::ButtonStyle;
    use tdlib_rs::types::{
        InlineKeyboardButton, InlineKeyboardButtonTypeCallback, InlineKeyboardButtonTypeUrl,
        KeyboardButton, ReplyMarkupInlineKeyboard, ReplyMarkupShowKeyboard,
    };

    use super::*;

    fn inline(text: &str, r#type: InlineKeyboardButtonType) -> InlineKeyboardButton {
        InlineKeyboardButton {
            text: text.into(),
            icon_custom_emoji_id: 0,
            style: ButtonStyle::Default,
            r#type,
        }
    }

    fn callback(text: &str, data: &str) -> InlineKeyboardButton {
        let data = data.into();
        inline(
            text,
            InlineKeyboardButtonType::Callback(InlineKeyboardButtonTypeCallback { data }),
        )
    }

    fn url(text: &str, url: &str) -> InlineKeyboardButton {
        let url = url.into();
        inline(
            text,
            InlineKeyboardButtonType::Url(InlineKeyboardButtonTypeUrl { url }),
        )
    }

    fn keyboard(rows: Vec<Vec<InlineKeyboardButton>>) -> Keyboard {
        let markup = ReplyMarkup::InlineKeyboard(ReplyMarkupInlineKeyboard { rows });
        Keyboard::of(Some(&markup)).unwrap()
    }

    #[test]
    fn inline_buttons_keep_their_rows_and_say_what_they_do() {
        let keyboard = keyboard(vec![
            vec![callback("Yes", "y"), callback("No\u{202e}pe", "n")],
            vec![url("Docs", "https://example.com/docs")],
            vec![url("example.com/docs", "https://example.com/docs/")],
            vec![
                url("Run", "file:///bin/sh"),
                url("Chat", "tg://resolve?domain=x"),
            ],
            vec![inline("Play", InlineKeyboardButtonType::CallbackGame)],
        ]);
        assert!(!keyboard.reply);
        let row = |i: usize| &keyboard.rows[i];
        assert_eq!(row(0)[0].press, Press::Callback("y".into()));
        assert_eq!(row(0)[1].label, "Nope", "the bot's words are cleaned");
        let Press::Open(docs) = &row(1)[0].press else {
            panic!("a link");
        };
        assert_eq!(docs.disguise.as_deref(), Some("Docs"), "asks first");
        let Press::Open(plain) = &row(2)[0].press else {
            panic!("a link");
        };
        assert!(
            plain.disguise.is_some(),
            "even words that look like the address may be cut off"
        );
        assert_eq!(
            row(2)[0].describe(),
            "Opens example.com in your browser, asking first"
        );
        assert!(!row(3)[0].works(), "only web links");
        assert_eq!(
            row(3)[1].press,
            Press::Telegram("tg://resolve?domain=x".into())
        );
        assert!(!row(4)[0].works());
    }

    #[test]
    fn reply_buttons_send_their_words() {
        let button = |text: &str, r#type| KeyboardButton {
            text: text.into(),
            icon_custom_emoji_id: 0,
            style: ButtonStyle::Default,
            r#type,
        };
        let markup = ReplyMarkup::ShowKeyboard(ReplyMarkupShowKeyboard {
            rows: vec![
                vec![button("*Menu*", KeyboardButtonType::Text)],
                vec![button(
                    "Share my number",
                    KeyboardButtonType::RequestPhoneNumber,
                )],
                vec![],
            ],
            ..ReplyMarkupShowKeyboard::default()
        });
        let keyboard = Keyboard::of(Some(&markup)).unwrap();
        assert!(keyboard.reply);
        assert_eq!(keyboard.rows.len(), 2, "empty rows are dropped");
        assert_eq!(keyboard.rows[0][0].press, Press::Send("*Menu*".into()));
        assert!(!keyboard.rows[1][0].works(), "your number stays yours");
    }

    #[test]
    fn the_cursor_moves_through_rows_and_round_the_end() {
        let keyboard = keyboard(vec![
            vec![callback("1", "1"), callback("2", "2"), callback("3", "3")],
            vec![callback("4", "4")],
        ]);
        let link = Link::from("https://example.com/a");
        let mut menu = ButtonMenu::new(9, "Pick".into(), &keyboard, None, &[link]);
        assert_eq!(menu.rows.len(), 3, "the message's link comes last");
        assert_eq!(menu.rows[2][0].label, "Open link: example.com");

        menu.move_cols(5);
        assert_eq!(menu.current().unwrap().label, "3");
        menu.move_rows(1);
        assert_eq!(menu.current().unwrap().label, "4", "the row's only one");
        menu.move_by(1);
        assert_eq!(menu.index(), 4);
        menu.move_by(1);
        assert_eq!(menu.current().unwrap().label, "1", "round the end");
        menu.move_by(-1);
        assert_eq!(menu.index(), 4);
        menu.select(1);
        assert_eq!(menu.current().unwrap().label, "2");
        menu.select(5);
        assert_eq!(menu.current().unwrap().label, "2", "no such button");
    }

    #[test]
    fn under_the_buttons_it_says_in_full_what_enter_does() {
        let reply = Button {
            label: "Yes".into(),
            press: Press::Send("Yes, and everything after it".into()),
        };
        assert_eq!(
            reply.describe(),
            "Sends as you: Yes, and everything after it"
        );
        let tg = link("Chat", "tg://resolve?domain=x\u{202e}");
        assert_eq!(
            tg,
            Press::Telegram("tg://resolve?domain=x".into()),
            "cleaned"
        );
        assert!(matches!(link("Chat", "tg://x y"), Press::Unsupported(_)));
    }
}
