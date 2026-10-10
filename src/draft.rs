//! Messages started and left unsent. Telegram keeps one per chat, and per
//! forum topic, as its draft: the composer has it again when you come back,
//! here or on your other devices, and the chat list shows it.

use tdlib_rs::enums::{InputMessageContent, InputMessageReplyTo};
use tdlib_rs::types::{DraftMessage, FormattedText, InputMessageText};

use crate::messages::one_line;
use crate::tg;

/// What was left written in a chat.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// What the composer shows: Markdown, as it was typed.
    pub text: String,
    /// The message it answers.
    pub reply_to: Option<i64>,
    /// What Telegram was given for `text`, to tell TDLib's echo of a draft
    /// saved here from one written elsewhere.
    formatted: FormattedText,
}

/// A draft to save in a chat, or a forum topic; `None` clears it.
pub struct Keep {
    pub chat_id: i64,
    pub topic: Option<i32>,
    pub draft: Option<Draft>,
}

impl Draft {
    /// A draft written here, its Markdown made formatting as when it's
    /// sent; `None` for nothing written, which clears it.
    pub fn new(text: &str, reply_to: Option<i64>) -> Option<Self> {
        if text.trim().is_empty() {
            return None;
        }
        Some(Self {
            text: text.to_string(),
            reply_to,
            formatted: tg::markdown(text.to_string()),
        })
    }

    /// Telegram's draft, from `updateChatDraftMessage` and the like. The
    /// one saved from here (`kept`) keeps its text as it was typed when
    /// TDLib echoes it back; one written elsewhere is written as Markdown.
    pub fn of(draft: Option<&DraftMessage>, kept: Option<&Draft>) -> Option<Self> {
        let draft = draft?;
        let InputMessageContent::InputMessageText(input) = &draft.input_message_text else {
            return None;
        };
        let reply_to = match &draft.reply_to {
            Some(InputMessageReplyTo::Message(r)) => Some(r.message_id),
            _ => None,
        };
        if let Some(kept) = kept.filter(|k| k.formatted == input.text && k.reply_to == reply_to) {
            return Some(kept.clone());
        }
        let text = tg::to_markdown(&input.text)
            .map_or_else(|| crate::text::clean(&input.text.text), |t| t.markdown);
        (!text.trim().is_empty()).then(|| Self {
            text,
            reply_to,
            formatted: input.text.clone(),
        })
    }

    /// What `setChatDraftMessage` takes.
    pub fn message(&self) -> DraftMessage {
        DraftMessage {
            reply_to: self.reply_to.map(tg::reply_to_message),
            date: 0,
            input_message_text: InputMessageContent::InputMessageText(InputMessageText {
                text: self.formatted.clone(),
                link_preview_options: None,
                clear_draft: false,
            }),
            effect_id: 0,
            suggested_post_info: None,
        }
    }

    /// One line of it, for the chat list.
    pub fn snippet(&self) -> String {
        one_line(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use tdlib_rs::enums::TextEntityType;
    use tdlib_rs::types::TextEntity;

    use super::*;

    #[test]
    fn nothing_written_is_no_draft() {
        tg::quiet();
        assert_eq!(Draft::new(" \n ", Some(4)), None);
        let draft = Draft::new("see you **at 5**", Some(4)).unwrap();
        assert_eq!(draft.text, "see you **at 5**");
        assert_eq!(draft.reply_to, Some(4));
        assert_eq!(draft.snippet(), "see you **at 5**");
    }

    #[test]
    fn a_draft_saved_here_comes_back_as_it_was_typed() {
        tg::quiet();
        // Markdown that isn't closed stays as typed, which TDLib could
        // write back otherwise.
        let kept = Draft::new("2 * 3 = __6", Some(9)).unwrap();
        let echo = kept.message();
        assert_eq!(Draft::of(Some(&echo), Some(&kept)), Some(kept.clone()));
        // Told it answers another message, it's a new draft.
        let mut moved = kept.message();
        moved.reply_to = Some(tg::reply_to_message(10));
        let other = Draft::of(Some(&moved), Some(&kept)).unwrap();
        assert_eq!(other.reply_to, Some(10));
    }

    #[test]
    fn a_draft_written_elsewhere_is_written_as_markdown() {
        tg::quiet();
        let bold = TextEntity {
            offset: 0,
            length: 5,
            r#type: TextEntityType::Bold,
        };
        let text = FormattedText {
            text: "hello there".into(),
            entities: vec![bold],
        };
        let message = DraftMessage {
            reply_to: None,
            date: 0,
            input_message_text: InputMessageContent::InputMessageText(InputMessageText {
                text,
                link_preview_options: None,
                clear_draft: false,
            }),
            effect_id: 0,
            suggested_post_info: None,
        };
        let draft = Draft::of(Some(&message), None).unwrap();
        assert_eq!(draft.text, "**hello** there");
        assert_eq!(draft.reply_to, None);
        assert_eq!(Draft::of(None, Some(&draft)), None, "cleared elsewhere");
    }
}
