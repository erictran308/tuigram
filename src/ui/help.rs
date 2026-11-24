//! The shortcuts tab of the `?` popup.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme::Colors;

/// Every keyboard shortcut, grouped by where it works. Keep this in step
/// with the key handling in `app.rs` (and the status bar hints).
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Everywhere",
        &[
            ("?", "This help, and settings"),
            (
                ":",
                "Type a command: info, secret, key, timer, leave, proxy or logout",
            ),
            ("s", "Find a chat or person: a name, @username or t.me link"),
            (
                "I",
                "What the chat is, and who's in it: Enter on someone writes to them",
            ),
            ("H", "Highlight or unhighlight the selected chat"),
            (
                "Ctrl-o / Ctrl-i",
                "Back to the chat or reply you left / forward again",
            ),
            (
                "Ctrl-r",
                "Resize the panes: h / l, = as at first, Enter keeps, Esc cancels",
            ),
            ("q", "Quit (press again to stop waiting)"),
            ("Ctrl-c", "Quit, except while writing"),
            (
                "",
                "Keys work in the Cyrillic layout too: н is y, р is h; typing takes letters as they are",
            ),
        ],
    ),
    (
        "Chat list",
        &[
            ("j / k", "Move down / up"),
            ("gg / G", "First / last chat"),
            ("Ctrl-d / Ctrl-u", "Half a page down / up"),
            (
                "Enter / l",
                "Open the chat, at its first unread message (Enter / h with the list on the right)",
            ),
            ("i", "Open the chat and write"),
            ("gm", "Open the chat at a message that mentions you"),
            (
                "Tab / Shift-Tab",
                "Next / previous folder, then the archive",
            ),
            ("/", "Filter chats by name"),
            ("p", "Pin or unpin the chat (on all your devices)"),
            ("m", "Mute or unmute the chat"),
            ("a", "Archive the chat, or move it out of the archive"),
            ("Esc", "Clear the filter"),
        ],
    ),
    (
        "Forum topics",
        &[
            ("j / k", "Move down / up"),
            ("gg / G", "First / last topic"),
            (
                "Enter / l",
                "Open the topic's messages (Enter / h with the list on the right)",
            ),
            ("i", "Open the topic and write"),
            ("Ctrl-r", "Resize the topics pane: h / l, = as at first"),
            (
                "h / Esc",
                "Back to the chat list (l with the list on the right)",
            ),
        ],
    ),
    (
        "Messages",
        &[
            ("j / k", "Newer / older message"),
            ("gg / G", "Oldest / newest message"),
            ("Ctrl-d / Ctrl-u", "Half a page newer / older"),
            (
                "Enter",
                "Send a message that wasn't sent again, show spoilers, view a photo shown only while open, play or pause a voice message, vote in a poll, list a bot's buttons, show the photo, or open the file or link",
            ),
            ("y", "Copy the text, a link, or the photo or file"),
            ("O", "Show the file in the file manager"),
            ("v", "Show the text to select part of it and copy that"),
            ("r", "Reply"),
            ("f", "Forward to another chat (an album goes whole)"),
            ("e", "Edit your message"),
            (
                "R",
                "React, or take your reaction back (/ finds an emoji by name)",
            ),
            ("X", "Take back all your reactions"),
            ("d", "Delete"),
            ("P", "Pin the message (asks how), or unpin it"),
            ("gp", "Pinned messages: Enter goes to one, P unpins it"),
            ("gd", "Go to the message a reply answers"),
            (
                "gu",
                "Go to the first unread message, under the \"Unread messages\" line",
            ),
            (
                "gm / gM",
                "Go to the oldest unread message that mentions you or answers yours (@ in the chat list), and seeing it reads it; once you've seen them all, to the one before / after the cursor",
            ),
            (
                "Tab",
                "Forward again, as Ctrl-i (most terminals send Ctrl-i as Tab)",
            ),
            ("i", "Write a message"),
            ("a", "Attach a file by its path (Tab completes it)"),
            ("p", "Paste a photo, files or text from the clipboard"),
            (
                "/",
                "Search the whole chat; a has:/type: search lists every match in a media browser",
            ),
            (
                "from: has:",
                "In a search: from:@alice, from:me, from:this or from:a name; has:photo (video, media, file, link, voice, gif, audio)",
            ),
            (
                "before: after:",
                "In a search: before:2025-10-01, after:2025-09-01 (that day counts)",
            ),
            ("n / N", "Next older / newer match"),
            (
                "Esc",
                "End the search, cancel the edit, remove the files, cancel the reply, or go back",
            ),
            (
                "h",
                "Back to the chat list, or a forum's topics (l with the list on the right)",
            ),
        ],
    ),
    (
        "Writing",
        &[
            ("Enter", "Send, or save the edit"),
            ("Alt-Enter / Ctrl-j", "New line"),
            ("Ctrl-u", "Delete back to the start of the line"),
            (
                "**bold**",
                "Formatting: __italic__ ~~strike~~ ||spoiler|| `code` ```block``` [text](url)",
            ),
            (
                "@name / :smile",
                "Suggests people in the group, or emoji; Tab puts one in",
            ),
            ("/command", "Starting a message: the bots' commands"),
            ("Ctrl-v", "Paste a photo, files or text from the clipboard"),
            (
                "Drop a file",
                "Attach it; the text you write is its caption",
            ),
            (
                "Ctrl-z",
                "Turn files just pasted back into their path as text",
            ),
            (
                "Ctrl-t",
                "Send attached photos as files, uncompressed, or back",
            ),
            ("Tab", "Stickers"),
            (
                "Esc / Ctrl-c",
                "Back to Normal mode; what's written stays as the chat's draft, on your other devices too",
            ),
        ],
    ),
    (
        "Selecting text (v)",
        &[
            ("h / j / k / l", "Move a character, or a row on screen"),
            ("w / b / e", "Next word / word before / end of the word"),
            ("0 / $", "Start / end of the line"),
            ("g / G", "Start / end of the text"),
            ("Ctrl-d / Ctrl-u", "Half a page down / up"),
            (
                "v",
                "Start selecting where the cursor is, or stop; it takes in both ends, as in vim",
            ),
            ("y", "Copy what's selected and close"),
            ("Esc", "Stop selecting, then close"),
        ],
    ),
    (
        "Stickers",
        &[
            ("h / j / k / l", "Move"),
            ("H / L", "Previous / next tab: Recent, Favorites, your sets"),
            ("Enter", "Send the sticker, or the reply"),
            ("/", "Find stickers by emoji or word"),
            ("Tab / Esc", "Back to writing"),
        ],
    ),
    (
        "Photo viewer",
        &[
            ("h / l", "The photo before / after it in the chat"),
            (
                "j / k",
                "Zoom in / out (also + / -), up to filling the window",
            ),
            ("o", "Open the photo in your computer's viewer"),
            ("y", "Copy the photo"),
            ("Enter / Esc / q", "Close"),
        ],
    ),
    (
        "Video player",
        &[
            (
                "Space",
                "Play (once) or pause, right in the message (size in Settings)",
            ),
            ("z", "Twice as big in the chat, or back"),
            ("j / k", "Zoom in / out, on the whole window"),
            ("o", "Open the video in your computer's player"),
            ("Enter", "Fill the window; it closes once played"),
            ("Esc / q", "Close"),
        ],
    ),
    (
        "Bot buttons",
        &[
            ("h / j / k / l", "Move, as the buttons are laid out"),
            ("Tab / Shift-Tab", "Next / previous button"),
            (
                "Enter",
                "Press it: the bot answers, a link opens (asking first), a reply button sends its words",
            ),
            ("Esc / q", "Close"),
        ],
    ),
    (
        "Secret chats",
        &[
            (
                ":secret",
                "Start one with the person in this chat (asks first); it's kept on this computer only",
            ),
            (
                ":key",
                "Its key as a picture, to compare with the other person's",
            ),
            (":timer", "How long new messages last once they're seen"),
            (
                ":leave",
                "End it for both of you, and delete it here (asks first)",
            ),
            (
                "Enter",
                "On a 🔥 photo: view it; it's covered again when you move on",
            ),
        ],
    ),
    (
        "Search and command prompts",
        &[
            ("Enter", "Search, keep the chat filter, or run the command"),
            (
                "Tab / Shift-Tab",
                "Complete a command or a search filter (again for the next one), or the path of a file to attach",
            ),
            ("Esc / Ctrl-c", "Cancel"),
        ],
    ),
    (
        "Forward and find popups",
        &[
            ("Type", "Search chats by name, or @username"),
            ("Up / Down", "Move (also Ctrl-p / Ctrl-n, Tab)"),
            ("Enter", "Forward there, or open it"),
            ("Esc", "Cancel"),
        ],
    ),
    (
        "Menus and popups",
        &[
            ("j / k", "Move"),
            ("Enter", "Choose"),
            ("1-9", "Choose by number"),
            (
                "Space",
                "Tick a setting, or use a sound or a theme (Enter too); saved at once",
            ),
            ("Esc / q", "Close"),
            ("Tab / h / l", "Switch tabs in this popup"),
            (
                "y / n",
                "Go ahead or not, when asked about a file, a link or joining",
            ),
        ],
    ),
];

/// The same table in Russian. One row per English row, in the same order,
/// so both tables size the popup alike.
const SHORTCUTS_RU: &[(&str, &[(&str, &str)])] = &[
    (
        "Везде",
        &[
            ("?", "Эта справка и настройки"),
            (
                ":",
                "Команда: info, secret, key, timer, leave, proxy или logout",
            ),
            (
                "s",
                "Найти чат или человека: имя, @username или ссылка t.me",
            ),
            (
                "I",
                "Что это за чат и кто в нём: Enter на человеке — написать ему",
            ),
            ("H", "Выделить чат или снять выделение"),
            (
                "Ctrl-o / Ctrl-i",
                "Назад к чату или ответу, откуда ушли / снова вперёд",
            ),
            (
                "Ctrl-r",
                "Ширина панелей: h / l, = как было, Enter сохранить, Esc отмена",
            ),
            ("q", "Выход (ещё раз — не ждать)"),
            ("Ctrl-c", "Выход, кроме режима набора текста"),
            (
                "",
                "Клавиши работают и в русской раскладке: н — это y, р — это h; при наборе текста буквы как есть",
            ),
        ],
    ),
    (
        "Список чатов",
        &[
            ("j / k", "Вниз / вверх"),
            ("gg / G", "Первый / последний чат"),
            ("Ctrl-d / Ctrl-u", "Полстраницы вниз / вверх"),
            (
                "Enter / l",
                "Открыть чат на первом непрочитанном (Enter / h, если список справа)",
            ),
            ("i", "Открыть чат и писать"),
            ("gm", "Открыть чат на сообщении с упоминанием вас"),
            (
                "Tab / Shift-Tab",
                "Следующая / предыдущая папка, затем архив",
            ),
            ("/", "Фильтр чатов по имени"),
            ("p", "Закрепить / открепить чат (на всех устройствах)"),
            ("m", "Выключить / включить уведомления чата"),
            ("a", "В архив или обратно из архива"),
            ("Esc", "Сбросить фильтр"),
        ],
    ),
    (
        "Темы форума",
        &[
            ("j / k", "Вниз / вверх"),
            ("gg / G", "Первая / последняя тема"),
            (
                "Enter / l",
                "Открыть сообщения темы (Enter / h, если список справа)",
            ),
            ("i", "Открыть тему и писать"),
            ("Ctrl-r", "Ширина панели тем: h / l, = как было"),
            ("h / Esc", "Назад к списку чатов (l, если список справа)"),
        ],
    ),
    (
        "Сообщения",
        &[
            ("j / k", "Новее / старее"),
            ("gg / G", "Самое старое / новое"),
            ("Ctrl-d / Ctrl-u", "Полстраницы новее / старее"),
            (
                "Enter",
                "Отправить неотправленное заново, показать спойлеры, открыть фото «на один раз», играть/пауза голосового, голосовать в опросе, кнопки бота, показать фото, открыть файл или ссылку",
            ),
            ("y", "Скопировать текст, ссылку, фото или файл"),
            ("O", "Показать файл в файловом менеджере"),
            ("v", "Выделить часть текста и скопировать её"),
            ("r", "Ответить"),
            ("f", "Переслать в другой чат (альбом — целиком)"),
            ("e", "Изменить своё сообщение"),
            ("R", "Реакция, или убрать свою (/ ищет эмодзи по имени)"),
            ("X", "Убрать все свои реакции"),
            ("d", "Удалить"),
            ("P", "Закрепить сообщение (спросит как) или открепить"),
            ("gp", "Закреплённые: Enter — перейти, P — открепить"),
            ("gd", "К сообщению, на которое отвечает этот ответ"),
            (
                "gu",
                "К первому непрочитанному, под чертой «Непрочитанные сообщения»",
            ),
            (
                "gm / gM",
                "К самому старому непрочитанному с упоминанием вас или ответом вам (@ в списке чатов), просмотр читает его; когда все просмотрены — к сообщению до / после курсора",
            ),
            (
                "Tab",
                "Снова вперёд, как Ctrl-i (большинство терминалов шлют Ctrl-i как Tab)",
            ),
            ("i", "Написать сообщение"),
            ("a", "Приложить файл по пути (Tab дополняет)"),
            ("p", "Вставить фото, файлы или текст из буфера"),
            (
                "/",
                "Поиск по всему чату; с has:/type: — все совпадения списком, как «Вложения»",
            ),
            (
                "from: has:",
                "В поиске: from:@alice, from:me, from:this или from:имя; has:photo (video, media, file, link, voice, gif, audio)",
            ),
            (
                "before: after:",
                "В поиске: before:2025-10-01, after:2025-09-01 (тот день считается)",
            ),
            ("n / N", "Следующее старое / новое совпадение"),
            (
                "Esc",
                "Закончить поиск, отменить правку, убрать файлы, отменить ответ или назад",
            ),
            (
                "h",
                "Назад к списку чатов или темам форума (l, если список справа)",
            ),
        ],
    ),
    (
        "Набор текста",
        &[
            ("Enter", "Отправить или сохранить правку"),
            ("Alt-Enter / Ctrl-j", "Новая строка"),
            ("Ctrl-u", "Стереть до начала строки"),
            (
                "**жирный**",
                "Разметка: __курсив__ ~~зачёркнутый~~ ||спойлер|| `код` ```блок``` [текст](url)",
            ),
            (
                "@имя / :smile",
                "Подскажет людей группы или эмодзи; Tab вставляет",
            ),
            ("/команда", "В начале сообщения: команды ботов"),
            ("Ctrl-v", "Вставить фото, файлы или текст из буфера"),
            (
                "Drop a file",
                "Приложить его; написанный текст станет подписью",
            ),
            ("Ctrl-z", "Вернуть вставленные файлы обратно в текст пути"),
            ("Ctrl-t", "Фото как файлы, без сжатия, или обратно"),
            ("Tab", "Стикеры"),
            (
                "Esc / Ctrl-c",
                "Назад в обычный режим; написанное останется черновиком чата, и на других устройствах тоже",
            ),
        ],
    ),
    (
        "Выделение текста (v)",
        &[
            ("h / j / k / l", "Символ или строка на экране"),
            ("w / b / e", "Следующее слово / слово перед / конец слова"),
            ("0 / $", "Начало / конец строки"),
            ("g / G", "Начало / конец текста"),
            ("Ctrl-d / Ctrl-u", "Полстраницы вниз / вверх"),
            (
                "v",
                "Начать выделение с курсора или закончить; захватывает оба конца, как в vim",
            ),
            ("y", "Скопировать выделенное и закрыть"),
            ("Esc", "Закончить выделение, затем закрыть"),
        ],
    ),
    (
        "Стикеры",
        &[
            ("h / j / k / l", "Движение"),
            ("H / L", "Вкладки: Недавние, Избранное, ваши наборы"),
            ("Enter", "Отправить стикер или ответ"),
            ("/", "Найти стикеры по эмодзи или слову"),
            ("Tab / Esc", "Назад к набору текста"),
        ],
    ),
    (
        "Просмотр фото",
        &[
            ("h / l", "Фото до / после в чате"),
            (
                "j / k",
                "Ближе / дальше (также + / -), вплоть до всего окна",
            ),
            ("o", "Открыть фото в просмотрщике компьютера"),
            ("y", "Скопировать фото"),
            ("Enter / Esc / q", "Закрыть"),
        ],
    ),
    (
        "Видеоплеер",
        &[
            (
                "Space",
                "Играть (один раз) / пауза, прямо в сообщении (размер — в настройках)",
            ),
            ("z", "Вдвое больше в чате, или обратно"),
            ("j / k", "Ближе / дальше, на всём окне"),
            ("o", "Открыть видео в плеере компьютера"),
            ("Enter", "На всё окно; закроется после проигрывания"),
            ("Esc / q", "Закрыть"),
        ],
    ),
    (
        "Кнопки ботов",
        &[
            ("h / j / k / l", "Движение по кнопкам, как они разложены"),
            ("Tab / Shift-Tab", "Следующая / предыдущая кнопка"),
            (
                "Enter",
                "Нажать: бот ответит, ссылка откроется (спросив), кнопка-ответ пошлёт свои слова",
            ),
            ("Esc / q", "Закрыть"),
        ],
    ),
    (
        "Секретные чаты",
        &[
            (
                ":secret",
                "Начать с человеком из этого чата (спросит); хранится только на этом компьютере",
            ),
            (":key", "Его ключ картинкой, чтобы сверить с собеседником"),
            (":timer", "Сколько живут новые сообщения после прочтения"),
            (":leave", "Завершить для обоих и удалить здесь (спросит)"),
            (
                "Enter",
                "На фото 🔥: посмотреть; при переходе дальше снова скроется",
            ),
        ],
    ),
    (
        "Строки поиска и команд",
        &[
            (
                "Enter",
                "Искать, оставить фильтр чата или выполнить команду",
            ),
            (
                "Tab / Shift-Tab",
                "Дополнить команду или фильтр поиска (ещё раз — следующий), или путь файла для вложения",
            ),
            ("Esc / Ctrl-c", "Отмена"),
        ],
    ),
    (
        "Окна пересылки и поиска",
        &[
            ("Печатайте", "Поиск чатов по имени или @username"),
            ("Up / Down", "Движение (также Ctrl-p / Ctrl-n, Tab)"),
            ("Enter", "Переслать туда или открыть"),
            ("Esc", "Отмена"),
        ],
    ),
    (
        "Меню и окна",
        &[
            ("j / k", "Движение"),
            ("Enter", "Выбрать"),
            ("1-9", "Выбрать по номеру"),
            (
                "Space",
                "Отметить настройку или применить звук или тему (Enter тоже); сохраняется сразу",
            ),
            ("Esc / q", "Закрыть"),
            ("Tab / h / l", "Вкладки этого окна"),
            (
                "y / n",
                "Согласиться или нет, когда спрашивают про файл, ссылку или вступление",
            ),
        ],
    ),
];

/// The table in the language set: Russian where translated, else the
/// English original. Both hold every group, one row per row.
fn table(ru: bool) -> &'static [(&'static str, &'static [(&'static str, &'static str)])] {
    if ru { SHORTCUTS_RU } else { SHORTCUTS }
}

/// The longest key, which sets where the descriptions start.
fn key_width(ru: bool) -> usize {
    table(ru)
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .map(|(key, _)| key.width())
        .max()
        .unwrap_or(0)
}

/// The shortcuts as lines: a heading per group, then each key and what it does.
fn lines(colors: &Colors, ru: bool) -> Vec<Line<'static>> {
    let key_width = key_width(ru);
    let mut lines = Vec::new();
    for (i, (group, keys)) in table(ru).iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.push(Line::from(format!(" {group}")).fg(colors.accent).bold());
        for (key, what) in keys.iter() {
            lines.push(Line::from(vec![
                Span::from(format!("   {key:<key_width$}  ")).fg(colors.primary),
                Span::from(*what),
            ]));
        }
    }
    lines
}

/// Columns the widest line takes, plus one for the scrollbar, for sizing the
/// popup.
pub fn width(ru: bool) -> usize {
    let what = table(ru)
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .map(|(_, what)| what.width())
        .max()
        .unwrap_or(0);
    // Indent, the keys, a gap, then the descriptions.
    3 + key_width(ru) + 2 + what + 1
}

/// Rows the list takes, for sizing the popup.
pub fn height(ru: bool) -> usize {
    table(ru)
        .iter()
        .map(|(_, keys)| keys.len() + 2)
        .sum::<usize>()
        - 1
}

/// Draws the list from row `scroll`, first pulling `scroll` back if it's
/// past the end.
pub fn draw(frame: &mut Frame, area: Rect, scroll: &mut usize, ru: bool, colors: &Colors) {
    let lines = lines(colors, ru);
    let max = lines.len().saturating_sub(usize::from(area.height));
    *scroll = (*scroll).min(max);
    frame.render_widget(Paragraph::new(lines).scroll((*scroll as u16, 0)), area);
    super::scrollbar(frame, area, max, *scroll, colors);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_counts_every_line() {
        for ru in [false, true] {
            assert_eq!(height(ru), lines(&Colors::default(), ru).len());
        }
    }

    #[test]
    fn width_leaves_room_for_every_line_and_the_scrollbar() {
        for ru in [false, true] {
            let widest = lines(&Colors::default(), ru)
                .iter()
                .map(Line::width)
                .max()
                .unwrap();
            assert_eq!(width(ru), widest + 1);
        }
    }
}
