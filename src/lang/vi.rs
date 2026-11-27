//! Vietnamese, keyed by the English it says (see [`super`]): as Telegram's
//! Vietnamese apps word it. Grouped by the file the words are used in.

/// `en` in Vietnamese, if it has been put.
pub(super) fn text(en: &str) -> Option<&'static str> {
    Some(match en {
        // app.rs
        "Found {request}: press s to open it" => "Đã tìm thấy {request}: nhấn s để mở",
        "Nobody called {name} wrote in the messages loaded: try from:@username" => {
            "Không có tin nhắn đã tải nào của người tên {name}: hãy thử from:@username"
        }
        "{name} could be {names}: use from:@username" => {
            "{name} có thể là {names}: hãy dùng from:@username"
        }
        "You're not in {title}." => "Bạn chưa tham gia {title}.",
        "this chat" => "cuộc trò chuyện này",
        "channel|this chat" => "kênh này",
        "group|this chat" => "nhóm này",
        "{name} changed since it was attached. Drop the files (Esc in Normal mode) and attach it again" => {
            "Tệp {name} đã thay đổi sau khi đính kèm. Hãy bỏ các tệp (Esc ở chế độ thường) rồi đính kèm lại"
        }
        "No chats match \"{query}\"" => "Không có cuộc trò chuyện nào khớp với \"{query}\"",
        "{who} started a secret chat, which this computer took anyway: :leave ends it" => {
            "{who} đã bắt đầu cuộc trò chuyện bí mật, và máy tính này vẫn nhận nó: :leave để kết thúc"
        }
        "With {with}. It's end-to-end encrypted, and kept only" => {
            "Với {with}. Được mã hóa đầu cuối và chỉ được lưu"
        }
        "on this computer and on the device of theirs that" => {
            "trên máy tính này và trên thiết bị mà người kia"
        }
        "accepts it: your other devices won't have it." => {
            "dùng để chấp nhận: các thiết bị khác của bạn sẽ không có."
        }
        "The key is made once {with} accepts the secret chat" => {
            "Khóa sẽ được tạo khi {with} chấp nhận cuộc trò chuyện bí mật"
        }
        "This ends the session on Telegram and deletes what" => {
            "Phiên trên Telegram sẽ kết thúc và mọi thứ tuigram"
        }
        "tuigram keeps on this computer. Logging in again" => {
            "lưu trên máy tính này sẽ bị xóa. Đăng nhập lại"
        }
        "takes a code, and your password if you have one." => "cần mã, và mật khẩu nếu bạn có đặt.",
        "No messages match \"{query}\"" => "Không có tin nhắn nào khớp với \"{query}\"",
        "Ask to join this channel?" => "Gửi yêu cầu tham gia kênh này?",
        "Ask to join this group?" => "Gửi yêu cầu tham gia nhóm này?",
        "Join this group?" => "Tham gia nhóm này?",
        "The text says: " => "Văn bản ghi:   ",
        "Full address:  " => "Địa chỉ thật:  ",
        "{name} downloaded: press Enter on it again to open it" => {
            "Đã tải xuống {name}: nhấn Enter vào đó lần nữa để mở"
        }
        "The link still has {mask} for the hidden password or secret: write it out" => {
            "Liên kết vẫn còn {mask} thay cho mật khẩu hoặc khóa bí mật bị ẩn: hãy viết đầy đủ"
        }
        "tuigram would stop using {what}." => "tuigram sẽ ngừng dùng {what}.",
        "open" => "mở",
        "edit" => "chỉnh sửa",
        "join" => "tham gia",
        "leave" => "rời",
        "log out" => "đăng xuất",
        "start it" => "bắt đầu",
        "end it" => "kết thúc",
        "use it" => "dùng",
        "connect directly" => "kết nối trực tiếp",
        "Show what this chat is, and who's in it (also I)" => {
            "Xem thông tin cuộc trò chuyện này và ai ở trong đó (hoặc I)"
        }
        "Show a secret chat's key, to compare with the other person's" => {
            "Xem khóa của cuộc trò chuyện bí mật để so với khóa của người kia"
        }
        "Leave this group or channel, or end a secret chat (asks first)" => {
            "Rời nhóm hoặc kênh này, hoặc kết thúc cuộc trò chuyện bí mật (hỏi trước)"
        }
        "Log out of Telegram on this computer (asks first)" => {
            "Đăng xuất Telegram trên máy tính này (hỏi trước)"
        }
        "Connect through a proxy where Telegram is blocked, or directly" => {
            "Kết nối qua proxy ở nơi Telegram bị chặn, hoặc kết nối trực tiếp"
        }
        "Start a secret chat with this person (asks first)" => {
            "Bắt đầu cuộc trò chuyện bí mật với người này (hỏi trước)"
        }
        "Set how long messages in a secret chat last once seen" => {
            "Hẹn giờ tự hủy tin nhắn trong cuộc trò chuyện bí mật, tính từ lúc được xem"
        }
        "No unread mentions" => "Không có lượt nhắc đến chưa đọc",
        "Nobody mentioned you here" => "Chưa ai nhắc đến bạn ở đây",
        "No older mentions" => "Không có lượt nhắc đến cũ hơn",
        "No newer mentions" => "Không có lượt nhắc đến mới hơn",
        "Waiting for network…" => "Đang chờ mạng…",
        "Connecting to the proxy…" => "Đang kết nối tới proxy…",
        "Updating…" => "Đang cập nhật…",
        "to your Telegram app on another device" => {
            "tới ứng dụng Telegram trên thiết bị khác của bạn"
        }
        "by SMS" => "qua SMS",
        "by phone call" => "qua cuộc gọi",
        "as a missed call: enter the last digits of the calling number" => {
            "qua cuộc gọi nhỡ: nhập các chữ số cuối của số đã gọi đến"
        }
        "to fragment.com" => "tới fragment.com",
        "to one of your devices" => "tới một thiết bị của bạn",
        "Connecting…" => "Đang kết nối…",
        "Whole message" => "Toàn bộ tin nhắn",
        "It goes to:" => "Dẫn đến:",
        "Server:" => "Máy chủ:",
        "Delete for everyone" => "Xóa cho mọi người",
        "Delete for me" => "Xóa cho tôi",
        "./.env is ignored: only development builds read it" => {
            "./.env bị bỏ qua: chỉ bản dựng phát triển mới đọc nó"
        }
        "Theme not used: " => "Không áp dụng được giao diện: ",
        "Sound not used: " => "Không áp dụng được âm thanh: ",
        "Can't play this video message: " => "Không thể phát tin nhắn video này: ",
        "Close what's open to paste" => "Hãy đóng thứ đang mở để dán",
        "Download failed" => "Không thể tải xuống",
        "Shown in the file manager" => "Đã hiện trong trình quản lý tệp",
        "Voice message downloaded: Enter plays it" => "Đã tải xuống tin nhắn thoại: Enter để phát",
        "Video message downloaded: Enter plays it" => "Đã tải xuống tin nhắn video: Enter để phát",
        "Video message downloaded: Space plays it" => "Đã tải xuống tin nhắn video: Space để phát",
        "tuigram can't open this kind of link" => "tuigram không mở được loại liên kết này",
        "to" => "tới",
        "Forwarded" => "Đã chuyển tiếp",
        "Joined" => "Đã tham gia",
        "with" => "với",
        "Ended the secret chat" => "Đã kết thúc cuộc trò chuyện bí mật",
        "Left" => "Đã rời",
        "That message isn't in this topic" => "Tin nhắn đó không thuộc chủ đề này",
        "Message not sent: " => "Chưa gửi được tin nhắn: ",
        "The message was edited: v selects again" => "Tin nhắn đã được chỉnh sửa: v để chọn lại",
        "This number has no Telegram account. Sign up in the official app first." => {
            "Số này chưa có tài khoản Telegram. Hãy đăng ký trong ứng dụng chính thức trước."
        }
        "Telegram requires a Premium purchase to log in with this number. Use the official app." => {
            "Telegram yêu cầu mua Premium để đăng nhập bằng số này. Hãy dùng ứng dụng chính thức."
        }
        "The API ID is a number, like 1234567" => "API ID là một số, ví dụ 1234567",
        "The API hash is 32 letters and digits" => "API hash gồm 32 chữ cái và chữ số",
        "Couldn't save them: " => "Không thể lưu khóa API: ",
        "Telegram rejected TG_API_ID / TG_API_HASH." => {
            "Telegram đã từ chối TG_API_ID / TG_API_HASH."
        }
        "Telegram rejected tuigram's built-in key. Use your own." => {
            "Telegram đã từ chối khóa có sẵn của tuigram. Hãy dùng khóa của bạn."
        }
        "Telegram rejected that API ID and hash." => "Telegram đã từ chối API ID và hash đó.",
        "Telegram rejected the API key; can't forget it: " => {
            "Telegram đã từ chối khóa API, nhưng không xóa được khóa đã lưu: "
        }
        "Your account isn't known yet" => "Chưa xác định được tài khoản của bạn",
        "Join this channel?" => "Tham gia kênh này?",
        "Join it to have it in your chat list." => {
            "Tham gia để có kênh trong danh sách trò chuyện."
        }
        "Join to write here?" => "Tham gia để viết ở đây?",
        "Join it to write, and to have it in your chat list." => {
            "Tham gia để viết, và để có nhóm trong danh sách trò chuyện."
        }
        "them" => "người kia",
        "from: doesn't work in secret chats" => {
            "from: không dùng được trong cuộc trò chuyện bí mật"
        }
        "Not a command: " => "Không có lệnh: ",
        "Someone" => "Ai đó",
        "Secret chat" => "Cuộc trò chuyện bí mật",
        "Someone started a secret chat with you" => {
            "Ai đó đã bắt đầu cuộc trò chuyện bí mật với bạn"
        }
        "New message" => "Tin nhắn mới",
        "Open the group or channel to leave first" => "Hãy mở nhóm hoặc kênh muốn rời trước",
        "End this secret chat?" => "Kết thúc cuộc trò chuyện bí mật này?",
        "With" => "Với",
        "Nothing more can be sent in it, by either of you," => {
            "Cả hai bạn sẽ không gửi được gì vào đó nữa,"
        }
        "and its messages are deleted from this computer." => {
            "và các tin nhắn trong đó sẽ bị xóa khỏi máy tính này."
        }
        "A chat with one person can't be left" => "Không thể rời cuộc trò chuyện với một người",
        "You're not in this chat" => "Bạn không ở trong cuộc trò chuyện này",
        "Leave this channel?" => "Rời kênh này?",
        "Its posts stop coming to you." => "Bạn sẽ không nhận được bài đăng của kênh nữa.",
        "Leave this group?" => "Rời nhóm này?",
        "Its messages stop coming to you." => "Bạn sẽ không nhận được tin nhắn của nhóm nữa.",
        "Open a chat with someone to start a secret chat with them" => {
            "Hãy mở cuộc trò chuyện với một người để bắt đầu cuộc trò chuyện bí mật với họ"
        }
        "Bots can't be in secret chats" => "Bot không thể tham gia cuộc trò chuyện bí mật",
        "Start a secret chat?" => "Bắt đầu cuộc trò chuyện bí mật?",
        "Open a secret chat first (:secret starts one)" => {
            "Hãy mở một cuộc trò chuyện bí mật trước (:secret để bắt đầu)"
        }
        "Telegram gave no key to compare for this chat" => {
            "Telegram không cung cấp khóa để so sánh cho cuộc trò chuyện này"
        }
        "Your secret chats go too: they're kept only here." => {
            "Các cuộc trò chuyện bí mật cũng mất: chúng chỉ được lưu ở đây."
        }
        "Log out?" => "Đăng xuất?",
        "Couldn't save settings: " => "Không thể lưu cài đặt: ",
        "No newer matches" => "Không còn kết quả mới hơn",
        "No older matches" => "Không còn kết quả cũ hơn",
        "Nothing to go back to" => "Không có gì để quay lại",
        "Nothing to go forward to" => "Không có gì để tiến tới",
        "You can't pin messages here" => "Bạn không thể ghim tin nhắn ở đây",
        "No pinned messages in this chat" => "Không có tin nhắn đã ghim trong cuộc trò chuyện này",
        "Downloading to show in the file manager…" => {
            "Đang tải xuống để hiện trong trình quản lý tệp…"
        }
        "No file in this message" => "Tin nhắn này không có tệp",
        "tuigram can't show this only while it's open" => {
            "tuigram không hiển thị được nội dung chỉ hiện khi mở"
        }
        "Nothing to open in this message" => "Tin nhắn này không có gì để mở",
        "Telegram won't take this message again: `d` deletes it" => {
            "Telegram sẽ không nhận lại tin nhắn này: `d` để xóa"
        }
        "Can't reply to a message that wasn't sent" => "Không thể trả lời tin nhắn chưa gửi được",
        "The edit came while something else was open: e edits again" => {
            "Chưa chỉnh sửa được vì đang mở thứ khác: e để chỉnh sửa lại"
        }
        "You can't edit this message" => "Bạn không thể chỉnh sửa tin nhắn này",
        "Edit and lose some formatting?" => "Chỉnh sửa và mất một phần định dạng?",
        "Some of its formatting can't be written in Markdown" => {
            "Một phần định dạng của nó không viết được bằng Markdown"
        }
        "(underline, custom emoji…), so an edit would lose it." => {
            "(gạch chân, emoji tùy chỉnh…), nên chỉnh sửa sẽ làm mất."
        }
        "A message can't be empty (d deletes it)" => "Tin nhắn không thể để trống (d để xóa)",
        "You can't delete this message" => "Bạn không thể xóa tin nhắn này",
        "Reactions are off in this chat" => "Cảm xúc đã bị tắt trong cuộc trò chuyện này",
        "Pressed" => "Đã nhấn",
        "That message isn't loaded any more" => "Tin nhắn đó không còn được tải",
        "The bot's link isn't a web address" => "Liên kết của bot không phải địa chỉ web",
        "Messages in secret chats can't be forwarded" => {
            "Không thể chuyển tiếp tin nhắn trong cuộc trò chuyện bí mật"
        }
        "your contact" => "liên hệ của bạn",
        "An admin has to let you in." => "Quản trị viên phải duyệt cho bạn vào.",
        "You haven't reacted to this message" => "Bạn chưa thả cảm xúc cho tin nhắn này",
        "An edit can't change a message into a sticker" => {
            "Chỉnh sửa không thể biến tin nhắn thành nhãn dán"
        }
        "No unread messages" => "Không có tin nhắn chưa đọc",
        "No chat selected" => "Chưa chọn cuộc trò chuyện nào",
        "This photo can't be shown here" => "Không thể hiển thị ảnh này ở đây",
        "This photo can't be saved, so it opens only here" => {
            "Không thể lưu ảnh này, nên nó chỉ mở được ở đây"
        }
        "Loading older messages…" => "Đang tải tin nhắn cũ hơn…",
        "Loading newer messages…" => "Đang tải tin nhắn mới hơn…",
        "No older photos in this chat" => "Không còn ảnh cũ hơn trong cuộc trò chuyện này",
        "No newer photos in this chat" => "Không còn ảnh mới hơn trong cuộc trò chuyện này",
        "Video messages play in the terminal only with kitty's protocol" => {
            "Tin nhắn video chỉ phát được trong terminal khi có giao thức của kitty"
        }
        "Video messages play in the terminal only on this computer" => {
            "Tin nhắn video chỉ phát được trong terminal trên chính máy tính này"
        }
        "This video message can't be played here" => "Không thể phát tin nhắn video này ở đây",
        "Can't play this video message: it's too big" => {
            "Không thể phát tin nhắn video này: tệp quá lớn"
        }
        "This terminal can't play video messages in the chat; Enter plays one on the whole window" => {
            "Terminal này không phát được tin nhắn video trong cuộc trò chuyện; Enter để phát trên toàn cửa sổ"
        }
        "Downloading the video message…" => "Đang tải xuống tin nhắn video…",
        "This video message can't be saved, so it plays only here" => {
            "Không thể lưu tin nhắn video này, nên nó chỉ phát được ở đây"
        }
        "Can't play this voice message: it's too long" => {
            "Không thể phát tin nhắn thoại này: quá dài"
        }
        "(has non-Latin letters)" => "(có chữ cái không phải Latinh)",
        "Open this link?" => "Mở liên kết này?",
        "Open" => "Mở",
        "Files like this can run programs on your computer." => {
            "Những tệp như thế này có thể chạy chương trình trên máy tính của bạn."
        }
        "Only open it if you trust whoever sent it." => "Chỉ mở nếu bạn tin người đã gửi nó.",
        "Couldn't open it: " => "Không thể mở: ",
        "a secret chat with" => "cuộc trò chuyện bí mật với",
        "Nothing to copy in this message" => "Tin nhắn này không có gì để sao chép",
        "No text to select in this message" => "Tin nhắn này không có văn bản để chọn",
        "Nothing selected: v starts a selection" => "Chưa chọn gì: v để bắt đầu chọn",
        "Over SSH, only text reaches your computer's clipboard" => {
            "Qua SSH, chỉ văn bản mới tới được bộ nhớ tạm của máy tính bạn"
        }
        "Copied" => "Đã sao chép",
        "Sent to the terminal's clipboard" => "Đã gửi vào bộ nhớ tạm của terminal",
        "Couldn't copy: " => "Không thể sao chép: ",
        "Can't play the sound for new messages: " => "Không thể phát âm thanh tin nhắn mới: ",
        "Can't play this voice message: " => "Không thể phát tin nhắn thoại này: ",
        "Files can't be added to an edit" => "Không thể thêm tệp khi chỉnh sửa",
        "Over SSH, paste with your terminal's own paste key: the clipboard here is the other computer's" => {
            "Qua SSH, hãy dán bằng phím dán của chính terminal: bộ nhớ tạm ở đây là của máy tính kia"
        }
        "The paste came after you left the messages: p pastes again" => {
            "Nội dung dán đến sau khi bạn rời khỏi khung tin nhắn: p để dán lại"
        }
        "The paste came while something else was open: p pastes again" => {
            "Nội dung dán đến khi đang mở thứ khác: p để dán lại"
        }
        "Copied files without a full path were left out" => {
            "Đã bỏ qua các tệp sao chép không có đường dẫn đầy đủ"
        }
        "Pasted image" => "Ảnh đã dán",
        "These go as files already: only photos can go either way" => {
            "Những tệp này vốn đã gửi dạng tệp: chỉ ảnh mới gửi được cả hai cách"
        }
        "Over SSH, tuigram plays no sound: it would play on the other computer" => {
            "Qua SSH, tuigram không phát âm thanh: âm thanh sẽ phát trên máy tính kia"
        }
        "Settings not saved: " => "Chưa lưu được cài đặt: ",
        "No folders yet: Telegram's apps can make them" => {
            "Chưa có thư mục: hãy tạo trong ứng dụng Telegram"
        }
        "the proxy in settings.toml" => "proxy trong settings.toml",
        "TG_PROXY sets the proxy: change it there" => "Proxy do TG_PROXY đặt: hãy đổi ở đó",
        "No proxy: tuigram connects to Telegram directly" => {
            "Không có proxy: tuigram kết nối trực tiếp tới Telegram"
        }
        "the proxy" => "proxy",
        "Connect directly?" => "Kết nối trực tiếp?",
        "Telegram would see your IP address, and your network that you use Telegram." => {
            "Telegram sẽ thấy địa chỉ IP của bạn, còn mạng của bạn sẽ thấy bạn dùng Telegram."
        }
        "Proxy:         " => "Proxy:         ",
        "tuigram would connect to Telegram through it. It sees your IP address and when you use Telegram, not your messages." => {
            "tuigram sẽ kết nối tới Telegram qua proxy này. Proxy thấy địa chỉ IP của bạn và lúc bạn dùng Telegram, nhưng không thấy tin nhắn."
        }
        "It can also put a channel it promotes in your chat list, marked \"proxy sponsor\"." => {
            "Proxy cũng có thể đưa một kênh nó quảng bá vào danh sách trò chuyện, có ghi \"nhà tài trợ proxy\"."
        }
        "`:proxy` changes it, or goes back to connecting directly." => {
            "`:proxy` để đổi, hoặc quay lại kết nối trực tiếp."
        }
        "Use this proxy?" => "Dùng proxy này?",
        "Switching to" => "Đang chuyển sang",
        "Switching to connecting directly…" => "Đang chuyển sang kết nối trực tiếp…",
        "Proxy not saved for next time: " => "Chưa lưu proxy cho lần sau: ",
        "Connecting through" => "Đang kết nối qua",
        "No proxy" => "Không dùng proxy",
        "Connecting to Telegram directly" => "Đang kết nối trực tiếp tới Telegram",
        "The proxy wasn't changed: " => "Không đổi được proxy: ",
        "Moved out of the archive" => "Đã bỏ lưu trữ",
        "Archived" => "Đã lưu trữ",
        "Highlight not saved: " => "Chưa lưu được highlight: ",
        // attach.rs
        "{name} is a folder" => "{name} là một thư mục",
        "{name} is empty" => "{name} là tệp trống",
        "Can't send {name}: its path isn't valid UTF-8" => {
            "Không thể gửi {name}: đường dẫn của tệp không phải UTF-8 hợp lệ"
        }
        "KB" => "KB",
        "MB" => "MB",
        "GB" => "GB",
        "TB" => "TB",
        "No such file" => "Không tìm thấy tệp",
        "Can't read" => "Không thể đọc",
        "B" => "B",
        // buttons.rs
        "Presses \"{label}\": the bot decides what happens" => {
            "Nhấn \"{label}\": bot quyết định điều gì xảy ra"
        }
        "Opens {host} in your browser, asking first" => "Mở {host} trong trình duyệt, có hỏi trước",
        "Opens {url} in tuigram" => "Mở {url} trong tuigram",
        "Opens your chat with {name}" => "Mở cuộc trò chuyện với {name}",
        "Copies" => "Sao chép",
        "Sends as you" => "Gửi dưới tên bạn",
        "Opens" => "Mở",
        "Only Telegram's own apps can do this" => {
            "Chỉ ứng dụng Telegram chính thức mới làm được việc này"
        }
        "Web apps only open in Telegram's own apps" => {
            "Ứng dụng web chỉ mở được trong ứng dụng Telegram chính thức"
        }
        "Games only run in Telegram's own apps" => {
            "Trò chơi chỉ chạy được trong ứng dụng Telegram chính thức"
        }
        "Payments only work in Telegram's own apps" => {
            "Thanh toán chỉ dùng được trong ứng dụng Telegram chính thức"
        }
        "This button asks for your password: use Telegram's own apps" => {
            "Nút này yêu cầu mật khẩu của bạn: hãy dùng ứng dụng Telegram chính thức"
        }
        "This button's link isn't a web address" => "Liên kết của nút này không phải địa chỉ web",
        "Open link" => "Mở liên kết",
        // chats.rs
        "typing" => "đang soạn tin",
        "recording a video" => "đang quay video",
        "sending a video" => "đang gửi video",
        "recording a voice message" => "đang ghi âm",
        "sending a voice message" => "đang gửi tin nhắn thoại",
        "sending a photo" => "đang gửi ảnh",
        "sending a file" => "đang gửi tệp",
        "choosing a sticker" => "đang chọn nhãn dán",
        "choosing a location" => "đang chọn vị trí",
        "choosing a contact" => "đang chọn liên hệ",
        "playing a game" => "đang chơi trò chơi",
        "recording a video message" => "đang quay tin nhắn video",
        "sending a video message" => "đang gửi tin nhắn video",
        "All" => "Tất cả",
        "Archive" => "Lưu trữ",
        " SCAM" => " LỪA ĐẢO",
        " FAKE" => " GIẢ MẠO",
        "Saved Messages" => "Tin nhắn đã lưu",
        "You" => "Bạn",
        "Photo" => "Ảnh",
        "Video" => "Video",
        "File" => "Tệp",
        "Audio" => "Âm thanh",
        "Voice message" => "Tin nhắn thoại",
        "Video message" => "Tin nhắn video",
        "Sticker" => "Nhãn dán",
        "Poll" => "Bình chọn",
        "Location" => "Vị trí",
        "Contact" => "Liên hệ",
        "Photo expired" => "Ảnh đã hết hạn",
        "Video expired" => "Video đã hết hạn",
        "Video message expired" => "Tin nhắn video đã hết hạn",
        "Voice message expired" => "Tin nhắn thoại đã hết hạn",
        "Message" => "Tin nhắn",
        // chime.rs
        "there is no sound called {name}" => "không có âm thanh nào tên {name}",
        "{bits}-bit samples can't be played" => "không phát được mẫu {bits}-bit",
        "Ding" => "Ting",
        "Chime" => "Chuông",
        "Trill" => "Líu lo",
        "Pop" => "Bụp",
        "Knock" => "Gõ cửa",
        "it's neither WAV nor Ogg" => "tệp không phải WAV cũng không phải Ogg",
        "it isn't a WAV file" => "đây không phải tệp WAV",
        "its sound comes before its format" => "phần âm thanh nằm trước phần định dạng",
        "it has no sound" => "tệp không có âm thanh",
        "its format is cut short" => "phần định dạng bị cắt cụt",
        "it's compressed, which can't be played: save it as PCM" => {
            "tệp bị nén nên không phát được: hãy lưu dưới dạng PCM"
        }
        "its format is broken" => "định dạng bị hỏng",
        "it couldn't be decoded" => "không giải mã được tệp",
        // clipboard.rs
        "Can't reach the clipboard" => "Không thể truy cập bộ nhớ tạm",
        "The image on the clipboard is broken" => "Ảnh trong bộ nhớ tạm bị hỏng",
        "Can't save the image" => "Không thể lưu ảnh",
        "Nothing to paste: the clipboard is empty" => "Không có gì để dán: bộ nhớ tạm trống",
        // config.rs
        "not a file" => "không phải tệp",
        "bigger than" => "lớn hơn",
        // images.rs
        "the image couldn't be decoded" => "không giải mã được ảnh",
        // info.rs
        "Couldn't get its members" => "Không thể tải danh sách thành viên",
        "Its admins hid who's in it" => "Quản trị viên đã ẩn danh sách thành viên",
        "Only its admins can see who's in it" => {
            "Chỉ quản trị viên mới xem được danh sách thành viên"
        }
        // messages.rs
        "watch it in Telegram on your phone" => "xem trong Telegram trên điện thoại",
        "[Photo · Enter to view]" => "[Ảnh · Enter để xem]",
        "This message has no text to edit" => "Tin nhắn này không có văn bản để chỉnh sửa",
        "No message selected" => "Chưa chọn tin nhắn nào",
        "Not a reply" => "Đây không phải tin nhắn trả lời",
        "It answers a message in another chat" => {
            "Tin nhắn này trả lời một tin nhắn ở cuộc trò chuyện khác"
        }
        "The message it answers was deleted" => "Tin nhắn được trả lời đã bị xóa",
        "This message wasn't sent" => "Tin nhắn này chưa gửi được",
        "Wait until it's sent" => "Hãy đợi tin nhắn được gửi xong",
        // notify.rs
        // pins.rs
        "Pin for me and {name}" => "Ghim cho tôi và {name}",
        "Pin" => "Ghim",
        "Pin just for me" => "Chỉ ghim cho tôi",
        "Pin and notify subscribers" => "Ghim và thông báo cho người đăng ký",
        "Pin and notify members" => "Ghim và thông báo cho thành viên",
        "Pin without notifying" => "Ghim không thông báo",
        // poll.rs
        "This poll is closed" => "Cuộc bình chọn này đã kết thúc",
        "You answered this quiz" => "Bạn đã trả lời câu đố này",
        "Anonymous quiz" => "Câu đố ẩn danh",
        "Anonymous poll" => "Bình chọn ẩn danh",
        "Quiz" => "Câu đố",
        "pick several" => "chọn nhiều",
        "closed" => "đã kết thúc",
        "quiz|This poll is closed" => "Câu đố này đã kết thúc",
        // proxy.rs
        "{kind} proxy {address}" => "proxy {kind} {address}",
        "The link has no secret" => "Liên kết không có khóa bí mật",
        "Not a proxy link: give socks5://host:port, http://host:port or a t.me/proxy link" => {
            "Không phải liên kết proxy: hãy nhập socks5://host:port, http://host:port hoặc một liên kết t.me/proxy"
        }
        "A [ in the address isn't closed" => "Dấu [ trong địa chỉ chưa được đóng",
        "The link has no server" => "Liên kết không có máy chủ",
        "The server isn't a host name or an address" => {
            "Máy chủ không phải tên miền hay địa chỉ IP"
        }
        "The link has no port, as in host:1080" => "Liên kết thiếu cổng, ví dụ host:1080",
        "The port must be a number from 1 to 65535" => "Cổng phải là một số từ 1 đến 65535",
        "The link's user name, password or secret can't be used" => {
            "Không dùng được tên người dùng, mật khẩu hoặc khóa bí mật trong liên kết"
        }
        "The proxy's secret isn't hex or base64" => {
            "Khóa bí mật của proxy không phải hex hay base64"
        }
        "The proxy's secret isn't one Telegram's proxies use" => {
            "Proxy của Telegram không dùng loại khóa bí mật này"
        }
        "A % in the link isn't followed by two hex digits" => {
            "Sau dấu % trong liên kết phải có hai chữ số hex"
        }
        "The link isn't valid text" => "Liên kết không phải văn bản hợp lệ",
        // reactions.rs
        "You post anonymously here, so you can't react" => {
            "Bạn đang đăng ẩn danh ở đây nên không thể thả cảm xúc"
        }
        "Join the chat to react" => "Hãy tham gia cuộc trò chuyện để thả cảm xúc",
        // search.rs
        "Only one {key} at a time" => "Chỉ được dùng {key} một lần",
        "Dates are like 2025-10-01, not \"{value}\"" => {
            "Ngày phải viết như 2025-10-01, không phải \"{value}\""
        }
        "has: takes photo, video, media, file, link, voice, note, gif or audio" => {
            "has: nhận photo, video, media, file, link, voice, note, gif hoặc audio"
        }
        "Add words, from: or has: to search for" => "Hãy thêm từ khóa, from: hoặc has: để tìm kiếm",
        "after: has to be a day before before:" => "after: phải là ngày trước before:",
        "from: takes me, @username or a name, like from:@alice" => {
            "from: nhận me, @username hoặc một tên, ví dụ from:@alice"
        }
        "of" => "/",
        "searching…" => "đang tìm…",
        // secret.rs
        "Off" => "Tắt",
        "Waiting for {name} to come online" => "Đang chờ {name} trực tuyến",
        "{n}s" => "{n} giây",
        "{n}m" => "{n} phút",
        "{n}h" => "{n} giờ",
        "{n}d" => "{n} ngày",
        "{n}w" => "{n} tuần",
        "This secret chat has ended" => "Cuộc trò chuyện bí mật này đã kết thúc",
        "off" => "tắt",
        "🔥 once" => "🔥 một lần",
        // service.rs
        "joined via an invite link" => "đã tham gia qua liên kết mời",
        "joined, approved by an admin" => "đã tham gia, được quản trị viên phê duyệt",
        "created the group" => "đã tạo nhóm",
        "created" => "đã tạo",
        "changed the name to" => "đã đổi tên thành",
        "changed the photo" => "đã đổi ảnh",
        "removed the photo" => "đã xóa ảnh",
        "upgraded the group to a supergroup" => "đã nâng cấp nhóm thành siêu nhóm",
        "pinned a message" => "đã ghim một tin nhắn",
        "took a screenshot" => "đã chụp màn hình",
        "turned the timer off" => "đã tắt hẹn giờ tự hủy",
        "set messages to disappear after {time}" => "đã đặt tin nhắn tự hủy sau {time}",
        "scheduled a video chat" => "đã lên lịch trò chuyện video",
        "started a video chat" => "đã bắt đầu trò chuyện video",
        "ended the video chat ({length})" => "đã kết thúc trò chuyện video ({length})",
        "invited people to the video chat" => "đã mời mọi người vào trò chuyện video",
        "changed the chat theme" => "đã đổi giao diện trò chuyện",
        "changed the chat background" => "đã đổi hình nền trò chuyện",
        "boosted the group {times}" => "đã tăng cường nhóm {times}",
        "boosted the group" => "đã tăng cường nhóm",
        "created the topic" => "đã tạo chủ đề",
        "edited a topic" => "đã chỉnh sửa một chủ đề",
        "closed a topic" => "đã đóng một chủ đề",
        "reopened a topic" => "đã mở lại một chủ đề",
        "hid the General topic" => "đã ẩn chủ đề General",
        "showed the General topic" => "đã hiện chủ đề General",
        "scored {points}" => "đã ghi {points}",
        "joined Telegram" => "đã tham gia Telegram",
        "\"{title}\"" => "\"{title}\"",
        " joined the group" => " đã tham gia nhóm",
        " added " => " đã thêm ",
        " left the group" => " đã rời nhóm",
        " removed " => " đã xóa ",
        "joined the group" => "đã tham gia nhóm",
        "added a member" => "đã thêm một thành viên",
        "left the group" => "đã rời nhóm",
        "removed a member" => "đã xóa một thành viên",
        "nobody" => "không ai",
        " and " => " và ",
        // settings.rs
        "cannot write" => "không thể ghi",
        // sound.rs
        "the sound device takes {format} samples, which tuigram can't make" => {
            "thiết bị âm thanh nhận mẫu {format}, loại mà tuigram không tạo được"
        }
        "the sound device can't be used" => "không dùng được thiết bị âm thanh",
        "the sound device stopped" => "thiết bị âm thanh đã dừng",
        "the sound device didn't start" => "thiết bị âm thanh không khởi động",
        "no speakers or headphones found" => "không tìm thấy loa hay tai nghe",
        "there's no sound here (ALSA's libasound.so.2 isn't installed)" => {
            "không có âm thanh ở đây (chưa cài libasound.so.2 của ALSA)"
        }
        "the sound device can't be opened" => "không mở được thiết bị âm thanh",
        "error" => "lỗi",
        "the sound device didn't start again" => "thiết bị âm thanh không khởi động lại được",
        "tuigram can't play sound on this system yet" => {
            "tuigram chưa phát được âm thanh trên hệ thống này"
        }
        // stickers.rs
        "Recent" => "Gần đây",
        "Favorites" => "Yêu thích",
        // tg.rs
        "That message can't be found" => "Không tìm thấy tin nhắn đó",
        "Couldn't take the secret chats others start" => {
            "Không thể nhận các cuộc trò chuyện bí mật do người khác bắt đầu"
        }
        "Couldn't leave to your phone the secret chats others start" => {
            "Không thể để điện thoại nhận các cuộc trò chuyện bí mật do người khác bắt đầu"
        }
        "Nobody on Telegram is called" => "Không có ai trên Telegram tên là",
        // theme.rs
        "{id}.toml: there is no theme called {parent} to inherit" => {
            "{id}.toml: không có giao diện nào tên {parent} để kế thừa"
        }
        "{id}.toml: inherits goes round in a circle" => "{id}.toml: inherits tạo thành vòng lặp",
        "there is no theme called {name}" => "không có giao diện nào tên {name}",
        "the palette has no color called {name}" => "bảng màu không có màu nào tên {name}",
        "palette.{name} is {value}, not a color like \"#1e1e2e\"" => {
            "palette.{name} là {value}, không phải màu dạng \"#1e1e2e\""
        }
        "colors.{name} is {value}, not a palette color, \"#rrggbb\" or \"reset\"" => {
            "colors.{name} là {value}, không phải màu trong bảng màu, \"#rrggbb\" hay \"reset\""
        }
        "there is no color called {name} to set" => "không có màu nào tên {name} để đặt",
        "it isn't UTF-8 text" => "tệp không phải văn bản UTF-8",
        "line" => "dòng",
        "the palette needs" => "bảng màu còn thiếu",
        "colors.names needs seven colors" => "colors.names cần bảy màu",
        // ui/chat_list.rs
        "Chats ({n}) " => "Trò chuyện ({n}) ",
        "Chats ({n} of {total}) /{filter} " => "Trò chuyện ({n} trên {total}) /{filter} ",
        " proxy sponsor" => " nhà tài trợ proxy",
        "loading… " => "đang tải… ",
        "No chats match" => "Không có cuộc trò chuyện nào khớp",
        "No chats here" => "Không có cuộc trò chuyện nào ở đây",
        // ui/help.rs
        "Everywhere" => "Mọi nơi",
        "This help, and settings" => "Trợ giúp này và cài đặt",
        "Type a command: info, secret, key, timer, leave, proxy or logout" => {
            "Gõ lệnh: info, secret, key, timer, leave, proxy hoặc logout"
        }
        "Find a chat or person: a name, @username or t.me link" => {
            "Tìm cuộc trò chuyện hoặc người: tên, @username hoặc liên kết t.me"
        }
        "What the chat is, and who's in it: Enter on someone writes to them" => {
            "Thông tin cuộc trò chuyện và ai ở trong đó: Enter trên một người để nhắn cho họ"
        }
        "Highlight or unhighlight the selected chat" => {
            "Highlight hoặc bỏ highlight cuộc trò chuyện đang chọn"
        }
        "Back to the chat or reply you left / forward again" => {
            "Quay lại cuộc trò chuyện hoặc tin trả lời vừa rời / tiến tới lại"
        }
        "Resize the panes: h / l, = as at first, saved at once; Esc or Enter is done" => {
            "Đổi cỡ các khung: h / l, = như ban đầu, lưu ngay; Esc hoặc Enter để xong"
        }
        "Quit (press again to stop waiting)" => "Thoát (nhấn lần nữa để khỏi chờ)",
        "Quit, except while writing" => "Thoát, trừ khi đang soạn tin",
        "Keys work in the Cyrillic layout too: н is y, р is h; typing takes letters as they are" => {
            "Phím cũng dùng được với bàn phím chữ Kirin: н là y, р là h; khi soạn tin, chữ được giữ nguyên"
        }
        "Chat list" => "Danh sách trò chuyện",
        "Move down / up" => "Xuống / lên",
        "First / last chat" => "Cuộc trò chuyện đầu / cuối",
        "Half a page down / up" => "Xuống / lên nửa trang",
        "Open the chat, at its first unread message (Enter / h with the list on the right)" => {
            "Mở cuộc trò chuyện tại tin nhắn chưa đọc đầu tiên (Enter / h nếu danh sách ở bên phải)"
        }
        "Open the chat and write" => "Mở cuộc trò chuyện và soạn tin",
        "Open the chat at a message that mentions you" => {
            "Mở cuộc trò chuyện tại tin nhắn nhắc đến bạn"
        }
        "Next / previous folder, then the archive" => "Thư mục sau / trước, rồi đến lưu trữ",
        "Filter chats by name" => "Lọc cuộc trò chuyện theo tên",
        "Pin or unpin the chat (on all your devices)" => {
            "Ghim hoặc bỏ ghim cuộc trò chuyện (trên mọi thiết bị của bạn)"
        }
        "Mute or unmute the chat" => "Tắt hoặc bật thông báo cho cuộc trò chuyện",
        "Archive the chat, or move it out of the archive" => {
            "Lưu trữ cuộc trò chuyện, hoặc đưa ra khỏi lưu trữ"
        }
        "Clear the filter" => "Xóa bộ lọc",
        "Forum topics" => "Chủ đề diễn đàn",
        "First / last topic" => "Chủ đề đầu / cuối",
        "Open the topic's messages (Enter / h with the list on the right)" => {
            "Mở tin nhắn của chủ đề (Enter / h nếu danh sách ở bên phải)"
        }
        "Open the topic and write" => "Mở chủ đề và soạn tin",
        "Resize the topics pane: h / l, = as at first" => {
            "Đổi cỡ khung chủ đề: h / l, = như ban đầu"
        }
        "Back to the chat list (l with the list on the right)" => {
            "Quay lại danh sách trò chuyện (l nếu danh sách ở bên phải)"
        }
        "Messages" => "Tin nhắn",
        "Newer / older message" => "Tin nhắn mới hơn / cũ hơn",
        "Oldest / newest message" => "Tin nhắn cũ nhất / mới nhất",
        "Half a page newer / older" => "Nửa trang mới hơn / cũ hơn",
        "Send a message that wasn't sent again, show spoilers, view a photo shown only while open, play or pause a voice message, vote in a poll, list a bot's buttons, show the photo, or open the file or link" => {
            "Gửi lại tin nhắn chưa gửi được, hiện nội dung ẩn, xem ảnh chỉ hiện khi mở, phát hoặc tạm dừng tin nhắn thoại, bỏ phiếu trong bình chọn, liệt kê các nút của bot, hiện ảnh, hoặc mở tệp hay liên kết"
        }
        "Copy the text, a link, or the photo or file" => "Sao chép văn bản, liên kết, ảnh hoặc tệp",
        "Show the file in the file manager" => "Hiện tệp trong trình quản lý tệp",
        "Show the text to select part of it and copy that" => {
            "Hiện văn bản để chọn một phần và sao chép"
        }
        "Reply" => "Trả lời",
        "Forward to another chat (an album goes whole)" => {
            "Chuyển tiếp đến cuộc trò chuyện khác (album được chuyển cả bộ)"
        }
        "Edit your message" => "Chỉnh sửa tin nhắn của bạn",
        "React, or take your reaction back (/ finds an emoji by name)" => {
            "Thả cảm xúc, hoặc gỡ cảm xúc của bạn (/ tìm emoji theo tên tiếng Anh)"
        }
        "Take back all your reactions" => "Gỡ hết cảm xúc của bạn",
        "Join the public group or channel you're viewing" => {
            "Tham gia nhóm hoặc kênh công khai đang xem"
        }
        "Delete" => "Xóa",
        "Pin the message (asks how), or unpin it" => {
            "Ghim tin nhắn (sẽ hỏi cách ghim), hoặc bỏ ghim"
        }
        "Pinned messages: Enter goes to one, P unpins it" => {
            "Tin nhắn đã ghim: Enter để đi đến, P để bỏ ghim"
        }
        "Go to the message a reply answers" => "Đến tin nhắn được trả lời",
        "Go to the first unread message, under the \"Unread messages\" line" => {
            "Đến tin nhắn chưa đọc đầu tiên, dưới dòng \"Tin nhắn chưa đọc\""
        }
        "Go to the oldest unread message that mentions you or answers yours (@ in the chat list), and seeing it reads it; once you've seen them all, to the one before / after the cursor" => {
            "Đến tin nhắn chưa đọc cũ nhất nhắc đến bạn hoặc trả lời tin nhắn của bạn (@ trong danh sách trò chuyện), xem là đã đọc; khi đã xem hết, đến tin trước / sau con trỏ"
        }
        "Forward again, as Ctrl-i (most terminals send Ctrl-i as Tab)" => {
            "Tiến tới lại, như Ctrl-i (đa số terminal gửi Ctrl-i thành Tab)"
        }
        "Write a message" => "Soạn tin nhắn",
        "Attach a file by its path (Tab completes it)" => {
            "Đính kèm tệp theo đường dẫn (Tab để điền tiếp)"
        }
        "Paste a photo, files or text from the clipboard" => {
            "Dán ảnh, tệp hoặc văn bản từ bộ nhớ tạm"
        }
        "Search the whole chat; a has: search lists every match in a media browser" => {
            "Tìm trong toàn bộ cuộc trò chuyện; tìm với has: liệt kê mọi kết quả trong trình duyệt tệp phương tiện"
        }
        "In a search: from:@alice, from:me, from:this or from:a name; has:photo (video, media, file, link, voice, note, gif, audio)" => {
            "Khi tìm: from:@alice, from:me, from:this hoặc from:tên; has:photo (video, media, file, link, voice, note, gif, audio)"
        }
        "In a search: before:2025-10-01, after:2025-09-01 (that day counts)" => {
            "Khi tìm: before:2025-10-01, after:2025-09-01 (tính cả ngày đó)"
        }
        "Next older / newer match" => "Kết quả cũ hơn / mới hơn tiếp theo",
        "Close the chat, leaving only the chat list" => {
            "Đóng cuộc trò chuyện, chỉ để lại danh sách trò chuyện"
        }
        "End the search, cancel the edit, remove the files, cancel the reply, or go back" => {
            "Kết thúc tìm kiếm, hủy chỉnh sửa, bỏ các tệp, hủy trả lời, hoặc quay lại"
        }
        "Back to the chat list, or a forum's topics (l with the list on the right)" => {
            "Quay lại danh sách trò chuyện, hoặc các chủ đề của diễn đàn (l nếu danh sách ở bên phải)"
        }
        "Writing" => "Soạn tin",
        "Send, or save the edit" => "Gửi, hoặc lưu chỉnh sửa",
        "New line" => "Xuống dòng",
        "Delete back to the start of the line" => "Xóa ngược về đầu dòng",
        "**bold**" => "**đậm**",
        "Formatting: __italic__ ~~strike~~ ||spoiler|| `code` ```block``` [text](url)" => {
            "Định dạng: __nghiêng__ ~~gạch ngang~~ ||ẩn|| `mã` ```khối``` [chữ](url)"
        }
        "@name / :smile" => "@tên / :smile",
        "Suggests people in the group, or emoji; Tab puts one in" => {
            "Gợi ý người trong nhóm, hoặc emoji; Tab để chèn"
        }
        "/command" => "/lệnh",
        "Starting a message: the bots' commands" => "Ở đầu tin nhắn: lệnh của các bot",
        "Drop a file" => "Thả tệp vào",
        "Attach it; the text you write is its caption" => {
            "Đính kèm tệp; văn bản bạn viết là chú thích"
        }
        "Turn files just pasted back into their path as text" => {
            "Đổi tệp vừa dán trở lại thành đường dẫn dạng văn bản"
        }
        "Send attached photos as files, uncompressed, or back" => {
            "Gửi ảnh đính kèm dạng tệp, không nén, hoặc ngược lại"
        }
        "Stickers" => "Nhãn dán",
        "Back to Normal mode; what's written stays as the chat's draft, on your other devices too" => {
            "Về chế độ thường; nội dung đã viết được giữ làm bản nháp của cuộc trò chuyện, cả trên các thiết bị khác của bạn"
        }
        "Selecting text (v)" => "Chọn văn bản (v)",
        "Move a character, or a row on screen" => {
            "Di chuyển một ký tự, hoặc một hàng trên màn hình"
        }
        "Next word / word before / end of the word" => "Từ sau / từ trước / cuối từ",
        "Start / end of the line" => "Đầu / cuối dòng",
        "Start / end of the text" => "Đầu / cuối văn bản",
        "Start selecting where the cursor is, or stop; it takes in both ends, as in vim" => {
            "Bắt đầu chọn từ chỗ con trỏ, hoặc dừng; lấy cả hai đầu, như trong vim"
        }
        "Copy what's selected and close" => "Sao chép phần đã chọn và đóng",
        "Stop selecting, then close" => "Thôi chọn, rồi đóng",
        "Move" => "Di chuyển",
        "Previous / next tab: Recent, Favorites, your sets" => {
            "Tab trước / sau: Gần đây, Yêu thích, các bộ của bạn"
        }
        "Send the sticker (as the reply, if you're replying)" => {
            "Gửi nhãn dán (làm câu trả lời nếu đang trả lời)"
        }
        "Find stickers by emoji or word" => "Tìm nhãn dán theo emoji hoặc từ khóa",
        "Back to writing" => "Quay lại soạn tin",
        "Photo viewer" => "Trình xem ảnh",
        "The photo before / after it in the chat" => "Ảnh trước / sau trong cuộc trò chuyện",
        "Zoom in / out (also + / -), up to filling the window" => {
            "Phóng to / thu nhỏ (cả + / -), tối đa vừa cửa sổ"
        }
        "Open the photo in your computer's viewer" => "Mở ảnh bằng trình xem ảnh của máy tính",
        "Copy the photo" => "Sao chép ảnh",
        "Close" => "Đóng",
        "Video player" => "Trình phát video",
        "Play (once) or pause, right in the message (size in Settings)" => {
            "Phát (một lần) hoặc tạm dừng, ngay trong tin nhắn (cỡ trong Cài đặt)"
        }
        "Twice as big in the chat, or back" => "To gấp đôi trong cuộc trò chuyện, hoặc trở lại",
        "Zoom in / out, on the whole window" => "Phóng to / thu nhỏ, trên toàn cửa sổ",
        "Open the video in your computer's player" => "Mở video bằng trình phát của máy tính",
        "Fill the window; it closes once played" => "Lấp đầy cửa sổ; tự đóng khi phát xong",
        "Bot buttons" => "Nút của bot",
        "Move, as the buttons are laid out" => "Di chuyển theo cách bố trí các nút",
        "Next / previous button" => "Nút sau / trước",
        "Press it: the bot answers, a link opens (asking first), a reply button sends its words" => {
            "Bấm nút: bot trả lời, liên kết được mở (hỏi trước), nút trả lời gửi nội dung của nó"
        }
        "Secret chats" => "Cuộc trò chuyện bí mật",
        "Start one with the person in this chat (asks first); it's kept on this computer only" => {
            "Bắt đầu với người trong cuộc trò chuyện này (hỏi trước); chỉ lưu trên máy tính này"
        }
        "Its key as a picture, to compare with the other person's" => {
            "Khóa của nó dưới dạng hình, để so với khóa của người kia"
        }
        "How long new messages last once they're seen" => {
            "Tin nhắn mới tồn tại bao lâu sau khi được xem"
        }
        "End it for both of you, and delete it here (asks first)" => {
            "Kết thúc cho cả hai người, và xóa ở đây (hỏi trước)"
        }
        "On a 🔥 photo: view it; it's covered again when you move on" => {
            "Trên ảnh 🔥: xem ảnh; ảnh bị che lại khi bạn chuyển đi"
        }
        "Search and command prompts" => "Dòng tìm kiếm và lệnh",
        "Search, keep the chat filter, or run the command" => {
            "Tìm, giữ bộ lọc cuộc trò chuyện, hoặc chạy lệnh"
        }
        "Complete a command or a search filter (again for the next one), or the path of a file to attach" => {
            "Điền tiếp lệnh hoặc bộ lọc tìm kiếm (nhấn lại để sang cái tiếp theo), hoặc đường dẫn tệp để đính kèm"
        }
        "Cancel" => "Hủy",
        "Forward and find popups" => "Cửa sổ chuyển tiếp và tìm",
        "Type" => "Gõ",
        "Search chats by name, or @username" => "Tìm cuộc trò chuyện theo tên, hoặc @username",
        "Move (also Ctrl-p / Ctrl-n, Tab)" => "Di chuyển (cả Ctrl-p / Ctrl-n, Tab)",
        "Forward there, or open it" => "Chuyển tiếp đến đó, hoặc mở",
        "Menus and popups" => "Menu và cửa sổ bật lên",
        "Choose" => "Chọn",
        "Choose by number" => "Chọn theo số",
        "Tick a setting, or use a sound or a theme (Enter too); saved at once" => {
            "Đánh dấu một cài đặt, hoặc dùng âm thanh hay giao diện (cả Enter); lưu ngay"
        }
        "Switch tabs in this popup" => "Chuyển tab trong cửa sổ này",
        "Go ahead or not, when asked about a file, a link or joining" => {
            "Đồng ý hay không, khi được hỏi về tệp, liên kết hoặc tham gia"
        }
        // ui/info.rs
        "Couldn't get it" => "Không lấy được thông tin",
        "Username" => "Tên người dùng",
        "Link" => "Liên kết",
        "Phone" => "Điện thoại",
        "Bio" => "Tiểu sử",
        "About" => "Mô tả",
        "In common" => "Nhóm chung",
        " `j/k` move · `Enter` write to them · `Esc` close " => {
            " `j/k` di chuyển · `Enter` nhắn cho họ · `Esc` đóng "
        }
        " `Esc` close " => " `Esc` đóng ",
        " Info " => " Thông tin ",
        "Subscribers" => "Người đăng ký",
        "Members" => "Thành viên",
        "Nobody to show" => "Không có ai để hiện",
        "secret chat" => "cuộc trò chuyện bí mật",
        "channel" => "kênh",
        "group with topics" => "nhóm có chủ đề",
        "group" => "nhóm",
        "owner" => "chủ sở hữu",
        "admin" => "quản trị viên",
        // ui/messages.rs
        "· waiting for {name} to come online " => "· đang chờ {name} trực tuyến ",
        "last seen today at {time}" => "truy cập lúc {time} hôm nay",
        "last seen yesterday at {time}" => "truy cập lúc {time} hôm qua",
        "last seen {date}" => "truy cập ngày {date}",
        " · closed" => " · đã đóng",
        "Unknown" => "Không rõ",
        "· ended " => "· đã kết thúc ",
        "· loading… " => "· đang tải… ",
        "No messages yet" => "Chưa có tin nhắn nào",
        "last seen a long time ago" => "truy cập đã lâu",
        "bot" => "bot",
        "online" => "trực tuyến",
        "last seen recently" => "truy cập gần đây",
        "last seen within a week" => "truy cập trong tuần",
        "last seen within a month" => "truy cập trong tháng",
        "last seen just now" => "vừa mới truy cập",
        "last seen a minute ago" => "truy cập 1 phút trước",
        "Preview unavailable" => "Không có bản xem trước",
        "Forwarded from" => "Được chuyển tiếp từ",
        "edited" => "đã chỉnh sửa",
        "sending" => "đang gửi",
        "sending…" => "đang gửi…",
        "not sent" => "chưa gửi được",
        " Unread messages " => " Tin nhắn chưa đọc ",
        "Deleted message" => "Tin nhắn đã bị xóa",
        "Pinned" => "Tin nhắn đã ghim",
        "list " => "danh sách ",
        " (me)" => " (tôi)",
        // ui/mod.rs
        "{name} is {doing}…" => "{name} {doing}…",
        "{a} and {b} are {doing}…" => "{a} và {b} {doing}…",
        "Telegram sent a code {sent_via}." => "Telegram đã gửi mã {sent_via}.",
        "Can't use {from}" => "Không thể dùng {from}",
        "{why}. Fix it, then start tuigram again. Until then it doesn't connect, rather than connect without the proxy." => {
            "{why}. Hãy sửa lỗi rồi khởi động lại tuigram. Cho đến lúc đó, tuigram sẽ không kết nối, thay vì kết nối mà không qua proxy."
        }
        "In Telegram on your phone: Settings → Devices → Link Desktop Device, then point the camera at this code." => {
            "Trong Telegram trên điện thoại: Cài đặt → Thiết bị → Liên kết thiết bị máy tính, rồi hướng camera vào mã này."
        }
        "Make the terminal at least {width} columns by {height} rows to show the QR code." => {
            "Hãy mở rộng terminal ít nhất {width} cột × {height} hàng để hiện mã QR."
        }
        "If {name} sees the same picture in this chat on their device, nobody can read it but the two of you." => {
            "Nếu {name} thấy cùng hình này trong cuộc trò chuyện trên thiết bị của họ, không ai ngoài hai bạn đọc được nó."
        }
        "{n} more: {names}" => "{n} tệp nữa: {names}",
        " … {n} more" => " … và {n} mục nữa",
        " Log in " => " Đăng nhập ",
        "Logging out…" => "Đang đăng xuất…",
        "Then you can log in again, as yourself or someone else." => {
            "Sau đó bạn có thể đăng nhập lại, bằng tài khoản của mình hoặc của người khác."
        }
        "Builds from source have no API key. Get yours once at my.telegram.org → API development tools (any app name), then paste api_id.\nOr skip it all: cargo binstall tuigram-cli" => {
            "Bản dựng từ mã nguồn không có khóa API. Hãy lấy khóa một lần tại my.telegram.org → API development tools (tên ứng dụng tùy ý), rồi dán api_id.\nHoặc bỏ qua tất cả: cargo binstall tuigram-cli"
        }
        "Get your own API ID and hash once at my.telegram.org → API development tools (any app name), then paste api_id." => {
            "Hãy lấy API ID và hash của riêng bạn một lần tại my.telegram.org → API development tools (tên ứng dụng tùy ý), rồi dán api_id."
        }
        "Now paste api_hash from the same page. Both stay on this computer." => {
            "Giờ hãy dán api_hash từ cùng trang đó. Cả hai chỉ được lưu trên máy tính này."
        }
        "Phone number" => "Số điện thoại",
        "Include the country code, e.g. +1 415 555 0123. Or press Tab to log in by scanning a QR code with Telegram on your phone." => {
            "Kèm mã quốc gia, ví dụ +1 415 555 0123. Hoặc nhấn Tab để đăng nhập bằng cách quét mã QR với Telegram trên điện thoại."
        }
        "Login code" => "Mã đăng nhập",
        "Two-step verification password" => "Mật khẩu xác minh hai bước",
        "Your account has a cloud password." => "Tài khoản của bạn có mật khẩu đám mây.",
        "Hint:" => "Gợi ý:",
        "Email address" => "Địa chỉ email",
        "Telegram needs an email address to send login codes to." => {
            "Telegram cần một địa chỉ email để gửi mã đăng nhập."
        }
        "Email code" => "Mã trong email",
        "Enter the code Telegram sent to your email." => {
            "Nhập mã Telegram đã gửi đến email của bạn."
        }
        "Confirm on another device" => "Xác nhận trên thiết bị khác",
        "Open this link on a logged-in device:" => "Mở liên kết này trên thiết bị đã đăng nhập:",
        "Can't log in here" => "Không thể đăng nhập ở đây",
        "Blocked here? See TG_PROXY in tuigram --help" => {
            "Bị chặn ở đây? Xem TG_PROXY trong tuigram --help"
        }
        "Sending…" => "Đang gửi…",
        "`Enter` submit · `Tab` QR code · `Ctrl-c` quit" => {
            "`Enter` gửi · `Tab` mã QR · `Ctrl-c` thoát"
        }
        "`Enter` submit · `Ctrl-c` quit" => "`Enter` gửi · `Ctrl-c` thoát",
        "`Ctrl-c` quit" => "`Ctrl-c` thoát",
        "`Esc` use phone number · `Ctrl-c` quit" => "`Esc` dùng số điện thoại · `Ctrl-c` thoát",
        " Scan the QR code " => " Quét mã QR ",
        "Press Enter to open a topic" => "Nhấn Enter để mở chủ đề",
        "Press Enter to open a chat" => "Nhấn Enter để mở cuộc trò chuyện",
        " Delete message " => " Xóa tin nhắn ",
        " `Enter` delete · `Esc` cancel " => " `Enter` xóa · `Esc` hủy ",
        " Checking…" => " Đang kiểm tra…",
        " `Tab` insert " => " `Tab` chèn ",
        " `Space` tick · `Enter` vote · `Esc` close " => {
            " `Space` đánh dấu · `Enter` bỏ phiếu · `Esc` đóng "
        }
        " `Enter` vote · `Esc` close " => " `Enter` bỏ phiếu · `Esc` đóng ",
        " Vote " => " Bỏ phiếu ",
        "  Take back my vote" => "  Rút lại phiếu bầu",
        " Forward to " => " Chuyển tiếp đến ",
        " `Enter` forward · `Esc` cancel " => " `Enter` chuyển tiếp · `Esc` hủy ",
        "type to find a chat" => "gõ để tìm cuộc trò chuyện",
        " Find a chat or person " => " Tìm cuộc trò chuyện hoặc người ",
        " `Enter` open · `Esc` cancel " => " `Enter` mở · `Esc` hủy ",
        "a name, @username or t.me link" => "tên, @username hoặc liên kết t.me",
        "· searching… " => "· đang tìm… ",
        " Searching Telegram…" => " Đang tìm trên Telegram…",
        " No chats yet" => " Chưa có cuộc trò chuyện nào",
        " Nothing found" => " Không tìm thấy",
        "public" => "công khai",
        "contact" => "liên hệ",
        "`Enter` take back" => "`Enter` gỡ",
        "`Enter` react" => "`Enter` thả cảm xúc",
        "back" => "quay lại",
        "search" => "tìm",
        "close" => "đóng",
        " React " => " Thả cảm xúc ",
        " removes all yours, also in the chat" => " gỡ hết cảm xúc, cả trong trò chuyện",
        " Loading…" => " Đang tải…",
        " No emoji by that name" => " Không có emoji nào tên như vậy",
        " Shortcuts " => " Phím tắt ",
        " Settings " => " Cài đặt ",
        " `j/k` scroll · `Tab` settings · `Esc` close " => {
            " `j/k` cuộn · `Tab` cài đặt · `Esc` đóng "
        }
        " `Enter` or `Space` use · `Tab` shortcuts · `Esc` close " => {
            " `Enter` hoặc `Space` dùng · `Tab` phím tắt · `Esc` đóng "
        }
        " `Enter` or `Space` change · `Tab` shortcuts · `Esc` close " => {
            " `Enter` hoặc `Space` thay đổi · `Tab` phím tắt · `Esc` đóng "
        }
        " Notifications" => " Thông báo",
        "New messages, while tuigram is in the background" => {
            "Tin nhắn mới, khi tuigram đang chạy nền"
        }
        " Chat list" => " Danh sách trò chuyện",
        "A gap between chats" => "Khoảng cách giữa các cuộc trò chuyện",
        "On the right side of the window" => "Ở bên phải cửa sổ",
        " Messages" => " Tin nhắn",
        "A gap between messages in a row from one person" => {
            "Khoảng cách giữa các tin nhắn liên tiếp của cùng một người"
        }
        " [small] " => " [nhỏ] ",
        " [medium] " => " [vừa] ",
        " [large] " => " [lớn] ",
        "Video messages play on Space" => "Tin nhắn video phát khi nhấn Space",
        " Look" => " Hiển thị",
        "Round corners (turn off if they look broken)" => "Bo tròn góc (tắt nếu trông bị lỗi)",
        "Round pills (needs a Nerd Font)" => "Nhãn bo tròn (cần Nerd Font)",
        "Animations (lights on focus, popups, toasts, new messages)" => {
            "Hiệu ứng (viền sáng, cửa sổ bật lên, thông báo nhanh, tin nhắn mới)"
        }
        " Composer" => " Khung soạn tin",
        "Back to Normal mode after sending a message" => "Về chế độ thường sau khi gửi tin nhắn",
        "Wrap long lines onto the next row" => "Ngắt dòng dài xuống hàng tiếp theo",
        " Secret chats" => " Cuộc trò chuyện bí mật",
        "Take the ones others start here, not on your phone" => {
            "Nhận các cuộc trò chuyện bí mật do những người khác khởi tạo bằng tuigram, thay vì trên điện thoại"
        }
        " Sound for new messages" => " Âm thanh tin nhắn mới",
        " Not played over SSH: it would play on the other computer" => {
            " Không phát qua SSH: âm thanh sẽ phát trên máy tính kia"
        }
        "Also while tuigram's window is focused" => "Cả khi cửa sổ tuigram đang được chọn",
        "None" => "Không có",
        " Your own sounds, WAV or Ogg Opus, go in" => {
            " Âm thanh riêng của bạn, WAV hoặc Ogg Opus, đặt trong"
        }
        " Theme" => " Giao diện",
        " Your own themes go in" => " Giao diện riêng của bạn đặt trong",
        "Telegram marks it as a SCAM" => "Telegram đánh dấu là LỪA ĐẢO",
        "Telegram marks it as FAKE: it poses as someone else" => {
            "Telegram đánh dấu là GIẢ MẠO: mạo danh người khác"
        }
        "Verified by Telegram ✓" => "Đã được Telegram xác minh ✓",
        "cancel" => "hủy",
        " Reply buttons " => " Nút trả lời ",
        " `Enter` send · `Esc` close " => " `Enter` gửi · `Esc` đóng ",
        " Buttons " => " Nút ",
        " `Enter` press · `Esc` close " => " `Enter` bấm · `Esc` đóng ",
        " Pin message " => " Ghim tin nhắn ",
        " `Enter` pin · `Esc` cancel " => " `Enter` ghim · `Esc` hủy ",
        " New messages disappear this long after they're seen." => {
            " Tin nhắn mới tự hủy sau chừng này thời gian kể từ khi được xem."
        }
        " Self-destruct timer · " => " Hẹn giờ tự hủy · ",
        " `Enter` set · `Esc` cancel " => " `Enter` đặt · `Esc` hủy ",
        " Make the window bigger to compare the key." => " Hãy phóng to cửa sổ để so sánh khóa.",
        " Encryption key " => " Khóa mã hóa ",
        " `Enter` close " => " `Enter` đóng ",
        "Encryption key" => "Khóa mã hóa",
        "Pinned messages" => "Tin nhắn đã ghim",
        " `Enter` go to · `P` unpin · `Esc` close " => {
            " `Enter` đi đến · `P` bỏ ghim · `Esc` đóng "
        }
        " `Enter` go to · `O` show file · `Esc` close " => {
            " `Enter` đi đến · `O` hiện tệp · `Esc` đóng "
        }
        " Searching… " => " Đang tìm… ",
        " Nothing found " => " Không tìm thấy ",
        " Open " => " Mở ",
        " `Enter` open · `1-9` pick · `Esc` close " => " `Enter` mở · `1-9` chọn · `Esc` đóng ",
        " Copy " => " Sao chép ",
        " `Enter` copy · `1-9` pick · `Esc` close " => {
            " `Enter` sao chép · `1-9` chọn · `Esc` đóng "
        }
        "secret chat " => "cuộc trò chuyện bí mật ",
        "Write the new text…" => "Viết nội dung mới…",
        "Add a caption…" => "Thêm chú thích…",
        "Write a reply… · `Tab` for stickers" => "Viết trả lời… · `Tab` để chọn nhãn dán",
        "Write a message… · `Tab` for stickers" => "Viết tin nhắn… · `Tab` để chọn nhãn dán",
        "Press `i` to edit" => "Nhấn `i` để chỉnh sửa",
        "Press `i` to add a caption" => "Nhấn `i` để thêm chú thích",
        "Press `i` to write your reply" => "Nhấn `i` để viết trả lời",
        "Press `i` to write a message" => "Nhấn `i` để viết tin nhắn",
        "File, uncompressed" => "Tệp, không nén",
        "↩ Reply to " => "↩ Trả lời ",
        "✎ Edit message" => "✎ Chỉnh sửa tin nhắn",
        " SEARCH " => " TÌM KIẾM ",
        "  `Enter` done · `Esc` cancel " => "  `Enter` xong · `Esc` hủy ",
        "  `Tab` filters: from: has: before: after: · `Enter` search · `Esc` cancel " => {
            "  `Tab` bộ lọc: from: has: before: after: · `Enter` tìm · `Esc` hủy "
        }
        " COMMAND " => " LỆNH ",
        "  `Tab` complete · `Enter` run · `Esc` cancel " => {
            "  `Tab` điền tiếp · `Enter` chạy · `Esc` hủy "
        }
        " ATTACH " => " ĐÍNH KÈM ",
        "  `Tab` complete · `Enter` attach · `Esc` cancel " => {
            "  `Tab` điền tiếp · `Enter` đính kèm · `Esc` hủy "
        }
        " PROXY " => " PROXY ",
        "  socks5://, http:// or t.me/proxy link · `Enter` use it, empty for none · `Esc` cancel " => {
            "  liên kết socks5://, http:// hoặc t.me/proxy · `Enter` dùng, để trống nếu không dùng · `Esc` hủy "
        }
        " Commands · Tab completes " => " Lệnh · Tab để điền tiếp ",
        " Matches · type more, then Tab " => " Kết quả khớp · gõ thêm rồi nhấn Tab ",
        "`Ctrl-t` send as photos" => "`Ctrl-t` gửi dạng ảnh",
        "`Ctrl-t` send as files" => "`Ctrl-t` gửi dạng tệp",
        "`Enter` view it" => "`Enter` xem",
        "`Enter` show spoiler" => "`Enter` hiện nội dung ẩn",
        "`Enter` buttons" => "`Enter` các nút",
        "`gd` go to replied" => "`gd` đến tin được trả lời",
        "`Ctrl-o` back" => "`Ctrl-o` quay lại",
        "`Ctrl-i` forward" => "`Ctrl-i` tiến tới",
        " NORMAL " => " THƯỜNG ",
        " INSERT " => " NHẬP ",
        " STICKERS " => " NHÃN DÁN ",
        " RESIZE " => " ĐỔI CỠ ",
        "  `h` narrower · `l` wider · `=` as at first · saved at once · `Esc` done" => {
            "  `h` hẹp hơn · `l` rộng hơn · `=` như ban đầu · lưu ngay · `Esc` xong"
        }
        "  `h` wider · `l` narrower · `=` as at first · saved at once · `Esc` done" => {
            "  `h` rộng hơn · `l` hẹp hơn · `=` như ban đầu · lưu ngay · `Esc` xong"
        }
        "  `y` edit · `n` or `Esc` cancel" => "  `y` chỉnh sửa · `n` hoặc `Esc` hủy",
        "  `y` join · `n` or `Esc` cancel" => "  `y` tham gia · `n` hoặc `Esc` hủy",
        "  `y` leave · `n` or `Esc` cancel" => "  `y` rời · `n` hoặc `Esc` hủy",
        "  `y` log out · `n` or `Esc` cancel" => "  `y` đăng xuất · `n` hoặc `Esc` hủy",
        "  `y` start it · `n` or `Esc` cancel" => "  `y` bắt đầu · `n` hoặc `Esc` hủy",
        "  `y` end it · `n` or `Esc` cancel" => "  `y` kết thúc · `n` hoặc `Esc` hủy",
        "  `y` use it · `n` or `Esc` cancel" => "  `y` dùng · `n` hoặc `Esc` hủy",
        "  `y` open · `n` or `Esc` cancel" => "  `y` mở · `n` hoặc `Esc` hủy",
        "  `j/k` scroll · `Tab` settings · `Esc` close" => {
            "  `j/k` cuộn · `Tab` cài đặt · `Esc` đóng"
        }
        "  `j/k` move · `Enter` or `Space` change, saved at once · `Tab` shortcuts · `Esc` close" => {
            "  `j/k` di chuyển · `Enter` hoặc `Space` thay đổi, lưu ngay · `Tab` phím tắt · `Esc` đóng"
        }
        "  `j/k` choose · `Enter` delete · `Esc` cancel" => {
            "  `j/k` chọn · `Enter` xóa · `Esc` hủy"
        }
        "  type a name, like heart or +1 · `arrows` choose · `Enter` react · `Esc` back" => {
            "  gõ tên, ví dụ heart hoặc +1 · `mũi tên` chọn · `Enter` thả cảm xúc · `Esc` quay lại"
        }
        "  `h/j/k/l` choose · `Enter` react, or take yours back · `X` take all yours back · `/` search · `Esc` close" => {
            "  `h/j/k/l` chọn · `Enter` thả cảm xúc, hoặc gỡ của bạn · `X` gỡ hết của bạn · `/` tìm · `Esc` đóng"
        }
        "  `j/k` choose · `Space` tick · `Enter` vote for the ticked · `Esc` close" => {
            "  `j/k` chọn · `Space` đánh dấu · `Enter` bỏ phiếu cho mục đã đánh dấu · `Esc` đóng"
        }
        "  `j/k` choose · `Enter` vote · `Esc` close" => {
            "  `j/k` chọn · `Enter` bỏ phiếu · `Esc` đóng"
        }
        "  `Enter` or `Esc` close" => "  `Enter` hoặc `Esc` đóng",
        "  `j/k` choose · `Enter` pin · `Esc` cancel" => {
            "  `j/k` chọn · `Enter` ghim · `Esc` hủy"
        }
        "  `j/k` choose · `Enter` set the timer · `Esc` cancel" => {
            "  `j/k` chọn · `Enter` đặt hẹn giờ · `Esc` hủy"
        }
        "  `j/k` move · `Enter` write to them · `g/G` first/last · `Esc` close" => {
            "  `j/k` di chuyển · `Enter` nhắn cho họ · `g/G` đầu/cuối · `Esc` đóng"
        }
        "  `Esc` close" => "  `Esc` đóng",
        " VISUAL " => " CHỌN ",
        "  `y` copy · `h/j/k/l` `w/b/e` `0/$` `g/G` move the end · `v` or `Esc` stop selecting" => {
            "  `y` sao chép · `h/j/k/l` `w/b/e` `0/$` `g/G` dời điểm cuối · `v` hoặc `Esc` thôi chọn"
        }
        "  `v` start selecting · `h/j/k/l` move · `w/b/e` by word · `0/$` line start/end · `g/G` start/end · `Ctrl-d/u` half page · `Esc` close" => {
            "  `v` bắt đầu chọn · `h/j/k/l` di chuyển · `w/b/e` theo từ · `0/$` đầu/cuối dòng · `g/G` đầu/cuối · `Ctrl-d/u` nửa trang · `Esc` đóng"
        }
        "  `h/l` older/newer photo · `j/k` zoom in/out · `o` open in its app · `y` copy · `Enter` or `Esc` close" => {
            "  `h/l` ảnh cũ hơn/mới hơn · `j/k` phóng to/thu nhỏ · `o` mở bằng ứng dụng · `y` sao chép · `Enter` hoặc `Esc` đóng"
        }
        "  `Space` pause · `j/k` zoom in/out · `o` open in its app · `Enter` or `Esc` close" => {
            "  `Space` tạm dừng · `j/k` phóng to/thu nhỏ · `o` mở bằng ứng dụng · `Enter` hoặc `Esc` đóng"
        }
        "  `j/k` choose · `Enter` go to it · `P` unpin it · `Esc` close" => {
            "  `j/k` chọn · `Enter` đi đến · `P` bỏ ghim · `Esc` đóng"
        }
        "  `j/k` choose · `Enter` go to it · `O` show the file · `Esc` close" => {
            "  `j/k` chọn · `Enter` đi đến · `O` hiện tệp · `Esc` đóng"
        }
        "  `h/j/k/l` choose · `Enter` send its words · `Esc` close" => {
            "  `h/j/k/l` chọn · `Enter` gửi nội dung nút · `Esc` đóng"
        }
        "  `h/j/k/l` choose · `Enter` press · `Esc` close" => {
            "  `h/j/k/l` chọn · `Enter` bấm · `Esc` đóng"
        }
        "  type a chat's name · `arrows` choose · `Enter` forward · `Esc` cancel" => {
            "  gõ tên cuộc trò chuyện · `mũi tên` chọn · `Enter` chuyển tiếp · `Esc` hủy"
        }
        "  type a name, @username or t.me link · `arrows` choose · `Enter` open · `Esc` cancel" => {
            "  gõ tên, @username hoặc liên kết t.me · `mũi tên` chọn · `Enter` mở · `Esc` hủy"
        }
        "  `j/k` move · `Enter` open · `Esc` clear search · `/` search again · `i` write · `q` quit" => {
            "  `j/k` di chuyển · `Enter` mở · `Esc` xóa tìm kiếm · `/` tìm lại · `i` soạn tin · `q` thoát"
        }
        "  `j/k` move · `Enter` open · `i` write · `Tab/Shift-Tab` folders · `/` search · `s` find anyone · `p` pin · `m` mute · `a` archive · `I` info · `Ctrl-o/i` back/forward · `H` highlight · `gg/G` top/bottom · `Ctrl-d/u` half page · `Ctrl-r` resize · `:` commands · `?` help · `q` quit" => {
            "  `j/k` di chuyển · `Enter` mở · `i` soạn tin · `Tab/Shift-Tab` thư mục · `/` tìm · `s` tìm bất kỳ ai · `p` ghim · `m` tắt thông báo · `a` lưu trữ · `I` thông tin · `Ctrl-o/i` lùi/tiến · `H` highlight · `gg/G` đầu/cuối · `Ctrl-d/u` nửa trang · `Ctrl-r` đổi cỡ · `:` lệnh · `?` trợ giúp · `q` thoát"
        }
        "  `j/k` move · `Enter` open · `i` write · `/` search · `s` find anyone · `p` pin · `m` mute · `a` archive · `I` info · `H` highlight · `gg/G` top/bottom · `Ctrl-d/u` half page · `Ctrl-r` resize · `:` commands · `?` help · `q` quit" => {
            "  `j/k` di chuyển · `Enter` mở · `i` soạn tin · `/` tìm · `s` tìm bất kỳ ai · `p` ghim · `m` tắt thông báo · `a` lưu trữ · `I` thông tin · `H` highlight · `gg/G` đầu/cuối · `Ctrl-d/u` nửa trang · `Ctrl-r` đổi cỡ · `:` lệnh · `?` trợ giúp · `q` thoát"
        }
        "  `j/k` move · `Enter` open · `i` write · `h` back · `s` find anyone · `I` info · `Ctrl-o/i` back/forward · `gg/G` top/bottom · `Ctrl-d/u` half page · `Ctrl-r` resize · `:` commands · `?` help · `q` quit" => {
            "  `j/k` di chuyển · `Enter` mở · `i` soạn tin · `h` quay lại · `s` tìm bất kỳ ai · `I` thông tin · `Ctrl-o/i` lùi/tiến · `gg/G` đầu/cuối · `Ctrl-d/u` nửa trang · `Ctrl-r` đổi cỡ · `:` lệnh · `?` trợ giúp · `q` thoát"
        }
        "  `n/N` older/newer match · `Esc` end search · `/` search again · `j/k` newer/older · `Enter` open/play media · `r` reply · `i` write · `h` back" => {
            "  `n/N` kết quả cũ hơn/mới hơn · `Esc` kết thúc tìm · `/` tìm lại · `j/k` mới hơn/cũ hơn · `Enter` mở/phát · `r` trả lời · `i` soạn tin · `h` quay lại"
        }
        "  `i` edit · `Esc` cancel edit · `e` edit selected instead · `j/k` newer/older · `h` back" => {
            "  `i` chỉnh sửa · `Esc` hủy chỉnh sửa · `e` sửa tin đang chọn · `j/k` mới hơn/cũ hơn · `h` quay lại"
        }
        "  `i` add caption · `Esc` remove files · `a` attach more · `p` paste more · `r` reply · `j/k` newer/older · `h` back" => {
            "  `i` thêm chú thích · `Esc` bỏ các tệp · `a` đính kèm thêm · `p` dán thêm · `r` trả lời · `j/k` mới hơn/cũ hơn · `h` quay lại"
        }
        "  `i` write reply · `Esc` cancel reply · `r` reply to selected instead · `j/k` newer/older · `Enter` open/play media · `h` back" => {
            "  `i` viết trả lời · `Esc` hủy trả lời · `r` trả lời tin đang chọn · `j/k` mới hơn/cũ hơn · `Enter` mở/phát · `h` quay lại"
        }
        "  you're not in this chat · `J` join · `j/k` newer/older · `Enter` open/play media · `s` find anyone · `h` back" => {
            "  bạn không ở trong cuộc trò chuyện này · `J` tham gia · `j/k` mới hơn/cũ hơn · `Enter` mở/phát · `s` tìm bất kỳ ai · `h` quay lại"
        }
        "  `j/k` newer/older · `y` copy · `r` reply · `f` forward · `R` react · `X` unreact · `e` edit · `d` delete · `P` pin · `gp` pinned · `gu/gm/gM` unread/mention · `Enter` open/play media · `i` write · `a` attach · `p` paste · `/` search · `s` find anyone · `I` info · `gg/G` oldest/newest · `h` back · `Ctrl-r` resize · `:` commands · `?` help · `q` quit" => {
            "  `j/k` mới hơn/cũ hơn · `y` sao chép · `r` trả lời · `f` chuyển tiếp · `R` thả cảm xúc · `X` gỡ cảm xúc · `e` chỉnh sửa · `d` xóa · `P` ghim · `gp` tin đã ghim · `gu/gm/gM` chưa đọc/nhắc đến · `Enter` mở/phát · `i` soạn tin · `a` đính kèm · `p` dán · `/` tìm · `s` tìm bất kỳ ai · `I` thông tin · `gg/G` cũ nhất/mới nhất · `h` quay lại · `Ctrl-r` đổi cỡ · `:` lệnh · `?` trợ giúp · `q` thoát"
        }
        "  type an emoji or a word, like cat · `arrows` choose · `Enter` send · `Esc` back" => {
            "  gõ emoji hoặc một từ, ví dụ cat · `mũi tên` chọn · `Enter` gửi · `Esc` quay lại"
        }
        "  `h/j/k/l` choose · `H/L` previous/next tab · `Enter` send · `/` search · `Tab` or `Esc` back to writing" => {
            "  `h/j/k/l` chọn · `H/L` tab trước/sau · `Enter` gửi · `/` tìm · `Tab` hoặc `Esc` quay lại soạn tin"
        }
        "  `Tab` insert · `arrows` choose · keep typing to narrow it down · `Enter` send · `Esc` normal mode" => {
            "  `Tab` chèn · `mũi tên` chọn · gõ thêm để thu hẹp · `Enter` gửi · `Esc` chế độ thường"
        }
        "  `Enter` save · `Alt-Enter` or `Ctrl-j` new line · `Esc` normal mode · `Esc Esc` cancel edit" => {
            "  `Enter` lưu · `Alt-Enter` hoặc `Ctrl-j` xuống dòng · `Esc` chế độ thường · `Esc Esc` hủy chỉnh sửa"
        }
        "  `Enter` send · `Ctrl-z` paste as text instead · `Alt-Enter` or `Ctrl-j` new line · `Esc` normal mode · `Esc Esc` remove files" => {
            "  `Enter` gửi · `Ctrl-z` dán dạng văn bản · `Alt-Enter` hoặc `Ctrl-j` xuống dòng · `Esc` chế độ thường · `Esc Esc` bỏ các tệp"
        }
        "  `Enter` send · `Ctrl-v` paste more · `Alt-Enter` or `Ctrl-j` new line · `Esc` normal mode · `Esc Esc` remove files" => {
            "  `Enter` gửi · `Ctrl-v` dán thêm · `Alt-Enter` hoặc `Ctrl-j` xuống dòng · `Esc` chế độ thường · `Esc Esc` bỏ các tệp"
        }
        "  `Enter` send reply · `Alt-Enter` or `Ctrl-j` new line · `Tab` stickers · `Esc` normal mode · `Esc Esc` cancel reply" => {
            "  `Enter` gửi trả lời · `Alt-Enter` hoặc `Ctrl-j` xuống dòng · `Tab` nhãn dán · `Esc` chế độ thường · `Esc Esc` hủy trả lời"
        }
        "  `Enter` send · `Alt-Enter` or `Ctrl-j` new line · `Tab` stickers · `Ctrl-v` paste photo or file · `Esc` normal mode" => {
            "  `Enter` gửi · `Alt-Enter` hoặc `Ctrl-j` xuống dòng · `Tab` nhãn dán · `Ctrl-v` dán ảnh hoặc tệp · `Esc` chế độ thường"
        }
        "`gm` mention" => "`gm` lượt nhắc đến",
        "`Enter` send again" => "`Enter` gửi lại",
        "chat list" => "danh sách trò chuyện",
        "  Closing… (q again to force)" => "  Đang đóng… (nhấn q lần nữa để thoát ngay)",
        "  Downloading… opens when done" => "  Đang tải xuống… sẽ mở khi xong",
        "  Downloading… copies when done" => "  Đang tải xuống… sẽ sao chép khi xong",
        "  Reading the clipboard…" => "  Đang đọc bộ nhớ tạm…",
        "Looking for" => "Đang tìm",
        " (Esc stops)" => " (Esc để dừng)",
        " Language" => " Ngôn ngữ",
        // ui/select.rs
        " `y` copy · `Esc` cancel " => " `y` sao chép · `Esc` hủy ",
        " `v` select · `Esc` close " => " `v` chọn · `Esc` đóng ",
        " Select text " => " Chọn văn bản ",
        // ui/stickers.rs
        " Stickers " => " Nhãn dán ",
        "No stickers yet · `/` finds some by emoji or word" => {
            "Chưa có nhãn dán · `/` tìm theo emoji hoặc từ khóa"
        }
        "Searching…" => "Đang tìm…",
        "No stickers found" => "Không tìm thấy nhãn dán",
        "No stickers in this set" => "Bộ này không có nhãn dán",
        // ui/topics.rs
        "Draft: " => "Bản nháp: ",
        "Loading…" => "Đang tải…",
        " closed" => " đã đóng",
        " · topics loading… " => " · đang tải chủ đề… ",
        " · topics " => " · chủ đề ",
        "No topics" => "Không có chủ đề",
        // ui/viewer.rs
        " · only its thumbnail" => " · chỉ có ảnh thu nhỏ",
        " `h/l` older/newer · `j/k` zoom in/out · `o` open in its app · `Esc` close " => {
            " `h/l` cũ hơn/mới hơn · `j/k` phóng to/thu nhỏ · `o` mở bằng ứng dụng · `Esc` đóng "
        }
        "Can't show this photo" => "Không thể hiện ảnh này",
        // video.rs
        "decoding broke" => "giải mã bị gián đoạn",
        "the frame file can't be written" => "không thể ghi tệp khung hình",
        "it can't be played" => "không thể phát",
        "ffmpeg isn't installed" => "chưa cài đặt ffmpeg",
        "loading…" => "đang tải…",
        "paused" => "tạm dừng",
        "playing" => "đang phát",
        // viewer.rs
        "Self-destructing media can't be copied" => "Không thể sao chép nội dung tự hủy",
        "This chat doesn't allow copying its messages" => {
            "Cuộc trò chuyện này không cho phép sao chép tin nhắn"
        }
        // voice.rs
        "there's no thread to play it on" => "không có luồng để phát",
        "it's cut short" => "tệp bị cắt ngang",
        "the file can't be read" => "không thể đọc tệp",
        "it's too long" => "tệp quá dài",
        "the sound device stopped playing" => "thiết bị âm thanh đã ngừng phát",
        "part of it is too big" => "một phần của tệp quá lớn",
        "it can't be read" => "không thể đọc được",
        "it isn't an Ogg file" => "đây không phải tệp Ogg",
        "it's empty" => "tệp trống",
        "it isn't Opus" => "đây không phải Opus",
        "it's a newer kind of Opus file" => "đây là tệp Opus phiên bản mới hơn",
        "it has more than two channels" => "tệp có hơn hai kênh âm thanh",
        "part of it is broken" => "một phần của tệp bị hỏng",
        _ => return None,
    })
}

/// A count's words keyed by English's many form: Vietnamese has one form.
pub(super) fn forms(en: &str) -> Option<&'static [&'static str]> {
    Some(match en {
        // app.rs
        "A channel with {n} members." => &["Kênh có {n} người đăng ký."],
        "A group with {n} members." => &["Nhóm có {n} thành viên."],
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
        "{n} new messages" => &["{n} tin nhắn mới"],
        // pins.rs
        // poll.rs
        "{n} votes" => &["{n} phiếu"],
        // proxy.rs
        // reactions.rs
        // search.rs
        // secret.rs
        "{n} seconds" => &["{n} giây"],
        "{n} minutes" => &["{n} phút"],
        "{n} hours" => &["{n} giờ"],
        "{n} days" => &["{n} ngày"],
        "{n} weeks" => &["{n} tuần"],
        // service.rs
        "added {n} members" => &["đã thêm {n} thành viên"],
        "{n} others" => &["{n} người khác"],
        "{n} times" => &["{n} lần"],
        "{n} in a game" => &["{n} điểm trong trò chơi"],
        // settings.rs
        // sound.rs
        // stickers.rs
        // tg.rs
        // theme.rs
        // ui/chat_list.rs
        // ui/help.rs
        // ui/info.rs
        "{total} members" => &["{total} thành viên"],
        "{total} subscribers" => &["{total} người đăng ký"],
        "{total} groups" => &["{total} nhóm"],
        // ui/messages.rs
        "last seen {n} minutes ago" => &["truy cập {n} phút trước"],
        "{n} pinned" => &["{n} tin nhắn đã ghim"],
        // ui/mod.rs
        "{n} people are {doing}…" => &["{n} người {doing}…"],
        "  topics {n} columns" => &["  chủ đề: {n} cột"],
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
pub(super) const WEEKDAYS: [&str; 7] = ["T2", "T3", "T4", "T5", "T6", "T7", "CN"];
