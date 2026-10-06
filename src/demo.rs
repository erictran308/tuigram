//! `tuigram --demo`: the real UI filled with made-up chats, for screenshots
//! and for a look around before logging in. TDLib never starts, so nothing
//! is read from or sent to Telegram, and the data directory isn't touched.
//! The photos are drawn here and written to a temporary folder, removed on
//! exit.
//!
//! Keys: 1–6 or Tab / Shift-Tab switch scenes, t / T change the theme, q
//! quits.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use chrono::{Local, TimeZone};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use image::{Rgb, RgbImage};
use ratatui_image::picker::Picker;
use tdlib_rs::enums::{ChatAction, MessageSender};
use tdlib_rs::types::MessageSenderUser;
use tokio::sync::mpsc::unbounded_channel;

use crate::app::{self, App, Focus, HelpTab, Screen, SettingsMenu};
use crate::chats::{Chat, ChatPhoto, Chats};
use crate::clipboard::Clipboard;
use crate::images::Images;
use crate::messages::{
    Editable, Link, Msg, OpenChat, Preview, Replied, ReplyTo, SendState, Sender,
};
use crate::reactions::{ReactMenu, Reaction, ReactionKind};
use crate::search::MessageSearch;
use crate::settings::Settings;
use crate::tg::Tg;
use crate::ui;

// People, by user id. A private chat has the other person's id, and Saved
// Messages has yours.
const ME: i64 = 1;
const MAYA: i64 = 2;
const LEO: i64 = 3;
const PRIYA: i64 = 4;
const ALEX: i64 = 5;
const MOM: i64 = 6;
const DAD: i64 = 7;

// Group and channel chats.
const HIKE: i64 = -101;
const TERMINAL_WEEKLY: i64 = -102;
const RUSTACEANS: i64 = -103;
const TOKYO: i64 = -104;
const BOOK_CLUB: i64 = -105;
const DESIGN: i64 = -106;

// Photos, by file id.
const SUNRISE: i32 = 1;
const HIKE_PHOTO: i32 = 2;
const TOKYO_PHOTO: i32 = 3;
const ALEX_PHOTO: i32 = 4;
const MOM_PHOTO: i32 = 5;

/// The sunrise photo's size, for its shape on screen.
const SUNRISE_SIZE: (u32, u32) = (1280, 853);

// Messages in the open chat, by id.
const SUNNY: i64 = 1;
const DRIVE: i64 = 5;
const PICK_UP: i64 = 7;
const TRAILHEAD: i64 = 6;
const TRAIL_LINK: i64 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    /// Reading the open chat in Normal mode.
    Reading,
    /// Insert mode, answering a message.
    Replying,
    /// The `R` popup, picking a reaction.
    Reacting,
    /// A `/` search through the chat, matches highlighted.
    Searching,
    /// The `?` popup on its settings tab.
    Settings,
    /// The `?` popup on its shortcuts tab.
    Shortcuts,
}

const SCENES: [Scene; 6] = [
    Scene::Reading,
    Scene::Replying,
    Scene::Reacting,
    Scene::Searching,
    Scene::Settings,
    Scene::Shortcuts,
];

pub async fn run() -> Result<()> {
    let dir = new_private_dir()?;
    let result = show(&dir).await;
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// A folder of the demo's own in the temporary folder: made new, never one
/// that's already there, and readable only by the user. On a shared `/tmp`,
/// a folder someone else made in advance could hold links that the photos
/// would be written through, onto the user's files.
fn new_private_dir() -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for attempt in 0..100 {
        let name = format!("tuigram-demo-{}-{nanos}-{attempt}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
                }
                return Ok(dir);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("can't make a folder in {}", std::env::temp_dir().display())
}

async fn show(dir: &Path) -> Result<()> {
    // Before the terminal is taken over, so an error prints as normal text.
    let mut photos = Vec::new();
    for (file_id, image) in draw_photos() {
        let path = dir.join(format!("{file_id}.png"));
        image.save(&path)?;
        photos.push((file_id, path.to_string_lossy().into_owned()));
    }

    let mut terminal = ratatui::init();
    crate::tmux::save();
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let (image_tx, mut image_rx) = unbounded_channel();
    let mut images = Images::new(picker, image_tx);
    // Every photo is on disk already, so nothing asks TDLib for a download.
    for (file_id, path) in photos {
        images.on_downloaded(file_id, Some(path));
    }
    let tg = Tg::detached(unbounded_channel().0);
    let mut app = demo_app(tg.clone(), images, dir);
    let mut scene = 0;
    show_scene(&mut app, SCENES[scene]);

    let mut keys = EventStream::new();
    let result = loop {
        if let Err(e) = terminal.draw(|frame| ui::draw(frame, &mut app)) {
            break Err(e.into());
        }
        app.images.fetch(&tg);
        tokio::select! {
            Some(event) = image_rx.recv() => app.images.on_built(event),
            Some(event) = keys.next() => match event {
                Ok(Event::Key(key)) => {
                    if !on_key(&mut app, &mut scene, key) {
                        break Ok(());
                    }
                }
                Ok(_) => {}
                Err(e) => break Err(e.into()),
            },
            else => break Ok(()),
        }
    };
    ratatui::restore();
    crate::tmux::restore();
    result
}

/// The app with the made-up chats, on the main screen.
/// Also for tests elsewhere that need a whole app; it makes no TDLib requests.
pub(crate) fn demo_app(tg: Tg, images: Images, dir: &Path) -> App {
    let (clipboard_tx, _) = unbounded_channel();
    let settings = Settings::default();
    // Settings are never saved: the demo's popup has no Enter.
    let mut app = App::new(
        tg,
        images,
        // The demo never pastes, so nothing is saved there.
        Clipboard::new(clipboard_tx, dir.join("outbox")),
        settings,
        dir.join("settings.toml"),
        None,
    );
    app.screen = Screen::Main;
    for (id, name) in [
        (ME, "Sam Taylor"),
        (MAYA, "Maya Chen"),
        (LEO, "Leo Park"),
        (PRIYA, "Priya Nair"),
        (ALEX, "Alex Rivera"),
    ] {
        app.users.insert(id, name.into());
    }
    fill_chats(&mut app.chats);
    app.selected = Some(HIKE);
    app.open = Some(hike());
    app
}

/// Returns false to quit.
fn on_key(app: &mut App, scene: &mut usize, key: KeyEvent) -> bool {
    if key.kind != KeyEventKind::Press {
        return true;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let next =
        |i: usize, step: isize| (i as isize + step).rem_euclid(SCENES.len() as isize) as usize;
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return false,
        KeyCode::Char('c') if ctrl => return false,
        KeyCode::Char(c @ '1'..='6') => *scene = c as usize - '1' as usize,
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => *scene = next(*scene, 1),
        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => *scene = next(*scene, -1),
        KeyCode::Char('t') => change_theme(app, 1),
        KeyCode::Char('T') => change_theme(app, -1),
        _ => return true,
    }
    show_scene(app, SCENES[*scene]);
    true
}

fn change_theme(app: &mut App, step: isize) {
    let themes = &app.themes.list;
    let at = themes
        .iter()
        .position(|t| t.id == app.settings.theme)
        .unwrap_or(0);
    let next = &themes[(at as isize + step).rem_euclid(themes.len() as isize) as usize];
    if let Ok(colors) = &next.colors {
        app.colors = *colors;
    }
    app.settings.theme = next.id.clone();
}

fn show_scene(app: &mut App, scene: Scene) {
    app.focus = Focus::Messages;
    app.composer = app::new_composer();
    app.settings_menu = match scene {
        Scene::Settings => Some(help(app, HelpTab::Settings)),
        Scene::Shortcuts => Some(help(app, HelpTab::Shortcuts)),
        _ => None,
    };
    app.react_menu = None;
    let Some(open) = app.open.as_mut() else {
        return;
    };
    open.selected = None;
    open.scroll = None;
    open.reply = None;
    open.search = None;
    match scene {
        Scene::Replying => {
            open.reply = open
                .messages
                .get(&PICK_UP)
                .map(|msg| Replied::new(PICK_UP, msg));
            app.focus = Focus::Input;
            app.composer.insert_str("Yes! I'll be outside at 6:55");
        }
        Scene::Reacting => {
            open.selected = Some(SUNNY);
            let mut menu =
                ReactMenu::new(SUNNY, "Sunny all weekend! Eagle Ridge on Saturday?".into());
            let emoji = TELEGRAM_REACTIONS.iter().map(|e| e.to_string()).collect();
            menu.set_choices(emoji, &["👍".into()]);
            menu.selected = 3;
            app.react_menu = Some(menu);
        }
        Scene::Searching => {
            let mut search = MessageSearch::new("trail".into());
            search.results = vec![TRAILHEAD, TRAIL_LINK];
            search.current = Some(0);
            search.done = true;
            open.search = Some(search);
            open.selected = Some(TRAILHEAD);
        }
        Scene::Reading | Scene::Settings | Scene::Shortcuts => {}
    }
}

/// The popup, with the cursor on the theme in use.
fn help(app: &App, tab: HelpTab) -> SettingsMenu {
    let theme = app
        .themes
        .list
        .iter()
        .position(|t| t.id == app.settings.theme)
        .unwrap_or(0);
    SettingsMenu {
        selected: SettingsMenu::THEMES + theme,
        ..SettingsMenu::new(tab, &app.settings)
    }
}

fn user(user_id: i64) -> MessageSender {
    MessageSender::User(MessageSenderUser { user_id })
}

fn add<'a>(
    chats: &'a mut Chats,
    id: i64,
    title: &str,
    photo: Option<ChatPhoto>,
    preview: &str,
) -> &'a mut Chat {
    let chat = chats.add_local(id, title, photo);
    chat.preview = preview.into();
    chat
}

fn photo(file_id: i32) -> Option<ChatPhoto> {
    Some(ChatPhoto {
        file_id,
        path: None,
        thumbnail: None,
    })
}

/// The chat list, newest first. Unread chats move to the top.
fn fill_chats(chats: &mut Chats) {
    chats.set_my_id(ME);
    // Everyone has read up to the snacks; "On my way!" was just sent.
    add(
        chats,
        HIKE,
        "Weekend Hike",
        photo(HIKE_PHOTO),
        "You: On my way!",
    )
    .read_outbox = 8;
    let alex = add(
        chats,
        ALEX,
        "Alex Rivera",
        photo(ALEX_PHOTO),
        "Did you try the new release?",
    );
    alex.is_private = true;
    alex.unread = 2;
    let news = add(
        chats,
        TERMINAL_WEEKLY,
        "Terminal Weekly",
        None,
        "10 TUI apps worth trying this fall",
    );
    news.is_channel = true;
    news.unread = 48;
    add(chats, MOM, "Mom", photo(MOM_PHOTO), "Call me when you land").is_private = true;
    let rust = add(
        chats,
        RUSTACEANS,
        "Rustaceans",
        None,
        "nextest cut our CI time in half",
    );
    rust.unread = 12;
    add(
        chats,
        ME,
        "Saved Messages",
        None,
        "You: Boarding pass, gate B12",
    )
    .is_private = true;
    add(
        chats,
        TOKYO,
        "Tokyo Trip",
        photo(TOKYO_PHOTO),
        "Booked the ryokan for three nights",
    );
    add(
        chats,
        BOOK_CLUB,
        "Book Club",
        None,
        "Chapter 7 for next week",
    );
    add(
        chats,
        DESIGN,
        "Design Team",
        None,
        "Pushed the new icon set",
    );
    add(chats, DAD, "Dad", None, "You: Happy birthday!").is_private = true;

    // Telegram's color ids: 1 orange, 2 violet, 3 green, 5 blue.
    for (id, accent) in [
        (TERMINAL_WEEKLY, 3),
        (RUSTACEANS, 1),
        (BOOK_CLUB, 5),
        (DESIGN, 2),
        (DAD, 5),
    ] {
        chats.set_accent(id, accent);
    }
    chats.set_action(ALEX, &user(ALEX), &ChatAction::Typing);
    chats.set_action(HIKE, &user(MAYA), &ChatAction::Typing);
    chats.set_highlighted(&[MOM]);
    chats.opened(HIKE);
    chats.refresh();
}

/// The reactions Telegram offers in most chats, in its order.
const TELEGRAM_REACTIONS: &[&str] = &[
    "👍",
    "👎",
    "❤",
    "🔥",
    "🥰",
    "👏",
    "😁",
    "🤔",
    "🤯",
    "😱",
    "🤬",
    "😢",
    "🎉",
    "🤩",
    "🤮",
    "💩",
    "🙏",
    "👌",
    "🕊",
    "🤡",
    "🥱",
    "🥴",
    "😍",
    "🐳",
    "❤‍🔥",
    "🌚",
    "🌭",
    "💯",
    "🤣",
    "⚡",
    "🍌",
    "🏆",
    "💔",
    "🤨",
    "😐",
    "🍓",
    "🍾",
    "💋",
    "🖕",
    "😈",
    "😴",
    "😭",
    "🤓",
    "👻",
    "👨‍💻",
    "👀",
    "🎃",
    "🙈",
    "😇",
    "😨",
    "🤝",
    "✍",
    "🤗",
    "🫡",
    "🎅",
    "🎄",
    "☃",
    "💅",
    "🤪",
    "🗿",
    "🆒",
    "💘",
    "🙉",
    "🦄",
    "😘",
    "💊",
    "🙊",
    "😎",
    "👾",
    "🤷‍♂",
    "🤷",
    "🤷‍♀",
    "😡",
];

/// Reactions on a demo message: (emoji, count, added by you).
fn reactions(list: &[(&str, i32, bool)]) -> Vec<Reaction> {
    list.iter()
        .map(|&(emoji, count, chosen)| Reaction {
            kind: ReactionKind::Emoji(emoji.into()),
            count,
            chosen,
        })
        .collect()
}

/// The open chat: planning a hike over two days.
fn hike() -> OpenChat {
    let at = |day, hour, min| {
        Local
            .with_ymd_and_hms(2026, 10, day, hour, min, 0)
            .single()
            .map_or(0, |t| t.timestamp() as i32)
    };
    let msg = |sender, date, text: &str| Msg {
        sender: Sender::User(sender),
        outgoing: sender == ME,
        date,
        text: text.into(),
        source_text: text.into(),
        preview: None,
        file: None,
        links: Vec::new(),
        link_ranges: Vec::new(),
        styles: Vec::new(),
        revealed: false,
        forwarded: None,
        state: SendState::Sent,
        reply_to: None,
        editable: Editable::Text,
        formatted: false,
        edited: false,
        album: 0,
        reactions: Vec::new(),
    };
    let url = "https://trails.example.com/eagle-ridge";
    let link = msg(LEO, at(2, 19, 5), &format!("Here's the trail: {url}"));
    let start = link.text.len() - url.len();
    let link = Msg {
        links: vec![Link::from(url)],
        link_ranges: std::iter::once(start..link.text.len()).collect(),
        ..link
    };
    let sunrise = Msg {
        preview: Some(Preview {
            file_id: SUNRISE,
            width: SUNRISE_SIZE.0,
            height: SUNRISE_SIZE.1,
            thumbnail: None,
            sticker: false,
        }),
        reactions: reactions(&[("❤", 3, true), ("😍", 1, false)]),
        ..msg(PRIYA, at(3, 7, 48), "Sunrise at the trailhead last year")
    };
    let pick_up = Msg {
        reply_to: Some(ReplyTo {
            message_id: Some(DRIVE),
            quote: None,
        }),
        ..msg(MAYA, at(3, 7, 52), "Perfect, can you pick me up at 7?")
    };

    let mut open = OpenChat::new(HIKE);
    open.all_loaded = true;
    let sunny = Msg {
        reactions: reactions(&[("👍", 3, true), ("🔥", 2, false)]),
        ..msg(
            MAYA,
            at(2, 19, 2),
            "Sunny all weekend! Eagle Ridge on Saturday?",
        )
    };
    let snacks = Msg {
        reactions: reactions(&[("🙏", 2, false)]),
        ..msg(LEO, at(3, 7, 53), "Bringing snacks and the good coffee")
    };
    open.messages.extend([
        (SUNNY, sunny),
        (2, msg(LEO, at(2, 19, 4), "I'm in")),
        (TRAIL_LINK, link),
        (4, msg(ME, at(2, 19, 11), "Count me in too")),
        (
            DRIVE,
            msg(ME, at(2, 19, 11), "I can drive, the car fits five"),
        ),
        (TRAILHEAD, sunrise),
        (PICK_UP, pick_up),
        (8, snacks),
        (9, msg(ME, at(3, 7, 55), "On my way!")),
    ]);
    open
}

type Color = [u8; 3];

/// Colors of a landscape: sky from top to horizon, the sun (or moon), and
/// mountain ridges from farthest to nearest.
struct Scenery {
    sky: [Color; 2],
    sun: Color,
    ridges: [Color; 3],
}

const DAWN: Scenery = Scenery {
    sky: [[38, 52, 104], [250, 176, 120]],
    sun: [255, 236, 186],
    ridges: [[150, 110, 140], [86, 66, 110], [38, 32, 62]],
};

const DAY: Scenery = Scenery {
    sky: [[70, 130, 220], [196, 226, 250]],
    sun: [255, 252, 230],
    ridges: [[120, 150, 186], [70, 112, 104], [34, 70, 58]],
};

const NIGHT: Scenery = Scenery {
    sky: [[10, 14, 40], [70, 52, 110]],
    sun: [236, 236, 250],
    ridges: [[52, 44, 88], [32, 26, 62], [14, 12, 30]],
};

/// The photos the demo shows, by file id.
fn draw_photos() -> Vec<(i32, RgbImage)> {
    let (width, height) = SUNRISE_SIZE;
    vec![
        (SUNRISE, landscape(width, height, &DAWN)),
        (HIKE_PHOTO, landscape(320, 320, &DAY)),
        (TOKYO_PHOTO, landscape(320, 320, &NIGHT)),
        (ALEX_PHOTO, person(320, [255, 150, 110], [190, 90, 200])),
        (MOM_PHOTO, person(320, [110, 210, 180], [60, 120, 210])),
    ]
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| {
        (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8
    })
}

/// Mountains under a sky that fades down to the horizon, with the sun (or
/// moon) glowing behind them. Nearer ridges are darker and lower.
fn landscape(width: u32, height: u32, scenery: &Scenery) -> RgbImage {
    let (w, h) = (width as f32, height as f32);
    let (sun_x, sun_y, radius) = (0.68 * w, 0.44 * h, 0.07 * w.min(h));
    // Top edge of ridge `i` at column `x`: a few sine waves, so each looks
    // like a mountain range rather than a wave.
    let ridge = |i: usize, x: f32| {
        let n = i as f32;
        let u = x / w;
        let shape = 0.6 * (u * (5.0 + 2.0 * n) + 1.3 * n).sin()
            + 0.3 * (u * (13.0 + 3.0 * n) + 0.7).sin()
            + 0.1 * (u * 31.0 + n).sin();
        h * (0.56 + 0.13 * n) - h * (0.12 - 0.025 * n) * shape
    };
    RgbImage::from_fn(width, height, |x, y| {
        let (x, y) = (x as f32, y as f32);
        for i in (0..3).rev() {
            let top = ridge(i, x);
            if y >= top {
                // Darker further down, like a slope in shadow.
                let shade = (y - top) / h * 0.6;
                return Rgb(mix(scenery.ridges[i], [0, 0, 0], shade));
            }
        }
        let sky = mix(scenery.sky[0], scenery.sky[1], y / (0.75 * h));
        let distance = ((x - sun_x).powi(2) + (y - sun_y).powi(2)).sqrt();
        if distance <= radius {
            return Rgb(scenery.sun);
        }
        let glow = 0.6 * (-(distance - radius) / (1.6 * radius)).exp();
        Rgb(mix(sky, scenery.sun, glow))
    })
}

/// A person's outline (head and shoulders) on a two-color gradient, standing
/// in for someone's photo.
fn person(size: u32, from: Color, to: Color) -> RgbImage {
    let s = size as f32;
    RgbImage::from_fn(size, size, |x, y| {
        let (u, v) = (x as f32 / s, y as f32 / s);
        let background = mix(from, to, (u + v) / 2.0);
        let head = ((u - 0.5).powi(2) + (v - 0.4).powi(2)).sqrt() < 0.17;
        let shoulders = ((u - 0.5) / 0.36).powi(2) + ((v - 0.98) / 0.34).powi(2) < 1.0;
        if head || shoulders {
            Rgb(mix(background, [255, 255, 255], 0.75))
        } else {
            Rgb(background)
        }
    })
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    #[test]
    fn the_demo_gets_a_new_folder_of_its_own() {
        let (a, b) = (new_private_dir().unwrap(), new_private_dir().unwrap());
        assert_ne!(a, b, "never one that's there already");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&a).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::remove_dir(&a).unwrap();
        std::fs::remove_dir(&b).unwrap();
    }

    fn rows(app: &mut App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| ui::draw(frame, app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn every_scene_draws_the_made_up_chats() {
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = demo_app(tg, images, &std::env::temp_dir());
        let has = |rows: &[String], text: &str| rows.iter().any(|r| r.contains(text));

        show_scene(&mut app, Scene::Reading);
        let screen = rows(&mut app);
        assert!(has(&screen, "Weekend Hike · Maya Chen is typing…"));
        assert!(has(&screen, "Alex Rivera"));
        assert!(has(&screen, "On my way!"));

        show_scene(&mut app, Scene::Replying);
        let screen = rows(&mut app);
        assert!(has(&screen, "INSERT"));
        assert!(has(&screen, "I'll be outside at 6:55"));

        show_scene(&mut app, Scene::Reacting);
        let screen = rows(&mut app);
        assert!(has(&screen, " React "));
        assert!(has(&screen, "fire  :fire:"));

        show_scene(&mut app, Scene::Searching);
        assert!(has(&rows(&mut app), "/trail 1 of 2"));

        show_scene(&mut app, Scene::Settings);
        assert!(has(&rows(&mut app), "Catppuccin Mocha"));
    }

    #[test]
    fn keys_switch_scenes_and_themes() {
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = demo_app(tg, images, &std::env::temp_dir());
        let mut scene = 0;
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);

        assert!(on_key(&mut app, &mut scene, press(KeyCode::Char('2'))));
        assert_eq!(SCENES[scene], Scene::Replying);
        assert!(on_key(&mut app, &mut scene, press(KeyCode::BackTab)));
        assert!(on_key(&mut app, &mut scene, press(KeyCode::BackTab)));
        assert_eq!(SCENES[scene], Scene::Shortcuts, "wraps around");

        let before = app.settings.theme.clone();
        on_key(&mut app, &mut scene, press(KeyCode::Char('t')));
        assert_ne!(app.settings.theme, before);
        assert!(!on_key(&mut app, &mut scene, press(KeyCode::Char('q'))));
    }

    #[test]
    fn photos_are_the_sizes_the_chats_expect() {
        let photos = draw_photos();
        let sunrise = &photos.iter().find(|(id, _)| *id == SUNRISE).unwrap().1;
        assert_eq!(sunrise.dimensions(), SUNRISE_SIZE);
        // The sky is lighter at the horizon than at the top.
        let top = sunrise.get_pixel(10, 0);
        let horizon = sunrise.get_pixel(10, 300);
        assert!(horizon[0] > top[0]);
    }
}
