//! The shortcuts tab of the `?` popup.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::lang;
use crate::theme::Colors;

/// Every keyboard shortcut, grouped by where it works, in English: titles,
/// descriptions and keys written in words are put in the language set where
/// they're drawn. Keep this in step with the key handling in `app.rs` (and
/// the status bar hints).
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        lang::key("Everywhere"),
        &[
            ("?", lang::key("This help, and settings")),
            (
                ":",
                lang::key("Type a command: info, secret, key, timer, leave, proxy or logout"),
            ),
            (
                "s",
                lang::key("Find a chat or person: a name, @username or t.me link"),
            ),
            (
                "I",
                lang::key("What the chat is, and who's in it: Enter on someone writes to them"),
            ),
            ("H", lang::key("Highlight or unhighlight the selected chat")),
            (
                "Ctrl-o / Ctrl-i",
                lang::key("Back to the chat or reply you left / forward again"),
            ),
            (
                "Ctrl-r",
                lang::key("Resize the panes: h / l, = as at first, Enter keeps, Esc cancels"),
            ),
            ("q", lang::key("Quit (press again to stop waiting)")),
            ("Ctrl-c", lang::key("Quit, except while writing")),
            (
                "",
                lang::key(
                    "Keys work in the Cyrillic layout too: н is y, р is h; typing takes letters as they are",
                ),
            ),
        ],
    ),
    (
        lang::key("Chat list"),
        &[
            ("j / k", lang::key("Move down / up")),
            ("gg / G", lang::key("First / last chat")),
            ("Ctrl-d / Ctrl-u", lang::key("Half a page down / up")),
            (
                "Enter / l",
                lang::key(
                    "Open the chat, at its first unread message (Enter / h with the list on the right)",
                ),
            ),
            ("i", lang::key("Open the chat and write")),
            (
                "gm",
                lang::key("Open the chat at a message that mentions you"),
            ),
            (
                "Tab / Shift-Tab",
                lang::key("Next / previous folder, then the archive"),
            ),
            ("/", lang::key("Filter chats by name")),
            (
                "p",
                lang::key("Pin or unpin the chat (on all your devices)"),
            ),
            ("m", lang::key("Mute or unmute the chat")),
            (
                "a",
                lang::key("Archive the chat, or move it out of the archive"),
            ),
            ("Esc", lang::key("Clear the filter")),
        ],
    ),
    (
        lang::key("Forum topics"),
        &[
            ("j / k", lang::key("Move down / up")),
            ("gg / G", lang::key("First / last topic")),
            (
                "Enter / l",
                lang::key("Open the topic's messages (Enter / h with the list on the right)"),
            ),
            ("i", lang::key("Open the topic and write")),
            (
                "Ctrl-r",
                lang::key("Resize the topics pane: h / l, = as at first"),
            ),
            (
                "h / Esc",
                lang::key("Back to the chat list (l with the list on the right)"),
            ),
        ],
    ),
    (
        lang::key("Messages"),
        &[
            ("j / k", lang::key("Newer / older message")),
            ("gg / G", lang::key("Oldest / newest message")),
            ("Ctrl-d / Ctrl-u", lang::key("Half a page newer / older")),
            (
                "Enter",
                lang::key(
                    "Send a message that wasn't sent again, show spoilers, view a photo shown only while open, play or pause a voice message, vote in a poll, list a bot's buttons, show the photo, or open the file or link",
                ),
            ),
            (
                "y",
                lang::key("Copy the text, a link, or the photo or file"),
            ),
            ("O", lang::key("Show the file in the file manager")),
            (
                "v",
                lang::key("Show the text to select part of it and copy that"),
            ),
            ("r", lang::key("Reply")),
            (
                "f",
                lang::key("Forward to another chat (an album goes whole)"),
            ),
            ("e", lang::key("Edit your message")),
            (
                "R",
                lang::key("React, or take your reaction back (/ finds an emoji by name)"),
            ),
            ("X", lang::key("Take back all your reactions")),
            (
                "J",
                lang::key("Join the public group or channel you're viewing"),
            ),
            ("d", lang::key("Delete")),
            ("P", lang::key("Pin the message (asks how), or unpin it")),
            (
                "gp",
                lang::key("Pinned messages: Enter goes to one, P unpins it"),
            ),
            ("gd", lang::key("Go to the message a reply answers")),
            (
                "gu",
                lang::key("Go to the first unread message, under the \"Unread messages\" line"),
            ),
            (
                "gm / gM",
                lang::key(
                    "Go to the oldest unread message that mentions you or answers yours (@ in the chat list), and seeing it reads it; once you've seen them all, to the one before / after the cursor",
                ),
            ),
            (
                "Tab",
                lang::key("Forward again, as Ctrl-i (most terminals send Ctrl-i as Tab)"),
            ),
            ("i", lang::key("Write a message")),
            (
                "a",
                lang::key("Attach a file by its path (Tab completes it)"),
            ),
            (
                "p",
                lang::key("Paste a photo, files or text from the clipboard"),
            ),
            (
                "/",
                lang::key(
                    "Search the whole chat; a has: search lists every match in a media browser",
                ),
            ),
            (
                "from: has:",
                lang::key(
                    "In a search: from:@alice, from:me, from:this or from:a name; has:photo (video, media, file, link, voice, note, gif, audio)",
                ),
            ),
            (
                "before: after:",
                lang::key("In a search: before:2025-10-01, after:2025-09-01 (that day counts)"),
            ),
            ("n / N", lang::key("Next older / newer match")),
            ("Q", lang::key("Close the chat, leaving only the chat list")),
            (
                "Esc",
                lang::key(
                    "End the search, cancel the edit, remove the files, cancel the reply, or go back",
                ),
            ),
            (
                "h",
                lang::key(
                    "Back to the chat list, or a forum's topics (l with the list on the right)",
                ),
            ),
        ],
    ),
    (
        lang::key("Writing"),
        &[
            ("Enter", lang::key("Send, or save the edit")),
            ("Alt-Enter / Ctrl-j", lang::key("New line")),
            ("Ctrl-u", lang::key("Delete back to the start of the line")),
            (
                lang::key("**bold**"),
                lang::key(
                    "Formatting: __italic__ ~~strike~~ ||spoiler|| `code` ```block``` [text](url)",
                ),
            ),
            (
                lang::key("@name / :smile"),
                lang::key("Suggests people in the group, or emoji; Tab puts one in"),
            ),
            (
                lang::key("/command"),
                lang::key("Starting a message: the bots' commands"),
            ),
            (
                "Ctrl-v",
                lang::key("Paste a photo, files or text from the clipboard"),
            ),
            (
                lang::key("Drop a file"),
                lang::key("Attach it; the text you write is its caption"),
            ),
            (
                "Ctrl-z",
                lang::key("Turn files just pasted back into their path as text"),
            ),
            (
                "Ctrl-t",
                lang::key("Send attached photos as files, uncompressed, or back"),
            ),
            ("Tab", lang::key("Stickers")),
            (
                "Esc / Ctrl-c",
                lang::key(
                    "Back to Normal mode; what's written stays as the chat's draft, on your other devices too",
                ),
            ),
        ],
    ),
    (
        lang::key("Selecting text (v)"),
        &[
            (
                "h / j / k / l",
                lang::key("Move a character, or a row on screen"),
            ),
            (
                "w / b / e",
                lang::key("Next word / word before / end of the word"),
            ),
            ("0 / $", lang::key("Start / end of the line")),
            ("g / G", lang::key("Start / end of the text")),
            ("Ctrl-d / Ctrl-u", lang::key("Half a page down / up")),
            (
                "v",
                lang::key(
                    "Start selecting where the cursor is, or stop; it takes in both ends, as in vim",
                ),
            ),
            ("y", lang::key("Copy what's selected and close")),
            ("Esc", lang::key("Stop selecting, then close")),
        ],
    ),
    (
        lang::key("Stickers"),
        &[
            ("h / j / k / l", lang::key("Move")),
            (
                "H / L",
                lang::key("Previous / next tab: Recent, Favorites, your sets"),
            ),
            (
                "Enter",
                lang::key("Send the sticker (as the reply, if you're replying)"),
            ),
            ("/", lang::key("Find stickers by emoji or word")),
            ("Tab / Esc", lang::key("Back to writing")),
        ],
    ),
    (
        lang::key("Photo viewer"),
        &[
            (
                "h / l",
                lang::key("The photo before / after it in the chat"),
            ),
            (
                "j / k",
                lang::key("Zoom in / out (also + / -), up to filling the window"),
            ),
            ("o", lang::key("Open the photo in your computer's viewer")),
            ("y", lang::key("Copy the photo")),
            ("Enter / Esc / q", lang::key("Close")),
        ],
    ),
    (
        lang::key("Video player"),
        &[
            (
                "Space",
                lang::key("Play (once) or pause, right in the message (size in Settings)"),
            ),
            ("z", lang::key("Twice as big in the chat, or back")),
            ("j / k", lang::key("Zoom in / out, on the whole window")),
            ("o", lang::key("Open the video in your computer's player")),
            ("Enter", lang::key("Fill the window; it closes once played")),
            ("Esc / q", lang::key("Close")),
        ],
    ),
    (
        lang::key("Bot buttons"),
        &[
            (
                "h / j / k / l",
                lang::key("Move, as the buttons are laid out"),
            ),
            ("Tab / Shift-Tab", lang::key("Next / previous button")),
            (
                "Enter",
                lang::key(
                    "Press it: the bot answers, a link opens (asking first), a reply button sends its words",
                ),
            ),
            ("Esc / q", lang::key("Close")),
        ],
    ),
    (
        lang::key("Secret chats"),
        &[
            (
                ":secret",
                lang::key(
                    "Start one with the person in this chat (asks first); it's kept on this computer only",
                ),
            ),
            (
                ":key",
                lang::key("Its key as a picture, to compare with the other person's"),
            ),
            (
                ":timer",
                lang::key("How long new messages last once they're seen"),
            ),
            (
                ":leave",
                lang::key("End it for both of you, and delete it here (asks first)"),
            ),
            (
                "Enter",
                lang::key("On a 🔥 photo: view it; it's covered again when you move on"),
            ),
        ],
    ),
    (
        lang::key("Search and command prompts"),
        &[
            (
                "Enter",
                lang::key("Search, keep the chat filter, or run the command"),
            ),
            (
                "Tab / Shift-Tab",
                lang::key(
                    "Complete a command or a search filter (again for the next one), or the path of a file to attach",
                ),
            ),
            ("Esc / Ctrl-c", lang::key("Cancel")),
        ],
    ),
    (
        lang::key("Forward and find popups"),
        &[
            (
                lang::key("Type"),
                lang::key("Search chats by name, or @username"),
            ),
            ("Up / Down", lang::key("Move (also Ctrl-p / Ctrl-n, Tab)")),
            ("Enter", lang::key("Forward there, or open it")),
            ("Esc", lang::key("Cancel")),
        ],
    ),
    (
        lang::key("Menus and popups"),
        &[
            ("j / k", lang::key("Move")),
            ("Enter", lang::key("Choose")),
            ("1-9", lang::key("Choose by number")),
            (
                "Space",
                lang::key("Tick a setting, or use a sound or a theme (Enter too); saved at once"),
            ),
            ("Esc / q", lang::key("Close")),
            ("Tab / h / l", lang::key("Switch tabs in this popup")),
            (
                "y / n",
                lang::key("Go ahead or not, when asked about a file, a link or joining"),
            ),
        ],
    ),
];

/// A key as the language set writes it: only keys written in words, such as
/// "Drop a file", have other words; the rest are looked up and stay as they are.
fn key_text(key: &'static str) -> &'static str {
    lang::t(key)
}

/// The longest key, which sets where the descriptions start.
fn key_width() -> usize {
    SHORTCUTS
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .map(|&(key, _)| key_text(key).width())
        .max()
        .unwrap_or(0)
}

/// The shortcuts as lines, in the language set: a heading per group, then
/// each key and what it does.
fn lines(colors: &Colors) -> Vec<Line<'static>> {
    let key_width = key_width();
    let mut lines = Vec::new();
    for (i, &(group, keys)) in SHORTCUTS.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.push(
            Line::from(format!(" {}", lang::t(group)))
                .fg(colors.accent)
                .bold(),
        );
        for &(key, what) in keys {
            let key = key_text(key);
            let pad = " ".repeat(key_width - key.width());
            lines.push(Line::from(vec![
                Span::from(format!("   {key}{pad}  ")).fg(colors.primary),
                Span::from(lang::t(what)),
            ]));
        }
    }
    lines
}

/// Columns the widest line takes in the language set, plus one for the
/// scrollbar, for sizing the popup.
pub fn width() -> usize {
    let what = SHORTCUTS
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .map(|&(_, what)| lang::t(what).width())
        .max()
        .unwrap_or(0);
    // Indent, the keys, a gap, then the descriptions.
    3 + key_width() + 2 + what + 1
}

/// Rows the list takes, for sizing the popup.
pub fn height() -> usize {
    SHORTCUTS
        .iter()
        .map(|(_, keys)| keys.len() + 2)
        .sum::<usize>()
        - 1
}

/// Draws the list from row `scroll`, first pulling `scroll` back if it's
/// past the end.
pub fn draw(frame: &mut Frame, area: Rect, scroll: &mut usize, colors: &Colors) {
    let lines = lines(colors);
    let max = lines.len().saturating_sub(usize::from(area.height));
    *scroll = (*scroll).min(max);
    frame.render_widget(Paragraph::new(lines).scroll((*scroll as u16, 0)), area);
    super::scrollbar(frame, area, max, *scroll, colors);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Lang;

    #[test]
    fn height_counts_every_line_in_every_language() {
        for language in Lang::ALL {
            lang::set(language);
            assert_eq!(height(), lines(&Colors::default()).len(), "{language:?}");
        }
    }

    #[test]
    fn width_leaves_room_for_every_line_and_the_scrollbar_in_every_language() {
        for language in Lang::ALL {
            lang::set(language);
            let widest = lines(&Colors::default())
                .iter()
                .map(Line::width)
                .max()
                .unwrap();
            assert_eq!(width(), widest + 1, "{language:?}");
        }
    }

    #[test]
    fn keys_written_in_words_are_translated_and_the_rest_stay() {
        let text = |l: &Line| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        let row = |key: &str| {
            let lines = lines(&Colors::default());
            lines.iter().map(text).find(|l| l.contains(key)).unwrap()
        };
        lang::set(Lang::Ru);
        assert!(row("Перетащить файл").contains("Прикрепить его"));
        assert!(row("Alt-Enter / Ctrl-j").contains("Новая строка"));

        lang::set(Lang::Vi);
        assert_eq!(row("Mọi nơi"), " Mọi nơi");
        assert!(row("Alt-Enter / Ctrl-j").contains("Xuống dòng"));
    }
}
