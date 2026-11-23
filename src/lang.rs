//! Interface languages. tuigram is translated in part so far: the status
//! bar, the `?` popup (shortcuts and settings) and the chat list's header.
//! What isn't translated stays English. `Settings::language` picks: "en"
//! (the default) or "ru".

/// The text in the language set: `ru` when Russian, else the English
/// original. Both sit at the use site, so a changed string is changed in
/// every language at once.
pub fn pick<'a>(ru: bool, en: &'a str, ru_text: &'a str) -> &'a str {
    if ru { ru_text } else { en }
}
