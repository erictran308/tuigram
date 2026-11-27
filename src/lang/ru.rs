//! Russian, keyed by the English it says (see [`super`]): as Telegram's
//! Russian apps word it. Grouped by the file the words are used in.

/// `en` in Russian, if it has been put.
pub(super) fn text(en: &str) -> Option<&'static str> {
    Some(match en {
        // app.rs
        "Found {request}: press s to open it" => "Найдено: {request}. Нажмите s, чтобы открыть",
        "Nobody called {name} wrote in the messages loaded: try from:@username" => {
            "Никто с именем {name} не писал в загруженных сообщениях: попробуйте from:@username"
        }
        "{name} could be {names}: use from:@username" => {
            "Под «{name}» подходят {names}: используйте from:@username"
        }
        "You're not in {title}." => "Вы не участник: {title}.",
        "channel|You're not in {title}." => "Вы не подписчик: {title}.",
        "this chat" => "этот чат",
        "channel|this chat" => "этот канал",
        "group|this chat" => "эта группа",
        "{name} changed since it was attached. Drop the files (Esc in Normal mode) and attach it again" => {
            "Файл {name} изменился после прикрепления. Уберите файлы (Esc в обычном режиме) и прикрепите его снова"
        }
        "No chats match \"{query}\"" => "Нет чатов по запросу «{query}»",
        "{who} started a secret chat, which this computer took anyway: :leave ends it" => {
            "{who} начал(а) секретный чат, и этот компьютер всё равно его принял: :leave завершает его"
        }
        "With {with}. It's end-to-end encrypted, and kept only" => {
            "Собеседник: {with}. Чат защищён сквозным шифрованием и хранится"
        }
        "on this computer and on the device of theirs that" => {
            "только на этом компьютере и на том устройстве собеседника,"
        }
        "accepts it: your other devices won't have it." => {
            "которое его примет: на других ваших устройствах его не будет."
        }
        "The key is made once {with} accepts the secret chat" => {
            "Ключ появится, когда {with} примет секретный чат"
        }
        "This ends the session on Telegram and deletes what" => {
            "Сеанс в Telegram завершится, а всё, что tuigram"
        }
        "tuigram keeps on this computer. Logging in again" => {
            "хранит на этом компьютере, удалится. Для нового входа"
        }
        "takes a code, and your password if you have one." => {
            "понадобятся код и пароль, если он у вас есть."
        }
        "No messages match \"{query}\"" => "Нет сообщений по запросу «{query}»",
        "Ask to join this channel?" => "Подать заявку на вступление в канал?",
        "Ask to join this group?" => "Подать заявку на вступление в группу?",
        "Join this group?" => "Вступить в группу?",
        "The text says: " => "В тексте:      ",
        "Full address:  " => "Полный адрес:  ",
        "{name} downloaded: press Enter on it again to open it" => {
            "Файл {name} скачан: нажмите на нём Enter ещё раз, чтобы открыть"
        }
        "The link still has {mask} for the hidden password or secret: write it out" => {
            "В ссылке всё ещё {mask} вместо скрытого пароля или секрета: впишите его"
        }
        "tuigram would stop using {what}." => "tuigram перестанет использовать {what}.",
        "open" => "открыть",
        "edit" => "изменить",
        "join" => "вступить",
        "channel|join" => "подписаться",
        "leave" => "покинуть",
        "log out" => "выйти из аккаунта",
        "start it" => "начать",
        "end it" => "завершить",
        "use it" => "использовать",
        "connect directly" => "подключиться напрямую",
        "Show what this chat is, and who's in it (also I)" => {
            "Показать, что это за чат и кто в нём (также I)"
        }
        "Show a secret chat's key, to compare with the other person's" => {
            "Показать ключ секретного чата, чтобы сверить его с ключом собеседника"
        }
        "Leave this group or channel, or end a secret chat (asks first)" => {
            "Покинуть группу или канал или завершить секретный чат (с подтверждением)"
        }
        "Log out of Telegram on this computer (asks first)" => {
            "Выйти из аккаунта Telegram на этом компьютере (с подтверждением)"
        }
        "Connect through a proxy where Telegram is blocked, or directly" => {
            "Подключаться через прокси, где Telegram заблокирован, или напрямую"
        }
        "Start a secret chat with this person (asks first)" => {
            "Начать секретный чат с этим человеком (с подтверждением)"
        }
        "Set how long messages in a secret chat last once seen" => {
            "Задать, сколько живут сообщения секретного чата после просмотра"
        }
        "No unread mentions" => "Нет непрочитанных упоминаний",
        "Nobody mentioned you here" => "Здесь вас никто не упоминал",
        "No older mentions" => "Более старых упоминаний нет",
        "No newer mentions" => "Более новых упоминаний нет",
        "Waiting for network…" => "Ожидание сети…",
        "Connecting to the proxy…" => "Подключение к прокси…",
        "Updating…" => "Обновление…",
        "to your Telegram app on another device" => "в приложение Telegram на другом устройстве",
        "by SMS" => "по SMS",
        "by phone call" => "звонком",
        "as a missed call: enter the last digits of the calling number" => {
            "пропущенным звонком: введите последние цифры номера, с которого звонили"
        }
        "to fragment.com" => "на fragment.com",
        "to one of your devices" => "на одно из ваших устройств",
        "Connecting…" => "Подключение…",
        "Whole message" => "Всё сообщение",
        "It goes to:" => "Ведёт на:",
        "Server:" => "Сервер:",
        "Delete for everyone" => "Удалить для всех",
        "Delete for me" => "Удалить для меня",
        "./.env is ignored: only development builds read it" => {
            "./.env не используется: его читают только сборки для разработки"
        }
        "Theme not used: " => "Тема не применена: ",
        "Sound not used: " => "Звук не применён: ",
        "Can't play this video message: " => "Не удаётся воспроизвести видеосообщение: ",
        "Close what's open to paste" => "Чтобы вставить, закройте то, что открыто",
        "Download failed" => "Не удалось скачать",
        "Shown in the file manager" => "Показано в файловом менеджере",
        "Voice message downloaded: Enter plays it" => {
            "Голосовое сообщение скачано: Enter воспроизводит его"
        }
        "Video message downloaded: Enter plays it" => {
            "Видеосообщение скачано: Enter воспроизводит его"
        }
        "Video message downloaded: Space plays it" => {
            "Видеосообщение скачано: Space воспроизводит его"
        }
        "tuigram can't open this kind of link" => "tuigram не открывает такие ссылки",
        "to" => "в чат",
        "Forwarded" => "Переслано",
        "Joined" => "Вы вступили",
        "with" => "Собеседник:",
        "Ended the secret chat" => "Секретный чат завершён",
        "Left" => "Вы вышли",
        "That message isn't in this topic" => "Этого сообщения нет в этой теме",
        "Message not sent: " => "Сообщение не отправлено: ",
        "The message was edited: v selects again" => "Сообщение изменено: v выделяет заново",
        "This number has no Telegram account. Sign up in the official app first." => {
            "С этим номером нет аккаунта Telegram. Сначала зарегистрируйтесь в официальном приложении."
        }
        "Telegram requires a Premium purchase to log in with this number. Use the official app." => {
            "Чтобы войти с этим номером, Telegram требует купить Premium. Используйте официальное приложение."
        }
        "The API ID is a number, like 1234567" => "API ID — это число, например 1234567",
        "The API hash is 32 letters and digits" => "API hash — это 32 буквы и цифры",
        "Couldn't save them: " => "Не удалось их сохранить: ",
        "Telegram rejected TG_API_ID / TG_API_HASH." => {
            "Telegram отклонил TG_API_ID / TG_API_HASH."
        }
        "Telegram rejected tuigram's built-in key. Use your own." => {
            "Telegram отклонил встроенный ключ tuigram. Используйте свой."
        }
        "Telegram rejected that API ID and hash." => "Telegram отклонил этот API ID и hash.",
        "Telegram rejected the API key; can't forget it: " => {
            "Telegram отклонил API-ключ; не удаётся его забыть: "
        }
        "Your account isn't known yet" => "Ваш аккаунт ещё не известен",
        "Join this channel?" => "Подписаться на канал?",
        "Join it to have it in your chat list." => "Подпишитесь, чтобы он был в списке чатов.",
        "Join to write here?" => "Вступить, чтобы писать здесь?",
        "Join it to write, and to have it in your chat list." => {
            "Вступите, чтобы писать здесь и видеть группу в списке чатов."
        }
        "them" => "собеседник",
        "from: doesn't work in secret chats" => "from: не работает в секретных чатах",
        "Not a command: " => "Нет такой команды: ",
        "Someone" => "Кто-то",
        "Secret chat" => "Секретный чат",
        "Someone started a secret chat with you" => "С вами начали секретный чат",
        "New message" => "Новое сообщение",
        "Open the group or channel to leave first" => {
            "Сначала откройте группу или канал, который хотите покинуть"
        }
        "End this secret chat?" => "Завершить секретный чат?",
        "With" => "Собеседник:",
        "Nothing more can be sent in it, by either of you," => {
            "Никто из вас больше не сможет в нём писать,"
        }
        "and its messages are deleted from this computer." => {
            "а его сообщения удалятся с этого компьютера."
        }
        "A chat with one person can't be left" => "Из чата с одним человеком нельзя выйти",
        "You're not in this chat" => "Вы не состоите в этом чате",
        "Leave this channel?" => "Покинуть канал?",
        "Its posts stop coming to you." => "Его публикации перестанут к вам приходить.",
        "Leave this group?" => "Покинуть группу?",
        "Its messages stop coming to you." => "Её сообщения перестанут к вам приходить.",
        "Open a chat with someone to start a secret chat with them" => {
            "Откройте чат с человеком, чтобы начать с ним секретный чат"
        }
        "Bots can't be in secret chats" => "С ботами нельзя вести секретные чаты",
        "Start a secret chat?" => "Начать секретный чат?",
        "Open a secret chat first (:secret starts one)" => {
            "Сначала откройте секретный чат (:secret начинает новый)"
        }
        "Telegram gave no key to compare for this chat" => {
            "Telegram не дал ключа для сверки в этом чате"
        }
        "Your secret chats go too: they're kept only here." => {
            "Секретные чаты тоже пропадут: они хранятся только здесь."
        }
        "Log out?" => "Выйти из аккаунта?",
        "Couldn't save settings: " => "Не удалось сохранить настройки: ",
        "No newer matches" => "Более новых совпадений нет",
        "No older matches" => "Более старых совпадений нет",
        "Nothing to go back to" => "Некуда возвращаться",
        "Nothing to go forward to" => "Некуда идти вперёд",
        "Wait until it's sent" => "Дождитесь отправки",
        "This message wasn't sent" => "Это сообщение не отправлено",
        "You can't pin messages here" => "Здесь нельзя закреплять сообщения",
        "No pinned messages in this chat" => "В этом чате нет закреплённых сообщений",
        "Downloading to show in the file manager…" => {
            "Скачивание, чтобы показать в файловом менеджере…"
        }
        "No file in this message" => "В этом сообщении нет файла",
        "tuigram can't show this only while it's open" => {
            "tuigram не умеет показывать то, что видно только пока открыто"
        }
        "Nothing to open in this message" => "В этом сообщении нечего открыть",
        "Telegram won't take this message again: `d` deletes it" => {
            "Telegram не примет это сообщение снова: `d` удаляет его"
        }
        "Can't reply to a message that wasn't sent" => {
            "Нельзя ответить на неотправленное сообщение"
        }
        "The edit came while something else was open: e edits again" => {
            "Правка пришла, пока было открыто другое: e правит снова"
        }
        "You can't edit this message" => "Это сообщение нельзя изменить",
        "Edit and lose some formatting?" => "Изменить и потерять часть форматирования?",
        "Some of its formatting can't be written in Markdown" => {
            "Часть его форматирования не записать в Markdown"
        }
        "(underline, custom emoji…), so an edit would lose it." => {
            "(подчёркивание, свои эмодзи…), и правка её потеряет."
        }
        "A message can't be empty (d deletes it)" => {
            "Сообщение не может быть пустым (d удаляет его)"
        }
        "You can't delete this message" => "Это сообщение нельзя удалить",
        "Reactions are off in this chat" => "В этом чате реакции выключены",
        "Pressed" => "Нажато",
        "That message isn't loaded any more" => "Это сообщение больше не загружено",
        "The bot's link isn't a web address" => "Ссылка бота — не веб-адрес",
        "Messages in secret chats can't be forwarded" => {
            "Сообщения из секретных чатов нельзя пересылать"
        }
        "your contact" => "ваш контакт",
        "An admin has to let you in." => "Заявку должен одобрить администратор.",
        "You haven't reacted to this message" => "Вы не ставили реакций на это сообщение",
        "An edit can't change a message into a sticker" => {
            "Правка не может превратить сообщение в стикер"
        }
        "No unread messages" => "Нет непрочитанных сообщений",
        "No chat selected" => "Чат не выбран",
        "This photo can't be shown here" => "Это фото нельзя показать здесь",
        "This photo can't be saved, so it opens only here" => {
            "Это фото нельзя сохранить, поэтому оно открывается только здесь"
        }
        "Loading older messages…" => "Загрузка более старых сообщений…",
        "Loading newer messages…" => "Загрузка более новых сообщений…",
        "No older photos in this chat" => "Более старых фото в этом чате нет",
        "No newer photos in this chat" => "Более новых фото в этом чате нет",
        "Video messages play in the terminal only with kitty's protocol" => {
            "Видеосообщения воспроизводятся в терминале только с протоколом kitty"
        }
        "Video messages play in the terminal only on this computer" => {
            "Видеосообщения воспроизводятся в терминале только на этом компьютере"
        }
        "This video message can't be played here" => {
            "Это видеосообщение нельзя воспроизвести здесь"
        }
        "Can't play this video message: it's too big" => {
            "Не удаётся воспроизвести видеосообщение: оно слишком большое"
        }
        "This terminal can't play video messages in the chat; Enter plays one on the whole window" => {
            "Этот терминал не умеет воспроизводить видеосообщения в чате; Enter воспроизводит видеосообщение на всё окно"
        }
        "Downloading the video message…" => "Скачивание видеосообщения…",
        "This video message can't be saved, so it plays only here" => {
            "Это видеосообщение нельзя сохранить, поэтому оно воспроизводится только здесь"
        }
        "Can't play this voice message: it's too long" => {
            "Не удаётся воспроизвести голосовое сообщение: оно слишком длинное"
        }
        "(has non-Latin letters)" => "(есть нелатинские буквы)",
        "Open this link?" => "Открыть ссылку?",
        "Open" => "Открыть",
        "Files like this can run programs on your computer." => {
            "Такие файлы могут запускать программы на вашем компьютере."
        }
        "Only open it if you trust whoever sent it." => {
            "Открывайте, только если доверяете отправителю."
        }
        "Couldn't open it: " => "Не удалось открыть: ",
        "a secret chat with" => "секретный чат —",
        "Nothing to copy in this message" => "В этом сообщении нечего копировать",
        "No text to select in this message" => "В этом сообщении нет текста для выделения",
        "Nothing selected: v starts a selection" => "Ничего не выделено: v начинает выделение",
        "Over SSH, only text reaches your computer's clipboard" => {
            "По SSH в буфер обмена вашего компьютера попадает только текст"
        }
        "Copied" => "Скопировано",
        "Sent to the terminal's clipboard" => "Отправлено в буфер обмена терминала",
        "Couldn't copy: " => "Не удалось скопировать: ",
        "Can't play the sound for new messages: " => {
            "Не удаётся воспроизвести звук новых сообщений: "
        }
        "Can't play this voice message: " => "Не удаётся воспроизвести голосовое сообщение: ",
        "Files can't be added to an edit" => "К правке нельзя добавить файлы",
        "Over SSH, paste with your terminal's own paste key: the clipboard here is the other computer's" => {
            "По SSH вставляйте клавишей вставки самого терминала: буфер обмена здесь — другого компьютера"
        }
        "The paste came after you left the messages: p pastes again" => {
            "Вставка пришла, когда вы уже ушли из сообщений: p вставляет снова"
        }
        "The paste came while something else was open: p pastes again" => {
            "Вставка пришла, пока было открыто другое: p вставляет снова"
        }
        "Copied files without a full path were left out" => {
            "Скопированные файлы без полного пути пропущены"
        }
        "Pasted image" => "Вставленное изображение",
        "These go as files already: only photos can go either way" => {
            "Они и так уйдут файлами: двумя способами можно отправить только фото"
        }
        "Over SSH, tuigram plays no sound: it would play on the other computer" => {
            "По SSH tuigram не воспроизводит звук: он зазвучал бы на другом компьютере"
        }
        "Settings not saved: " => "Настройки не сохранены: ",
        "No folders yet: Telegram's apps can make them" => {
            "Папок пока нет: их можно создать в приложениях Telegram"
        }
        "the proxy in settings.toml" => "прокси из settings.toml",
        "TG_PROXY sets the proxy: change it there" => "Прокси задан в TG_PROXY: меняйте его там",
        "No proxy: tuigram connects to Telegram directly" => {
            "Прокси нет: tuigram подключается к Telegram напрямую"
        }
        "the proxy" => "прокси",
        "Connect directly?" => "Подключаться напрямую?",
        "Telegram would see your IP address, and your network that you use Telegram." => {
            "Telegram увидит ваш IP-адрес, а ваша сеть — что вы пользуетесь Telegram."
        }
        "Proxy:         " => "Прокси:        ",
        "tuigram would connect to Telegram through it. It sees your IP address and when you use Telegram, not your messages." => {
            "tuigram будет подключаться к Telegram через него. Он видит ваш IP-адрес и когда вы пользуетесь Telegram, но не ваши сообщения."
        }
        "It can also put a channel it promotes in your chat list, marked \"proxy sponsor\"." => {
            "Он также может добавить в список чатов продвигаемый им канал с пометкой «спонсор прокси»."
        }
        "`:proxy` changes it, or goes back to connecting directly." => {
            "`:proxy` меняет его или возвращает прямое подключение."
        }
        "Use this proxy?" => "Использовать этот прокси?",
        "Switching to" => "Переключение на",
        "Switching to connecting directly…" => "Переключение на прямое подключение…",
        "Proxy not saved for next time: " => "Прокси не сохранён на следующий раз: ",
        "Connecting through" => "Подключение через",
        "No proxy" => "Без прокси",
        "Connecting to Telegram directly" => "Подключение к Telegram напрямую",
        "The proxy wasn't changed: " => "Прокси не изменён: ",
        "Moved out of the archive" => "Возвращено из архива",
        "Archived" => "Перенесено в архив",
        "Highlight not saved: " => "Подсветка не сохранена: ",
        "channel|Joined" => "Вы подписались",
        // attach.rs
        "{name} is a folder" => "{name} — это папка",
        "{name} is empty" => "{name} — пустой файл",
        "Can't send {name}: its path isn't valid UTF-8" => {
            "Не удаётся отправить {name}: путь к нему не в UTF-8"
        }
        "KB" => "КБ",
        "MB" => "МБ",
        "GB" => "ГБ",
        "TB" => "ТБ",
        "No such file" => "Нет такого файла",
        "Can't read" => "Не удаётся прочитать",
        "B" => "Б",
        // buttons.rs
        "Presses \"{label}\": the bot decides what happens" => {
            "Нажимает «{label}»: что будет, решает бот"
        }
        "Opens {host} in your browser, asking first" => {
            "Открывает {host} в браузере, сначала спросив"
        }
        "Opens {url} in tuigram" => "Открывает {url} в tuigram",
        "Opens your chat with {name}" => "Открывает ваш чат с пользователем: {name}",
        "Copies" => "Копирует",
        "Sends as you" => "Отправляет от вашего имени",
        "Opens" => "Открывает",
        "Only Telegram's own apps can do this" => "Это умеют только приложения Telegram",
        "Web apps only open in Telegram's own apps" => {
            "Мини-приложения открываются только в приложениях Telegram"
        }
        "Games only run in Telegram's own apps" => "Игры работают только в приложениях Telegram",
        "Payments only work in Telegram's own apps" => {
            "Платежи работают только в приложениях Telegram"
        }
        "This button asks for your password: use Telegram's own apps" => {
            "Эта кнопка запрашивает ваш пароль: используйте приложения Telegram"
        }
        "This button's link isn't a web address" => "Ссылка этой кнопки — не веб-адрес",
        "Open link" => "Открыть ссылку",
        // chats.rs
        "typing" => "печатает",
        "several|typing" => "печатают",
        "recording a video" => "записывает видео",
        "several|recording a video" => "записывают видео",
        "sending a video" => "отправляет видео",
        "several|sending a video" => "отправляют видео",
        "recording a voice message" => "записывает голосовое",
        "several|recording a voice message" => "записывают голосовое",
        "sending a voice message" => "отправляет голосовое",
        "several|sending a voice message" => "отправляют голосовое",
        "sending a photo" => "отправляет фото",
        "several|sending a photo" => "отправляют фото",
        "sending a file" => "отправляет файл",
        "several|sending a file" => "отправляют файл",
        "choosing a sticker" => "выбирает стикер",
        "several|choosing a sticker" => "выбирают стикер",
        "choosing a location" => "выбирает геопозицию",
        "several|choosing a location" => "выбирают геопозицию",
        "choosing a contact" => "выбирает контакт",
        "several|choosing a contact" => "выбирают контакт",
        "playing a game" => "играет в игру",
        "several|playing a game" => "играют в игру",
        "recording a video message" => "записывает видеосообщение",
        "several|recording a video message" => "записывают видеосообщение",
        "sending a video message" => "отправляет видеосообщение",
        "several|sending a video message" => "отправляют видеосообщение",
        "All" => "Все чаты",
        "Archive" => "Архив",
        " SCAM" => " МОШЕННИК",
        " FAKE" => " ФЕЙК",
        "Saved Messages" => "Избранное",
        "You" => "Вы",
        "Photo" => "Фото",
        "Video" => "Видео",
        "File" => "Файл",
        "Audio" => "Аудио",
        "Voice message" => "Голосовое сообщение",
        "Video message" => "Видеосообщение",
        "Sticker" => "Стикер",
        "Poll" => "Опрос",
        "Location" => "Геопозиция",
        "Contact" => "Контакт",
        "Photo expired" => "Срок действия фото истёк",
        "Video expired" => "Срок действия видео истёк",
        "Video message expired" => "Срок действия видеосообщения истёк",
        "Voice message expired" => "Срок действия голосового сообщения истёк",
        "Message" => "Сообщение",
        // chime.rs
        "there is no sound called {name}" => "нет звука с именем {name}",
        "{bits}-bit samples can't be played" => "{bits}-битные сэмплы не воспроизводятся",
        "Ding" => "Динь",
        "Chime" => "Перезвон",
        "Trill" => "Трель",
        "Pop" => "Хлопок",
        "Knock" => "Стук",
        "it's neither WAV nor Ogg" => "это не WAV и не Ogg",
        "it isn't a WAV file" => "это не файл WAV",
        "its sound comes before its format" => "звук в нём идёт раньше формата",
        "it has no sound" => "в нём нет звука",
        "its format is cut short" => "его формат обрезан",
        "it's compressed, which can't be played: save it as PCM" => {
            "он сжат, такой не воспроизводится: сохраните его как PCM"
        }
        "its format is broken" => "его формат повреждён",
        "it couldn't be decoded" => "не удалось его декодировать",
        // clipboard.rs
        "Can't reach the clipboard" => "Нет доступа к буферу обмена",
        "The image on the clipboard is broken" => "Изображение в буфере обмена повреждено",
        "Can't save the image" => "Не удаётся сохранить изображение",
        "Nothing to paste: the clipboard is empty" => "Нечего вставить: буфер обмена пуст",
        // config.rs
        "not a file" => "это не файл",
        "bigger than" => "больше",
        // images.rs
        "the image couldn't be decoded" => "не удалось декодировать изображение",
        // info.rs
        "Couldn't get its members" => "Не удалось получить список участников",
        "Its admins hid who's in it" => "Администраторы скрыли, кто здесь состоит",
        "Only its admins can see who's in it" => "Кто здесь состоит, видят только администраторы",
        // messages.rs
        "watch it in Telegram on your phone" => "смотрите в Telegram на телефоне",
        "[Photo · Enter to view]" => "[Фото · Enter — открыть]",
        "This message has no text to edit" => "В этом сообщении нет текста для изменения",
        "No message selected" => "Сообщение не выбрано",
        "Not a reply" => "Это не ответ",
        "It answers a message in another chat" => "Это ответ на сообщение из другого чата",
        "The message it answers was deleted" => "Сообщение, на которое это ответ, удалено",
        // notify.rs
        // pins.rs
        "Pin for me and {name}" => "Закрепить у обоих ({name})",
        "Pin" => "Закрепить",
        "Pin just for me" => "Закрепить только у меня",
        "Pin and notify subscribers" => "Закрепить и уведомить подписчиков",
        "Pin and notify members" => "Закрепить и уведомить участников",
        "Pin without notifying" => "Закрепить без уведомления",
        // poll.rs
        "This poll is closed" => "Этот опрос завершён",
        "You answered this quiz" => "Вы уже ответили в этой викторине",
        "Anonymous quiz" => "Анонимная викторина",
        "Anonymous poll" => "Анонимный опрос",
        "Quiz" => "Викторина",
        "pick several" => "несколько ответов",
        "closed" => "завершён",
        "quiz|This poll is closed" => "Эта викторина завершена",
        "quiz|closed" => "завершена",
        // proxy.rs
        "{kind} proxy {address}" => "прокси {kind} {address}",
        "The link has no secret" => "В ссылке нет секрета",
        "Not a proxy link: give socks5://host:port, http://host:port or a t.me/proxy link" => {
            "Это не ссылка на прокси: укажите socks5://host:port, http://host:port или ссылку t.me/proxy"
        }
        "A [ in the address isn't closed" => "Скобка [ в адресе не закрыта",
        "The link has no server" => "В ссылке нет сервера",
        "The server isn't a host name or an address" => "Сервер — не имя хоста и не адрес",
        "The link has no port, as in host:1080" => "В ссылке нет порта, как в host:1080",
        "The port must be a number from 1 to 65535" => "Порт должен быть числом от 1 до 65535",
        "The link's user name, password or secret can't be used" => {
            "Имя пользователя, пароль или секрет в ссылке не подходят"
        }
        "The proxy's secret isn't hex or base64" => "Секрет прокси — не hex и не base64",
        "The proxy's secret isn't one Telegram's proxies use" => {
            "Прокси Telegram не используют такой секрет"
        }
        "A % in the link isn't followed by two hex digits" => {
            "После % в ссылке должны идти две шестнадцатеричные цифры"
        }
        "The link isn't valid text" => "В ссылке недопустимый текст",
        // reactions.rs
        "You post anonymously here, so you can't react" => {
            "Вы пишете здесь анонимно, поэтому не можете ставить реакции"
        }
        "Join the chat to react" => "Вступите в чат, чтобы ставить реакции",
        // search.rs
        "Only one {key} at a time" => "{key} можно указать только один раз",
        "Dates are like 2025-10-01, not \"{value}\"" => {
            "Даты пишутся как 2025-10-01, а не «{value}»"
        }
        "has: takes photo, video, media, file, link, voice, note, gif or audio" => {
            "has: принимает photo, video, media, file, link, voice, note, gif или audio"
        }
        "Add words, from: or has: to search for" => "Добавьте слова, from: или has:, чтобы искать",
        "after: has to be a day before before:" => {
            "День в after: должен быть раньше, чем в before:"
        }
        "from: takes me, @username or a name, like from:@alice" => {
            "from: принимает me, @username или имя, например from:@alice"
        }
        "of" => "из",
        "searching…" => "ищу…",
        // secret.rs
        "Off" => "Выкл.",
        "Waiting for {name} to come online" => "Ждём, когда {name} появится в сети",
        "{n}s" => "{n}с",
        "{n}m" => "{n}м",
        "{n}h" => "{n}ч",
        "{n}d" => "{n}д",
        "{n}w" => "{n}нед",
        "This secret chat has ended" => "Этот секретный чат завершён",
        "off" => "выкл",
        "🔥 once" => "🔥 один раз",
        // service.rs
        "joined via an invite link" => "вступил(а) в группу по ссылке-приглашению",
        "you|joined via an invite link" => "вступили в группу по ссылке-приглашению",
        "joined, approved by an admin" => "вступил(а) с одобрения администратора",
        "you|joined, approved by an admin" => "вступили с одобрения администратора",
        "created the group" => "создал(а) группу",
        "you|created the group" => "создали группу",
        "created" => "создал(а)",
        "you|created" => "создали",
        "changed the name to" => "изменил(а) название на",
        "you|changed the name to" => "изменили название на",
        "changed the photo" => "изменил(а) фото",
        "you|changed the photo" => "изменили фото",
        "removed the photo" => "удалил(а) фото",
        "you|removed the photo" => "удалили фото",
        "upgraded the group to a supergroup" => "преобразовал(а) группу в супергруппу",
        "you|upgraded the group to a supergroup" => "преобразовали группу в супергруппу",
        "pinned a message" => "закрепил(а) сообщение",
        "you|pinned a message" => "закрепили сообщение",
        "took a screenshot" => "сделал(а) скриншот",
        "you|took a screenshot" => "сделали скриншот",
        "turned the timer off" => "выключил(а) таймер самоуничтожения",
        "you|turned the timer off" => "выключили таймер самоуничтожения",
        "set messages to disappear after {time}" => "установил(а) таймер самоуничтожения: {time}",
        "you|set messages to disappear after {time}" => "установили таймер самоуничтожения: {time}",
        "scheduled a video chat" => "запланировал(а) видеочат",
        "you|scheduled a video chat" => "запланировали видеочат",
        "started a video chat" => "начал(а) видеочат",
        "you|started a video chat" => "начали видеочат",
        "ended the video chat ({length})" => "завершил(а) видеочат ({length})",
        "you|ended the video chat ({length})" => "завершили видеочат ({length})",
        "invited people to the video chat" => "пригласил(а) участников в видеочат",
        "you|invited people to the video chat" => "пригласили участников в видеочат",
        "changed the chat theme" => "изменил(а) оформление чата",
        "you|changed the chat theme" => "изменили оформление чата",
        "changed the chat background" => "изменил(а) обои чата",
        "you|changed the chat background" => "изменили обои чата",
        "boosted the group {times}" => "забустил(а) группу {times}",
        "you|boosted the group {times}" => "забустили группу {times}",
        "boosted the group" => "забустил(а) группу",
        "you|boosted the group" => "забустили группу",
        "created the topic" => "создал(а) тему",
        "you|created the topic" => "создали тему",
        "edited a topic" => "изменил(а) тему",
        "you|edited a topic" => "изменили тему",
        "closed a topic" => "закрыл(а) тему",
        "you|closed a topic" => "закрыли тему",
        "reopened a topic" => "снова открыл(а) тему",
        "you|reopened a topic" => "снова открыли тему",
        "hid the General topic" => "скрыл(а) тему «General»",
        "you|hid the General topic" => "скрыли тему «General»",
        "showed the General topic" => "показал(а) тему «General»",
        "you|showed the General topic" => "показали тему «General»",
        "scored {points}" => "набрал(а) {points}",
        "you|scored {points}" => "набрали {points}",
        "joined Telegram" => "теперь в Telegram",
        "\"{title}\"" => "«{title}»",
        " joined the group" => " вступил(а) в группу",
        "you| joined the group" => " вступили в группу",
        " added " => " добавил(а) ",
        "you| added " => " добавили ",
        " left the group" => " покинул(а) группу",
        "you| left the group" => " покинули группу",
        " removed " => " удалил(а) ",
        "you| removed " => " удалили ",
        "joined the group" => "вступил(а) в группу",
        "added a member" => "добавил(а) участника",
        "left the group" => "покинул(а) группу",
        "removed a member" => "удалил(а) участника",
        "nobody" => "никого",
        " and " => " и ",
        // settings.rs
        "cannot write" => "не удаётся записать",
        // sound.rs
        "the sound device takes {format} samples, which tuigram can't make" => {
            "звуковое устройство принимает сэмплы {format}, а tuigram таких не делает"
        }
        "the sound device can't be used" => "не удаётся использовать звуковое устройство",
        "the sound device stopped" => "звуковое устройство остановилось",
        "the sound device didn't start" => "звуковое устройство не запустилось",
        "no speakers or headphones found" => "не найдены ни динамики, ни наушники",
        "there's no sound here (ALSA's libasound.so.2 isn't installed)" => {
            "звука здесь нет (не установлена libasound.so.2 из ALSA)"
        }
        "the sound device can't be opened" => "не удаётся открыть звуковое устройство",
        "error" => "ошибка",
        "the sound device didn't start again" => "звуковое устройство не запустилось снова",
        "tuigram can't play sound on this system yet" => {
            "tuigram пока не умеет воспроизводить звук в этой системе"
        }
        // stickers.rs
        "Recent" => "Недавние",
        "Favorites" => "Избранные",
        // tg.rs
        "That message can't be found" => "Это сообщение не найдено",
        "Couldn't take the secret chats others start" => {
            "Не удалось принимать здесь секретные чаты, которые начинают другие"
        }
        "Couldn't leave to your phone the secret chats others start" => {
            "Не удалось оставить телефону секретные чаты, которые начинают другие"
        }
        "Nobody on Telegram is called" => "В Telegram нет никого с именем",
        // theme.rs
        "{id}.toml: there is no theme called {parent} to inherit" => {
            "{id}.toml: нет темы {parent}, чтобы взять её за основу"
        }
        "{id}.toml: inherits goes round in a circle" => "{id}.toml: inherits ходит по кругу",
        "there is no theme called {name}" => "нет темы {name}",
        "the palette has no color called {name}" => "в палитре нет цвета {name}",
        "palette.{name} is {value}, not a color like \"#1e1e2e\"" => {
            "palette.{name} — {value}, а не цвет вида \"#1e1e2e\""
        }
        "colors.{name} is {value}, not a palette color, \"#rrggbb\" or \"reset\"" => {
            "colors.{name} — {value}, а не цвет палитры, \"#rrggbb\" или \"reset\""
        }
        "there is no color called {name} to set" => "нет цвета {name}, который можно задать",
        "it isn't UTF-8 text" => "это не текст в UTF-8",
        "line" => "строка",
        "the palette needs" => "в палитре не хватает",
        "colors.names needs seven colors" => "в colors.names нужно семь цветов",
        // ui/chat_list.rs
        "Chats ({n}) " => "Чаты ({n}) ",
        "Chats ({n} of {total}) /{filter} " => "Чаты ({n} из {total}) /{filter} ",
        " proxy sponsor" => " спонсор прокси",
        "Draft: " => "Черновик: ",
        "loading… " => "загрузка… ",
        "No chats match" => "Нет подходящих чатов",
        "Loading…" => "Загрузка…",
        "No chats here" => "Здесь нет чатов",
        // ui/help.rs
        "Everywhere" => "Везде",
        "This help, and settings" => "Эта справка и настройки",
        "Type a command: info, secret, key, timer, leave, proxy or logout" => {
            "Команда: info, secret, key, timer, leave, proxy или logout"
        }
        "Find a chat or person: a name, @username or t.me link" => {
            "Найти чат или человека: имя, @username или ссылка t.me"
        }
        "What the chat is, and who's in it: Enter on someone writes to them" => {
            "Что это за чат и кто в нём: Enter на человеке — написать ему"
        }
        "Highlight or unhighlight the selected chat" => "Подсветить чат или снять подсветку",
        "Back to the chat or reply you left / forward again" => {
            "Назад к чату или ответу, откуда ушли / снова вперёд"
        }
        "Resize the panes: h / l, = as at first, Enter keeps, Esc cancels" => {
            "Ширина панелей: h / l, = как было, Enter сохранить, Esc отмена"
        }
        "Quit (press again to stop waiting)" => "Выход (ещё раз — не ждать)",
        "Quit, except while writing" => "Выход, кроме режима набора текста",
        "Keys work in the Cyrillic layout too: н is y, р is h; typing takes letters as they are" => {
            "Клавиши работают и в русской раскладке: н — это y, р — это h; при наборе текста буквы как есть"
        }
        "Chat list" => "Список чатов",
        "Move down / up" => "Вниз / вверх",
        "First / last chat" => "Первый / последний чат",
        "Half a page down / up" => "Полстраницы вниз / вверх",
        "Open the chat, at its first unread message (Enter / h with the list on the right)" => {
            "Открыть чат на первом непрочитанном (Enter / h, если список справа)"
        }
        "Open the chat and write" => "Открыть чат и писать",
        "Open the chat at a message that mentions you" => {
            "Открыть чат на сообщении с упоминанием вас"
        }
        "Next / previous folder, then the archive" => "Следующая / предыдущая папка, затем архив",
        "Filter chats by name" => "Фильтр чатов по имени",
        "Pin or unpin the chat (on all your devices)" => {
            "Закрепить / открепить чат (на всех устройствах)"
        }
        "Mute or unmute the chat" => "Выключить / включить уведомления чата",
        "Archive the chat, or move it out of the archive" => "В архив или обратно из архива",
        "Clear the filter" => "Сбросить фильтр",
        "Forum topics" => "Темы форума",
        "First / last topic" => "Первая / последняя тема",
        "Open the topic's messages (Enter / h with the list on the right)" => {
            "Открыть сообщения темы (Enter / h, если список справа)"
        }
        "Open the topic and write" => "Открыть тему и писать",
        "Resize the topics pane: h / l, = as at first" => "Ширина панели тем: h / l, = как было",
        "Back to the chat list (l with the list on the right)" => {
            "Назад к списку чатов (l, если список справа)"
        }
        "Messages" => "Сообщения",
        "Newer / older message" => "Сообщение новее / старее",
        "Oldest / newest message" => "Самое старое / новое сообщение",
        "Half a page newer / older" => "Полстраницы новее / старее",
        "Send a message that wasn't sent again, show spoilers, view a photo shown only while open, play or pause a voice message, vote in a poll, list a bot's buttons, show the photo, or open the file or link" => {
            "Отправить неотправленное заново, показать спойлеры, открыть фото «на один раз», играть/пауза голосового, голосовать в опросе, кнопки бота, показать фото, открыть файл или ссылку"
        }
        "Copy the text, a link, or the photo or file" => "Скопировать текст, ссылку, фото или файл",
        "Show the file in the file manager" => "Показать файл в файловом менеджере",
        "Show the text to select part of it and copy that" => {
            "Выделить часть текста и скопировать её"
        }
        "Reply" => "Ответить",
        "Forward to another chat (an album goes whole)" => {
            "Переслать в другой чат (альбом — целиком)"
        }
        "Edit your message" => "Изменить своё сообщение",
        "React, or take your reaction back (/ finds an emoji by name)" => {
            "Реакция или снять свою (/ ищет эмодзи по имени)"
        }
        "Take back all your reactions" => "Снять все свои реакции",
        "Join the public group or channel you're viewing" => {
            "Вступить в публичную группу или канал, который смотрите"
        }
        "Delete" => "Удалить",
        "Pin the message (asks how), or unpin it" => {
            "Закрепить сообщение (спросит как) или открепить"
        }
        "Pinned messages: Enter goes to one, P unpins it" => {
            "Закреплённые: Enter — перейти, P — открепить"
        }
        "Go to the message a reply answers" => "К сообщению, на которое отвечает этот ответ",
        "Go to the first unread message, under the \"Unread messages\" line" => {
            "К первому непрочитанному, под чертой «Непрочитанные сообщения»"
        }
        "Go to the oldest unread message that mentions you or answers yours (@ in the chat list), and seeing it reads it; once you've seen them all, to the one before / after the cursor" => {
            "К самому старому непрочитанному с упоминанием вас или ответом вам (@ в списке чатов), просмотр читает его; когда все просмотрены — к сообщению до / после курсора"
        }
        "Forward again, as Ctrl-i (most terminals send Ctrl-i as Tab)" => {
            "Снова вперёд, как Ctrl-i (большинство терминалов шлют Ctrl-i как Tab)"
        }
        "Write a message" => "Написать сообщение",
        "Attach a file by its path (Tab completes it)" => "Прикрепить файл по пути (Tab дополняет)",
        "Paste a photo, files or text from the clipboard" => {
            "Вставить фото, файлы или текст из буфера"
        }
        "Search the whole chat; a has: search lists every match in a media browser" => {
            "Поиск по всему чату; с has: — все совпадения списком медиа"
        }
        "In a search: from:@alice, from:me, from:this or from:a name; has:photo (video, media, file, link, voice, note, gif, audio)" => {
            "В поиске: from:@alice, from:me, from:this или from:имя; has:photo (video, media, file, link, voice, note, gif, audio)"
        }
        "In a search: before:2025-10-01, after:2025-09-01 (that day counts)" => {
            "В поиске: before:2025-10-01, after:2025-09-01 (включая этот день)"
        }
        "Next older / newer match" => "Совпадение старее / новее",
        "Close the chat, leaving only the chat list" => "Закрыть чат, оставив только список чатов",
        "End the search, cancel the edit, remove the files, cancel the reply, or go back" => {
            "Закончить поиск, отменить изменение, убрать файлы, отменить ответ или назад"
        }
        "Back to the chat list, or a forum's topics (l with the list on the right)" => {
            "Назад к списку чатов или темам форума (l, если список справа)"
        }
        "Writing" => "Набор текста",
        "Send, or save the edit" => "Отправить или сохранить изменение",
        "New line" => "Новая строка",
        "Delete back to the start of the line" => "Стереть до начала строки",
        "**bold**" => "**жирный**",
        "Formatting: __italic__ ~~strike~~ ||spoiler|| `code` ```block``` [text](url)" => {
            "Разметка: __курсив__ ~~зачёркнутый~~ ||спойлер|| `код` ```блок``` [текст](url)"
        }
        "@name / :smile" => "@имя / :smile",
        "Suggests people in the group, or emoji; Tab puts one in" => {
            "Подскажет людей группы или эмодзи; Tab вставляет"
        }
        "/command" => "/команда",
        "Starting a message: the bots' commands" => "В начале сообщения: команды ботов",
        "Drop a file" => "Перетащить файл",
        "Attach it; the text you write is its caption" => {
            "Прикрепить его; написанный текст станет подписью"
        }
        "Turn files just pasted back into their path as text" => {
            "Вернуть вставленные файлы обратно в текст пути"
        }
        "Send attached photos as files, uncompressed, or back" => {
            "Фото как файлы, без сжатия, или обратно"
        }
        "Stickers" => "Стикеры",
        "Back to Normal mode; what's written stays as the chat's draft, on your other devices too" => {
            "Назад в обычный режим; написанное останется черновиком чата, и на других устройствах тоже"
        }
        "Selecting text (v)" => "Выделение текста (v)",
        "Move a character, or a row on screen" => "На символ или на строку экрана",
        "Next word / word before / end of the word" => "Следующее / предыдущее слово / конец слова",
        "Start / end of the line" => "Начало / конец строки",
        "Start / end of the text" => "Начало / конец текста",
        "Start selecting where the cursor is, or stop; it takes in both ends, as in vim" => {
            "Начать выделение с курсора или закончить; захватывает оба конца, как в vim"
        }
        "Copy what's selected and close" => "Скопировать выделенное и закрыть",
        "Stop selecting, then close" => "Закончить выделение, затем закрыть",
        "Move" => "Движение",
        "Previous / next tab: Recent, Favorites, your sets" => {
            "Пред. / след. вкладка: Недавние, Избранные, ваши наборы"
        }
        "Send the sticker (as the reply, if you're replying)" => {
            "Отправить стикер (ответом, если вы отвечаете)"
        }
        "Find stickers by emoji or word" => "Найти стикеры по эмодзи или слову",
        "Back to writing" => "Назад к набору текста",
        "Photo viewer" => "Просмотр фото",
        "The photo before / after it in the chat" => "Фото до / после в чате",
        "Zoom in / out (also + / -), up to filling the window" => {
            "Крупнее / мельче (также + / -), вплоть до всего окна"
        }
        "Open the photo in your computer's viewer" => "Открыть фото в просмотрщике компьютера",
        "Copy the photo" => "Скопировать фото",
        "Close" => "Закрыть",
        "Video player" => "Видеоплеер",
        "Play (once) or pause, right in the message (size in Settings)" => {
            "Воспроизвести (один раз) или пауза, прямо в сообщении (размер — в настройках)"
        }
        "Twice as big in the chat, or back" => "Вдвое больше в чате или обратно",
        "Zoom in / out, on the whole window" => "Крупнее / мельче, на всё окно",
        "Open the video in your computer's player" => "Открыть видео в плеере компьютера",
        "Fill the window; it closes once played" => "На всё окно; закроется после проигрывания",
        "Bot buttons" => "Кнопки ботов",
        "Move, as the buttons are laid out" => "Движение по кнопкам, как они разложены",
        "Next / previous button" => "Следующая / предыдущая кнопка",
        "Press it: the bot answers, a link opens (asking first), a reply button sends its words" => {
            "Нажать: бот ответит, ссылка откроется (сначала спросит), кнопка-ответ пошлёт свои слова"
        }
        "Secret chats" => "Секретные чаты",
        "Start one with the person in this chat (asks first); it's kept on this computer only" => {
            "Начать с человеком из этого чата (с подтверждением); хранится только на этом компьютере"
        }
        "Its key as a picture, to compare with the other person's" => {
            "Его ключ картинкой, чтобы сверить с собеседником"
        }
        "How long new messages last once they're seen" => {
            "Сколько живут новые сообщения после прочтения"
        }
        "End it for both of you, and delete it here (asks first)" => {
            "Завершить для обоих и удалить здесь (с подтверждением)"
        }
        "On a 🔥 photo: view it; it's covered again when you move on" => {
            "На фото 🔥: посмотреть; при переходе дальше снова скроется"
        }
        "Search and command prompts" => "Строки поиска и команд",
        "Search, keep the chat filter, or run the command" => {
            "Искать, оставить фильтр чата или выполнить команду"
        }
        "Complete a command or a search filter (again for the next one), or the path of a file to attach" => {
            "Дополнить команду или фильтр поиска (ещё раз — следующий), или путь файла для вложения"
        }
        "Cancel" => "Отмена",
        "Forward and find popups" => "Окна пересылки и поиска",
        "Type" => "Печатайте",
        "Search chats by name, or @username" => "Поиск чатов по имени или @username",
        "Move (also Ctrl-p / Ctrl-n, Tab)" => "Движение (также Ctrl-p / Ctrl-n, Tab)",
        "Forward there, or open it" => "Переслать туда или открыть",
        "Menus and popups" => "Меню и окна",
        "Choose" => "Выбрать",
        "Choose by number" => "Выбрать по номеру",
        "Tick a setting, or use a sound or a theme (Enter too); saved at once" => {
            "Отметить настройку или применить звук или тему (Enter тоже); сохраняется сразу"
        }
        "Switch tabs in this popup" => "Вкладки этого окна",
        "Go ahead or not, when asked about a file, a link or joining" => {
            "Согласиться или нет, когда спрашивают про файл, ссылку или вступление"
        }
        // ui/info.rs
        "Couldn't get it" => "Не удалось получить",
        "Username" => "Имя пользователя",
        "Link" => "Ссылка",
        "Phone" => "Телефон",
        "Bio" => "О себе",
        "About" => "Описание",
        "In common" => "Общие",
        " `j/k` move · `Enter` write to them · `Esc` close " => {
            " `j/k` перемещение · `Enter` написать · `Esc` закрыть "
        }
        " `Esc` close " => " `Esc` закрыть ",
        " Info " => " Информация ",
        "Subscribers" => "Подписчики",
        "Members" => "Участники",
        "Nobody to show" => "Некого показать",
        "secret chat" => "секретный чат",
        "channel" => "канал",
        "group with topics" => "группа с темами",
        "group" => "группа",
        " (me)" => " (я)",
        "owner" => "владелец",
        "admin" => "админ",
        // ui/messages.rs
        "· waiting for {name} to come online " => "· ждём, когда {name} появится в сети ",
        "last seen today at {time}" => "был(а) сегодня в {time}",
        "last seen yesterday at {time}" => "был(а) вчера в {time}",
        "last seen {date}" => "был(а) {date}",
        " · closed" => " · закрыта",
        "Unknown" => "Неизвестный",
        "· ended " => "· завершён ",
        "· loading… " => "· загрузка… ",
        "No messages yet" => "Сообщений пока нет",
        "last seen a long time ago" => "был(а) давно",
        "bot" => "бот",
        "online" => "в сети",
        "last seen recently" => "был(а) недавно",
        "last seen within a week" => "был(а) на этой неделе",
        "last seen within a month" => "был(а) в этом месяце",
        "last seen just now" => "был(а) только что",
        "last seen a minute ago" => "был(а) минуту назад",
        "Preview unavailable" => "Превью недоступно",
        "Forwarded from" => "Переслано от",
        "edited" => "изменено",
        "sending" => "отправка",
        "sending…" => "отправка…",
        "not sent" => "не отправлено",
        " Unread messages " => " Непрочитанные сообщения ",
        "Deleted message" => "Удалённое сообщение",
        "Pinned" => "Закреплено",
        "list " => "список ",
        // ui/mod.rs
        "{name} is {doing}…" => "{name} {doing}…",
        "{a} and {b} are {doing}…" => "{a} и {b} {doing}…",
        "Telegram sent a code {sent_via}." => "Telegram отправил код {sent_via}.",
        "Can't use {from}" => "Не удаётся использовать {from}",
        "{why}. Fix it, then start tuigram again. Until then it doesn't connect, rather than connect without the proxy." => {
            "{why}. Исправьте это и запустите tuigram снова. До тех пор он не подключается, чтобы не подключиться без прокси."
        }
        "In Telegram on your phone: Settings → Devices → Link Desktop Device, then point the camera at this code." => {
            "В Telegram на телефоне: Настройки → Устройства → Подключить устройство, затем наведите камеру на этот код."
        }
        "Make the terminal at least {width} columns by {height} rows to show the QR code." => {
            "Чтобы показать QR-код, терминалу нужно не меньше {width}×{height} (колонки × строки)."
        }
        "If {name} sees the same picture in this chat on their device, nobody can read it but the two of you." => {
            "Если {name} видит в этом чате на своём устройстве такую же картинку, никто, кроме вас двоих, не прочитает переписку."
        }
        "{n} more: {names}" => "ещё {n}: {names}",
        " … {n} more" => " … и ещё {n}",
        " Log in " => " Вход ",
        "Logging out…" => "Выход из аккаунта…",
        "Then you can log in again, as yourself or someone else." => {
            "Затем можно войти снова: в свой аккаунт или в другой."
        }
        "Builds from source have no API key. Get yours once at my.telegram.org → API development tools (any app name), then paste api_id.\nOr skip it all: cargo binstall tuigram-cli" => {
            "У сборок из исходников нет ключа API. Получите свой один раз на my.telegram.org → API development tools (название приложения любое), затем вставьте api_id.\nИли без всего этого: cargo binstall tuigram-cli"
        }
        "Get your own API ID and hash once at my.telegram.org → API development tools (any app name), then paste api_id." => {
            "Получите свои API ID и hash один раз на my.telegram.org → API development tools (название приложения любое), затем вставьте api_id."
        }
        "Now paste api_hash from the same page. Both stay on this computer." => {
            "Теперь вставьте api_hash с той же страницы. Оба хранятся только на этом компьютере."
        }
        "Phone number" => "Номер телефона",
        "Include the country code, e.g. +1 415 555 0123. Or press Tab to log in by scanning a QR code with Telegram on your phone." => {
            "С кодом страны, например +1 415 555 0123. Или нажмите Tab, чтобы войти, отсканировав QR-код в Telegram на телефоне."
        }
        "Login code" => "Код для входа",
        "Two-step verification password" => "Пароль двухэтапной аутентификации",
        "Your account has a cloud password." => "У вашего аккаунта есть облачный пароль.",
        "Hint:" => "Подсказка:",
        "Email address" => "Электронная почта",
        "Telegram needs an email address to send login codes to." => {
            "Telegram нужен адрес электронной почты, чтобы присылать коды для входа."
        }
        "Email code" => "Код из письма",
        "Enter the code Telegram sent to your email." => {
            "Введите код, который Telegram отправил вам на почту."
        }
        "Confirm on another device" => "Подтвердите на другом устройстве",
        "Open this link on a logged-in device:" => {
            "Откройте эту ссылку на устройстве, где вы вошли:"
        }
        "Can't log in here" => "Здесь войти не удаётся",
        "Blocked here? See TG_PROXY in tuigram --help" => {
            "Заблокировано? См. TG_PROXY в tuigram --help"
        }
        "Sending…" => "Отправка…",
        "`Enter` submit · `Tab` QR code · `Ctrl-c` quit" => {
            "`Enter` отправить · `Tab` QR-код · `Ctrl-c` выход"
        }
        "`Enter` submit · `Ctrl-c` quit" => "`Enter` отправить · `Ctrl-c` выход",
        "`Ctrl-c` quit" => "`Ctrl-c` выход",
        "`Esc` use phone number · `Ctrl-c` quit" => "`Esc` войти по номеру · `Ctrl-c` выход",
        " Scan the QR code " => " Отсканируйте QR-код ",
        "Press Enter to open a topic" => "Нажмите Enter, чтобы открыть тему",
        "Press Enter to open a chat" => "Нажмите Enter, чтобы открыть чат",
        " Delete message " => " Удалить сообщение ",
        " `Enter` delete · `Esc` cancel " => " `Enter` удалить · `Esc` отмена ",
        " Checking…" => " Проверка…",
        " `Tab` insert " => " `Tab` вставить ",
        " `Space` tick · `Enter` vote · `Esc` close " => {
            " `Space` отметить · `Enter` голосовать · `Esc` закрыть "
        }
        " `Enter` vote · `Esc` close " => " `Enter` голосовать · `Esc` закрыть ",
        " Vote " => " Голосование ",
        "  Take back my vote" => "  Отменить голос",
        " Forward to " => " Переслать в ",
        " `Enter` forward · `Esc` cancel " => " `Enter` переслать · `Esc` отмена ",
        "type to find a chat" => "введите имя чата",
        " Find a chat or person " => " Найти чат или человека ",
        " `Enter` open · `Esc` cancel " => " `Enter` открыть · `Esc` отмена ",
        "a name, @username or t.me link" => "имя, @username или ссылка t.me",
        "· searching… " => "· ищу… ",
        " Searching Telegram…" => " Ищу в Telegram…",
        " No chats yet" => " Пока нет чатов",
        " Nothing found" => " Ничего не найдено",
        "public" => "публичный",
        "contact" => "контакт",
        "`Enter` take back" => "`Enter` снять",
        "`Enter` react" => "`Enter` реакция",
        "back" => "назад",
        "search" => "поиск",
        "close" => "закрыть",
        " React " => " Реакция ",
        " removes all yours, also in the chat" => " снимает все ваши, и в чате тоже",
        " Loading…" => " Загрузка…",
        " No emoji by that name" => " Нет эмодзи с таким именем",
        " Shortcuts " => " Клавиши ",
        " Settings " => " Настройки ",
        " `j/k` scroll · `Tab` settings · `Esc` close " => {
            " `j/k` листать · `Tab` настройки · `Esc` закрыть "
        }
        " `Enter` or `Space` use · `Tab` shortcuts · `Esc` close " => {
            " `Enter` или `Space` выбрать · `Tab` клавиши · `Esc` закрыть "
        }
        " `Enter` or `Space` change · `Tab` shortcuts · `Esc` close " => {
            " `Enter` или `Space` изменить · `Tab` клавиши · `Esc` закрыть "
        }
        " Language" => " Язык",
        " Notifications" => " Уведомления",
        "New messages, while tuigram is in the background" => {
            "Новые сообщения, когда tuigram в фоне"
        }
        " Chat list" => " Список чатов",
        "A gap between chats" => "Промежуток между чатами",
        "On the right side of the window" => "Справа в окне",
        " Messages" => " Сообщения",
        "A gap between messages in a row from one person" => {
            "Промежуток между подряд идущими сообщениями одного человека"
        }
        " [small] " => " [маленький] ",
        " [medium] " => " [средний] ",
        " [large] " => " [большой] ",
        "Video messages play on Space" => "Видеосообщения воспроизводятся по Space",
        " Look" => " Вид",
        "Round corners (turn off if they look broken)" => {
            "Круглые углы (выключите, если выглядят сломанными)"
        }
        "Round pills (needs a Nerd Font)" => "Круглые «пилюли» (нужен Nerd Font)",
        "Animations (lights on focus, popups, toasts, new messages)" => {
            "Анимации (подсветка при фокусе, окна, подсказки, новые сообщения)"
        }
        " Composer" => " Поле ввода",
        "Back to Normal mode after sending a message" => {
            "Назад в обычный режим после отправки сообщения"
        }
        "Wrap long lines onto the next row" => "Переносить длинные строки на следующую",
        " Secret chats" => " Секретные чаты",
        "Take the ones others start here, not on your phone" => {
            "Принимать начатые другими здесь, а не на телефоне"
        }
        " Sound for new messages" => " Звук новых сообщений",
        " Not played over SSH: it would play on the other computer" => {
            " По SSH не воспроизводится: зазвучал бы на другом компьютере"
        }
        "Also while tuigram's window is focused" => "Также когда окно tuigram в фокусе",
        "None" => "Нет",
        " Your own sounds, WAV or Ogg Opus, go in" => " Свои звуки, WAV или Ogg Opus, кладутся в",
        " Theme" => " Тема",
        " Your own themes go in" => " Свои темы кладутся в",
        "Telegram marks it as a SCAM" => "Telegram предупреждает: мошенник (SCAM)",
        "Telegram marks it as FAKE: it poses as someone else" => {
            "Telegram предупреждает: фейк, выдаёт себя за другого (FAKE)"
        }
        "Verified by Telegram ✓" => "Подтверждено Telegram ✓",
        "cancel" => "отмена",
        " Reply buttons " => " Кнопки ответа ",
        " `Enter` send · `Esc` close " => " `Enter` отправить · `Esc` закрыть ",
        " Buttons " => " Кнопки ",
        " `Enter` press · `Esc` close " => " `Enter` нажать · `Esc` закрыть ",
        " Pin message " => " Закрепить сообщение ",
        " `Enter` pin · `Esc` cancel " => " `Enter` закрепить · `Esc` отмена ",
        " New messages disappear this long after they're seen." => {
            " Столько живут новые сообщения после прочтения."
        }
        " Self-destruct timer · " => " Таймер самоуничтожения · ",
        " `Enter` set · `Esc` cancel " => " `Enter` задать · `Esc` отмена ",
        " Make the window bigger to compare the key." => " Увеличьте окно, чтобы сверить ключ.",
        " Encryption key " => " Ключ шифрования ",
        " `Enter` close " => " `Enter` закрыть ",
        "Encryption key" => "Ключ шифрования",
        "Pinned messages" => "Закреплённые сообщения",
        " `Enter` go to · `P` unpin · `Esc` close " => {
            " `Enter` перейти · `P` открепить · `Esc` закрыть "
        }
        " `Enter` go to · `O` show file · `Esc` close " => {
            " `Enter` перейти · `O` показать файл · `Esc` закрыть "
        }
        " Searching… " => " Ищу… ",
        " Nothing found " => " Ничего не найдено ",
        " Open " => " Открыть ",
        " `Enter` open · `1-9` pick · `Esc` close " => {
            " `Enter` открыть · `1-9` выбор · `Esc` закрыть "
        }
        " Copy " => " Скопировать ",
        " `Enter` copy · `1-9` pick · `Esc` close " => {
            " `Enter` скопировать · `1-9` выбор · `Esc` закрыть "
        }
        "secret chat " => "секретный чат ",
        "Write the new text…" => "Новый текст…",
        "Add a caption…" => "Добавьте подпись…",
        "Write a reply… · `Tab` for stickers" => "Напишите ответ… · `Tab` — стикеры",
        "Write a message… · `Tab` for stickers" => "Напишите сообщение… · `Tab` — стикеры",
        "Press `i` to edit" => "Нажмите `i`, чтобы изменить",
        "Press `i` to add a caption" => "Нажмите `i`, чтобы добавить подпись",
        "Press `i` to write your reply" => "Нажмите `i`, чтобы написать ответ",
        "Press `i` to write a message" => "Нажмите `i`, чтобы написать сообщение",
        "File, uncompressed" => "Файл, без сжатия",
        "↩ Reply to " => "↩ Ответ: ",
        "✎ Edit message" => "✎ Изменение сообщения",
        " SEARCH " => " ПОИСК ",
        "  `Enter` done · `Esc` cancel " => "  `Enter` готово · `Esc` отмена ",
        "  `Tab` filters: from: has: before: after: · `Enter` search · `Esc` cancel " => {
            "  `Tab` фильтры: from: has: before: after: · `Enter` искать · `Esc` отмена "
        }
        " COMMAND " => " КОМАНДА ",
        "  `Tab` complete · `Enter` run · `Esc` cancel " => {
            "  `Tab` дополнить · `Enter` выполнить · `Esc` отмена "
        }
        " ATTACH " => " ФАЙЛ ",
        "  `Tab` complete · `Enter` attach · `Esc` cancel " => {
            "  `Tab` дополнить · `Enter` прикрепить · `Esc` отмена "
        }
        " PROXY " => " ПРОКСИ ",
        "  socks5://, http:// or t.me/proxy link · `Enter` use it, empty for none · `Esc` cancel " => {
            "  ссылка socks5://, http:// или t.me/proxy · `Enter` использовать, пусто — без прокси · `Esc` отмена "
        }
        " Commands · Tab completes " => " Команды · Tab дополняет ",
        " Matches · type more, then Tab " => " Совпадения · допишите и Tab ",
        "`Ctrl-t` send as photos" => "`Ctrl-t` отправить как фото",
        "`Ctrl-t` send as files" => "`Ctrl-t` отправить как файлы",
        "`Enter` view it" => "`Enter` посмотреть",
        "`Enter` show spoiler" => "`Enter` показать спойлер",
        "`Enter` buttons" => "`Enter` кнопки",
        "`gd` go to replied" => "`gd` к отвеченному",
        "`Ctrl-o` back" => "`Ctrl-o` назад",
        "`Ctrl-i` forward" => "`Ctrl-i` вперёд",
        " NORMAL " => " ОБЫЧНЫЙ ",
        " INSERT " => " НАБОР ",
        " STICKERS " => " СТИКЕРЫ ",
        " RESIZE " => " ШИРИНА ",
        "  `h` narrower · `l` wider · `=` as at first · `Enter` keep · `Esc` cancel" => {
            "  `h` уже · `l` шире · `=` как было · `Enter` оставить · `Esc` отмена"
        }
        "  `h` wider · `l` narrower · `=` as at first · `Enter` keep · `Esc` cancel" => {
            "  `h` шире · `l` уже · `=` как было · `Enter` оставить · `Esc` отмена"
        }
        "  `y` edit · `n` or `Esc` cancel" => "  `y` изменить · `n` или `Esc` отмена",
        "  `y` join · `n` or `Esc` cancel" => "  `y` вступить · `n` или `Esc` отмена",
        "channel|  `y` join · `n` or `Esc` cancel" => "  `y` подписаться · `n` или `Esc` отмена",
        "  `y` leave · `n` or `Esc` cancel" => "  `y` покинуть · `n` или `Esc` отмена",
        "  `y` log out · `n` or `Esc` cancel" => "  `y` выйти из аккаунта · `n` или `Esc` отмена",
        "  `y` start it · `n` or `Esc` cancel" => "  `y` начать · `n` или `Esc` отмена",
        "  `y` end it · `n` or `Esc` cancel" => "  `y` завершить · `n` или `Esc` отмена",
        "  `y` use it · `n` or `Esc` cancel" => "  `y` использовать · `n` или `Esc` отмена",
        "  `y` open · `n` or `Esc` cancel" => "  `y` открыть · `n` или `Esc` отмена",
        "  `j/k` scroll · `Tab` settings · `Esc` close" => {
            "  `j/k` листать · `Tab` настройки · `Esc` закрыть"
        }
        "  `j/k` move · `Enter` or `Space` change, saved at once · `Tab` shortcuts · `Esc` close" => {
            "  `j/k` перемещение · `Enter` или `Space` изменить, сохраняется сразу · `Tab` клавиши · `Esc` закрыть"
        }
        "  `j/k` choose · `Enter` delete · `Esc` cancel" => {
            "  `j/k` выбор · `Enter` удалить · `Esc` отмена"
        }
        "  type a name, like heart or +1 · `arrows` choose · `Enter` react · `Esc` back" => {
            "  имя, например heart или +1 · `стрелки` выбор · `Enter` реакция · `Esc` назад"
        }
        "  `h/j/k/l` choose · `Enter` react, or take yours back · `X` take all yours back · `/` search · `Esc` close" => {
            "  `h/j/k/l` выбор · `Enter` реакция или снять свою · `X` снять все свои · `/` поиск · `Esc` закрыть"
        }
        "  `j/k` choose · `Space` tick · `Enter` vote for the ticked · `Esc` close" => {
            "  `j/k` выбор · `Space` отметить · `Enter` голосовать за отмеченные · `Esc` закрыть"
        }
        "  `j/k` choose · `Enter` vote · `Esc` close" => {
            "  `j/k` выбор · `Enter` голосовать · `Esc` закрыть"
        }
        "  `Enter` or `Esc` close" => "  `Enter` или `Esc` закрыть",
        "  `j/k` choose · `Enter` pin · `Esc` cancel" => {
            "  `j/k` выбор · `Enter` закрепить · `Esc` отмена"
        }
        "  `j/k` choose · `Enter` set the timer · `Esc` cancel" => {
            "  `j/k` выбор · `Enter` задать таймер · `Esc` отмена"
        }
        "  `j/k` move · `Enter` write to them · `g/G` first/last · `Esc` close" => {
            "  `j/k` перемещение · `Enter` написать · `g/G` первый/последний · `Esc` закрыть"
        }
        "  `Esc` close" => "  `Esc` закрыть",
        " VISUAL " => " ВЫДЕЛЕНИЕ ",
        "  `y` copy · `h/j/k/l` `w/b/e` `0/$` `g/G` move the end · `v` or `Esc` stop selecting" => {
            "  `y` скопировать · `h/j/k/l` `w/b/e` `0/$` `g/G` конец выделения · `v` или `Esc` стоп"
        }
        "  `v` start selecting · `h/j/k/l` move · `w/b/e` by word · `0/$` line start/end · `g/G` start/end · `Ctrl-d/u` half page · `Esc` close" => {
            "  `v` выделять · `h/j/k/l` перемещение · `w/b/e` по словам · `0/$` начало/конец строки · `g/G` начало/конец · `Ctrl-d/u` полстраницы · `Esc` закрыть"
        }
        "  `h/l` older/newer photo · `j/k` zoom in/out · `o` open in its app · `y` copy · `Enter` or `Esc` close" => {
            "  `h/l` старее/новее фото · `j/k` крупнее/мельче · `o` открыть в приложении · `y` скопировать · `Enter` или `Esc` закрыть"
        }
        "  `Space` pause · `j/k` zoom in/out · `o` open in its app · `Enter` or `Esc` close" => {
            "  `Space` пауза · `j/k` крупнее/мельче · `o` открыть в приложении · `Enter` или `Esc` закрыть"
        }
        "  `j/k` choose · `Enter` go to it · `P` unpin it · `Esc` close" => {
            "  `j/k` выбор · `Enter` перейти · `P` открепить · `Esc` закрыть"
        }
        "  `j/k` choose · `Enter` go to it · `O` show the file · `Esc` close" => {
            "  `j/k` выбор · `Enter` перейти · `O` показать файл · `Esc` закрыть"
        }
        "  `h/j/k/l` choose · `Enter` send its words · `Esc` close" => {
            "  `h/j/k/l` выбор · `Enter` отправить её текст · `Esc` закрыть"
        }
        "  `h/j/k/l` choose · `Enter` press · `Esc` close" => {
            "  `h/j/k/l` выбор · `Enter` нажать · `Esc` закрыть"
        }
        "  type a chat's name · `arrows` choose · `Enter` forward · `Esc` cancel" => {
            "  имя чата · `стрелки` выбор · `Enter` переслать · `Esc` отмена"
        }
        "  type a name, @username or t.me link · `arrows` choose · `Enter` open · `Esc` cancel" => {
            "  имя, @username или ссылка t.me · `стрелки` выбор · `Enter` открыть · `Esc` отмена"
        }
        "  `j/k` move · `Enter` open · `Esc` clear search · `/` search again · `i` write · `q` quit" => {
            "  `j/k` перемещение · `Enter` открыть · `Esc` сбросить поиск · `/` искать снова · `i` написать · `q` выход"
        }
        "  `j/k` move · `Enter` open · `i` write · `Tab/Shift-Tab` folders · `/` search · `s` find anyone · `p` pin · `m` mute · `a` archive · `I` info · `Ctrl-o/i` back/forward · `H` highlight · `gg/G` top/bottom · `Ctrl-d/u` half page · `Ctrl-r` resize · `:` commands · `?` help · `q` quit" => {
            "  `j/k` перемещение · `Enter` открыть · `i` написать · `Tab/Shift-Tab` папки · `/` поиск · `s` найти любого · `p` закрепить · `m` без звука · `a` архив · `I` инфо · `Ctrl-o/i` назад/вперёд · `H` подсветить · `gg/G` вверх/вниз · `Ctrl-d/u` полстраницы · `Ctrl-r` ширина · `:` команды · `?` помощь · `q` выход"
        }
        "  `j/k` move · `Enter` open · `i` write · `/` search · `s` find anyone · `p` pin · `m` mute · `a` archive · `I` info · `H` highlight · `gg/G` top/bottom · `Ctrl-d/u` half page · `Ctrl-r` resize · `:` commands · `?` help · `q` quit" => {
            "  `j/k` перемещение · `Enter` открыть · `i` написать · `/` поиск · `s` найти любого · `p` закрепить · `m` без звука · `a` архив · `I` инфо · `H` подсветить · `gg/G` вверх/вниз · `Ctrl-d/u` полстраницы · `Ctrl-r` ширина · `:` команды · `?` помощь · `q` выход"
        }
        "  `j/k` move · `Enter` open · `i` write · `h` back · `s` find anyone · `I` info · `Ctrl-o/i` back/forward · `gg/G` top/bottom · `Ctrl-d/u` half page · `Ctrl-r` resize · `:` commands · `?` help · `q` quit" => {
            "  `j/k` перемещение · `Enter` открыть · `i` написать · `h` назад · `s` найти любого · `I` инфо · `Ctrl-o/i` назад/вперёд · `gg/G` вверх/вниз · `Ctrl-d/u` полстраницы · `Ctrl-r` ширина · `:` команды · `?` помощь · `q` выход"
        }
        "  `n/N` older/newer match · `Esc` end search · `/` search again · `j/k` newer/older · `Enter` open/play media · `r` reply · `i` write · `h` back" => {
            "  `n/N` старее/новее · `Esc` конец поиска · `/` искать снова · `j/k` новее/старее · `Enter` открыть/играть · `r` ответить · `i` написать · `h` назад"
        }
        "  `i` edit · `Esc` cancel edit · `e` edit selected instead · `j/k` newer/older · `h` back" => {
            "  `i` изменить · `Esc` отмена изменения · `e` изменить выбранное · `j/k` новее/старее · `h` назад"
        }
        "  `i` add caption · `Esc` remove files · `a` attach more · `p` paste more · `r` reply · `j/k` newer/older · `h` back" => {
            "  `i` подпись · `Esc` убрать файлы · `a` ещё файл · `p` вставить ещё · `r` ответить · `j/k` новее/старее · `h` назад"
        }
        "  `i` write reply · `Esc` cancel reply · `r` reply to selected instead · `j/k` newer/older · `Enter` open/play media · `h` back" => {
            "  `i` написать ответ · `Esc` отмена ответа · `r` ответить выбранному · `j/k` новее/старее · `Enter` открыть/играть · `h` назад"
        }
        "  you're not in this chat · `J` join · `j/k` newer/older · `Enter` open/play media · `s` find anyone · `h` back" => {
            "  вы не в этом чате · `J` вступить · `j/k` новее/старее · `Enter` открыть/играть · `s` найти любого · `h` назад"
        }
        "  `j/k` newer/older · `y` copy · `r` reply · `f` forward · `R` react · `X` unreact · `e` edit · `d` delete · `P` pin · `gp` pinned · `gu/gm/gM` unread/mention · `Enter` open/play media · `i` write · `a` attach · `p` paste · `/` search · `s` find anyone · `I` info · `gg/G` oldest/newest · `h` back · `Ctrl-r` resize · `:` commands · `?` help · `q` quit" => {
            "  `j/k` новее/старее · `y` скопировать · `r` ответить · `f` переслать · `R` реакция · `X` снять реакцию · `e` изменить · `d` удалить · `P` закрепить · `gp` закреплённые · `gu/gm/gM` непрочитанное/упоминание · `Enter` открыть/играть · `i` написать · `a` прикрепить · `p` вставить · `/` поиск · `s` найти любого · `I` инфо · `gg/G` старейшее/новейшее · `h` назад · `Ctrl-r` ширина · `:` команды · `?` помощь · `q` выход"
        }
        "  type an emoji or a word, like cat · `arrows` choose · `Enter` send · `Esc` back" => {
            "  эмодзи или слово, например кот · `стрелки` выбор · `Enter` отправить · `Esc` назад"
        }
        "  `h/j/k/l` choose · `H/L` previous/next tab · `Enter` send · `/` search · `Tab` or `Esc` back to writing" => {
            "  `h/j/k/l` выбор · `H/L` пред./след. вкладка · `Enter` отправить · `/` поиск · `Tab` или `Esc` обратно к тексту"
        }
        "  `Tab` insert · `arrows` choose · keep typing to narrow it down · `Enter` send · `Esc` normal mode" => {
            "  `Tab` вставить · `стрелки` выбор · продолжайте печатать для уточнения · `Enter` отправить · `Esc` обычный режим"
        }
        "  `Enter` save · `Alt-Enter` or `Ctrl-j` new line · `Esc` normal mode · `Esc Esc` cancel edit" => {
            "  `Enter` сохранить · `Alt-Enter` или `Ctrl-j` новая строка · `Esc` обычный режим · `Esc Esc` отмена изменения"
        }
        "  `Enter` send · `Ctrl-z` paste as text instead · `Alt-Enter` or `Ctrl-j` new line · `Esc` normal mode · `Esc Esc` remove files" => {
            "  `Enter` отправить · `Ctrl-z` вставить как текст · `Alt-Enter` или `Ctrl-j` новая строка · `Esc` обычный режим · `Esc Esc` убрать файлы"
        }
        "  `Enter` send · `Ctrl-v` paste more · `Alt-Enter` or `Ctrl-j` new line · `Esc` normal mode · `Esc Esc` remove files" => {
            "  `Enter` отправить · `Ctrl-v` вставить ещё · `Alt-Enter` или `Ctrl-j` новая строка · `Esc` обычный режим · `Esc Esc` убрать файлы"
        }
        "  `Enter` send reply · `Alt-Enter` or `Ctrl-j` new line · `Tab` stickers · `Esc` normal mode · `Esc Esc` cancel reply" => {
            "  `Enter` отправить ответ · `Alt-Enter` или `Ctrl-j` новая строка · `Tab` стикеры · `Esc` обычный режим · `Esc Esc` отмена ответа"
        }
        "  `Enter` send · `Alt-Enter` or `Ctrl-j` new line · `Tab` stickers · `Ctrl-v` paste photo or file · `Esc` normal mode" => {
            "  `Enter` отправить · `Alt-Enter` или `Ctrl-j` новая строка · `Tab` стикеры · `Ctrl-v` вставить фото или файл · `Esc` обычный режим"
        }
        "`gm` mention" => "`gm` упоминание",
        "`Enter` send again" => "`Enter` отправить снова",
        "chat list" => "список чатов",
        "  Closing… (q again to force)" => "  Закрытие… (q ещё раз — не ждать)",
        "  Downloading… opens when done" => "  Скачивание… откроется по готовности",
        "  Downloading… copies when done" => "  Скачивание… скопируется по готовности",
        "  Reading the clipboard…" => "  Чтение буфера обмена…",
        "Looking for" => "Поиск:",
        " (Esc stops)" => " (Esc — стоп)",
        "thing|Unknown" => "Неизвестно",
        // ui/select.rs
        " `y` copy · `Esc` cancel " => " `y` скопировать · `Esc` отмена ",
        " `v` select · `Esc` close " => " `v` выделить · `Esc` закрыть ",
        " Select text " => " Выделение текста ",
        // ui/stickers.rs
        " Stickers " => " Стикеры ",
        "No stickers yet · `/` finds some by emoji or word" => {
            "Пока нет стикеров · `/` найдёт по эмодзи или слову"
        }
        "Searching…" => "Ищу…",
        "No stickers found" => "Стикеры не найдены",
        "No stickers in this set" => "В этом наборе нет стикеров",
        // ui/topics.rs
        " closed" => " закрыта",
        " · topics loading… " => " · темы, загрузка… ",
        " · topics " => " · темы ",
        "No topics" => "Нет тем",
        // ui/viewer.rs
        "loading…" => "загрузка…",
        " · only its thumbnail" => " · только миниатюра",
        " `h/l` older/newer · `j/k` zoom in/out · `o` open in its app · `Esc` close " => {
            " `h/l` старее/новее · `j/k` крупнее/мельче · `o` открыть в приложении · `Esc` закрыть "
        }
        "Can't show this photo" => "Не удаётся показать это фото",
        // video.rs
        "decoding broke" => "декодирование прервалось",
        "the frame file can't be written" => "не удаётся записать файл кадра",
        "it can't be played" => "не удаётся воспроизвести",
        "ffmpeg isn't installed" => "ffmpeg не установлен",
        "paused" => "пауза",
        "playing" => "воспроизведение",
        // viewer.rs
        "Self-destructing media can't be copied" => "Самоуничтожающиеся медиа нельзя скопировать",
        "This chat doesn't allow copying its messages" => "В этом чате нельзя копировать сообщения",
        // voice.rs
        "there's no thread to play it on" => "не удалось запустить поток для воспроизведения",
        "it's cut short" => "файл обрывается",
        "the file can't be read" => "не удаётся прочитать файл",
        "it's too long" => "файл слишком большой",
        "the sound device stopped playing" => "звуковое устройство перестало воспроизводить звук",
        "part of it is too big" => "часть файла слишком большая",
        "it can't be read" => "не удаётся прочитать",
        "it isn't an Ogg file" => "это не файл Ogg",
        "it's empty" => "файл пуст",
        "it isn't Opus" => "это не Opus",
        "it's a newer kind of Opus file" => "это Opus новой версии",
        "it has more than two channels" => "в нём больше двух каналов",
        "part of it is broken" => "часть файла повреждена",
        _ => return None,
    })
}

/// A count's words keyed by English's many form: for 1, 21…; 2–4, 22–24…;
/// the rest.
pub(super) fn forms(en: &str) -> Option<&'static [&'static str]> {
    Some(match en {
        // app.rs
        "A channel with {n} members." => &[
            "Канал, {n} подписчик.",
            "Канал, {n} подписчика.",
            "Канал, {n} подписчиков.",
        ],
        "A group with {n} members." => &[
            "Группа, {n} участник.",
            "Группа, {n} участника.",
            "Группа, {n} участников.",
        ],
        // attach.rs
        // buttons.rs
        // chats.rs
        // chime.rs
        // clipboard.rs
        // config.rs
        // images.rs
        // info.rs
        // messages.rs
        // notify.rs
        "{n} new messages" => &[
            "{n} новое сообщение",
            "{n} новых сообщения",
            "{n} новых сообщений",
        ],
        // pins.rs
        // poll.rs
        "{n} votes" => &["{n} голос", "{n} голоса", "{n} голосов"],
        // proxy.rs
        // reactions.rs
        // search.rs
        // secret.rs
        "{n} seconds" => &["{n} секунда", "{n} секунды", "{n} секунд"],
        "{n} minutes" => &["{n} минута", "{n} минуты", "{n} минут"],
        "{n} hours" => &["{n} час", "{n} часа", "{n} часов"],
        "{n} days" => &["{n} день", "{n} дня", "{n} дней"],
        "{n} weeks" => &["{n} неделя", "{n} недели", "{n} недель"],
        // service.rs
        "added {n} members" => &[
            "добавил(а) участников: {n}",
            "добавил(а) участников: {n}",
            "добавил(а) участников: {n}",
        ],
        "{n} others" => &["ещё {n}", "ещё {n}", "ещё {n}"],
        "{n} times" => &["{n} раз", "{n} раза", "{n} раз"],
        "{n} in a game" => &["{n} очко в игре", "{n} очка в игре", "{n} очков в игре"],
        // settings.rs
        // sound.rs
        // stickers.rs
        // tg.rs
        // theme.rs
        // ui/chat_list.rs
        // ui/help.rs
        // ui/info.rs
        "{total} members" => &[
            "{total} участник",
            "{total} участника",
            "{total} участников",
        ],
        "{total} subscribers" => &[
            "{total} подписчик",
            "{total} подписчика",
            "{total} подписчиков",
        ],
        "{total} groups" => &["{total} группа", "{total} группы", "{total} групп"],
        // ui/messages.rs
        "last seen {n} minutes ago" => &[
            "был(а) {n} минуту назад",
            "был(а) {n} минуты назад",
            "был(а) {n} минут назад",
        ],
        "{n} pinned" => &["Закреплено: {n}", "Закреплено: {n}", "Закреплено: {n}"],
        // ui/mod.rs
        "{n} people are {doing}…" => &[
            "{n} человек {doing}…",
            "{n} человека {doing}…",
            "{n} человек {doing}…",
        ],
        "  topics {n} columns" => &[
            "  темы: {n} колонка",
            "  темы: {n} колонки",
            "  темы: {n} колонок",
        ],
        // ui/select.rs
        // ui/stickers.rs
        // ui/topics.rs
        // ui/viewer.rs
        // video.rs
        // viewer.rs
        // voice.rs
        _ => return None,
    })
}

/// Days, Monday first, as a date says them short.
pub(super) const WEEKDAYS: [&str; 7] = ["пн", "вт", "ср", "чт", "пт", "сб", "вс"];

/// Months as a date says them: "8 октября".
pub(super) const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];
