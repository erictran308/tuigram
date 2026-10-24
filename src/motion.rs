//! Short animations, while `Settings.animations` is on: lights drawing the
//! border of the pane with the keys when the window gets focus, popups
//! fading in, new messages from others glowing as they come, and the toast
//! sliding in and out. Here is when each started; `ui::motion` draws them,
//! and the run loop wakes for their frames.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

/// How often what moves moves on: about 60 times a second.
pub const FRAME: Duration = Duration::from_millis(16);
/// How often a new message's glow moves on, when nothing else moves: a
/// slow change of color needs fewer frames, and a busy group would
/// otherwise keep the app drawing 60 times a second.
pub const GLOW_FRAME: Duration = Duration::from_millis(50);
/// How long the lights take to meet.
pub const LIGHTS: Duration = Duration::from_millis(1000);
/// How long a popup takes to fade in.
pub const POPUP: Duration = Duration::from_millis(150);
/// How long a new message glows.
pub const GLOW: Duration = Duration::from_millis(1500);
/// How long the toast takes to slide in, and out.
pub const SLIDE: Duration = Duration::from_millis(200);
/// New messages glowing at most at once, so a busy group can't grow the
/// list: the oldest stop first.
const MAX_GLOWING: usize = 32;

#[derive(Debug, Default)]
pub struct Motion {
    /// When the window last got focus, while the lights run.
    pub focused: Option<Instant>,
    /// When the popup up now came up, while it fades in.
    popup: Option<Instant>,
    /// A popup was up in the last frame drawn.
    popup_was_up: bool,
    /// Messages from others that just came into the open chat, by chat and
    /// message id, and when, while they glow.
    arrived: VecDeque<(i64, i64, Instant)>,
}

/// How far into `length` something that started `at` is, while it runs.
fn running(at: Instant, length: Duration, now: Instant) -> Option<Duration> {
    let elapsed = now.saturating_duration_since(at);
    (elapsed < length).then_some(elapsed)
}

impl Motion {
    /// How far the lights are into their run, while they run.
    pub fn lights(&self, now: Instant) -> Option<Duration> {
        self.focused.and_then(|at| running(at, LIGHTS, now))
    }

    /// Told each frame whether a popup is up, says how far it is into
    /// fading in, while it fades: from when it came up, so one replacing
    /// another doesn't fade again. One that comes up while it isn't to
    /// `animate` never fades, nor wakes the run loop for frames.
    pub fn popup(&mut self, up: bool, animate: bool, now: Instant) -> Option<Duration> {
        if up && !self.popup_was_up && animate {
            self.popup = Some(now);
        }
        self.popup_was_up = up;
        // Gone before it faded in: nothing left to draw frames for.
        if !up {
            self.popup = None;
        }
        self.popup
            .filter(|_| up && animate)
            .and_then(|at| running(at, POPUP, now))
    }

    /// Animations were turned off: the lights and glows stop now, rather
    /// than keep the run loop waking for frames nobody draws.
    pub fn stop(&mut self) {
        self.focused = None;
        self.popup = None;
        self.arrived.clear();
    }

    /// A message from someone else just came into the open chat.
    pub fn arrived(&mut self, chat_id: i64, message_id: i64, now: Instant) {
        if self.arrived.len() >= MAX_GLOWING {
            self.arrived.pop_front();
        }
        self.arrived.push_back((chat_id, message_id, now));
    }

    /// The messages of `chat_id` glowing, and how far into it each is.
    pub fn glowing(&self, chat_id: i64, now: Instant) -> Vec<(i64, Duration)> {
        self.arrived
            .iter()
            .filter(|&&(chat, _, _)| chat == chat_id)
            .filter_map(|&(_, id, at)| Some((id, running(at, GLOW, now)?)))
            .collect()
    }

    /// When to draw again, while anything moves. Each keeps going a frame
    /// past its length, so the last frame drawn shows it done; then it's
    /// dropped. Glows of chats other than the one `open` are dropped too:
    /// they're out of sight.
    pub fn next_frame(&mut self, now: Instant, open: Option<i64>) -> Option<Instant> {
        let moving = |at: Instant, length: Duration, frame: Duration| {
            running(at, length + frame, now).is_some()
        };
        self.focused = self.focused.filter(|&at| moving(at, LIGHTS, FRAME));
        self.popup = self.popup.filter(|&at| moving(at, POPUP, FRAME));
        self.arrived
            .retain(|&(chat, _, at)| Some(chat) == open && moving(at, GLOW, GLOW_FRAME));
        if self.focused.is_some() || self.popup.is_some() {
            Some(now + FRAME)
        } else {
            (!self.arrived.is_empty()).then_some(now + GLOW_FRAME)
        }
    }
}

/// How far the toast shown at `shown`, gone at `until`, is out of place to
/// the right, from 0 (in place) to 1 (all the way out): quick at first,
/// settling in, and the other way round on the way out.
pub fn toast_out(shown: Instant, until: Instant, now: Instant) -> f32 {
    let left = |d: Duration| 1.0 - (d.as_secs_f32() / SLIDE.as_secs_f32()).min(1.0);
    let coming = left(now.saturating_duration_since(shown));
    let going = left(until.saturating_duration_since(now));
    let out = coming.max(going);
    out * out
}

/// When to draw the toast again while it slides.
pub fn toast_frame(shown: Instant, until: Instant, now: Instant) -> Option<Instant> {
    let coming = running(shown, SLIDE + FRAME, now).is_some();
    let going = until.saturating_duration_since(now) < SLIDE;
    (coming || going).then_some(now + FRAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_popup_fades_in_once_when_it_comes_up() {
        let mut motion = Motion::default();
        let start = Instant::now();
        assert_eq!(motion.popup(false, true, start), None);
        assert_eq!(motion.popup(true, true, start), Some(Duration::ZERO));
        let later = start + POPUP / 2;
        assert_eq!(motion.popup(true, true, later), Some(POPUP / 2));
        assert_eq!(motion.popup(true, true, start + POPUP), None, "faded in");
        assert_eq!(
            motion.popup(true, true, start + POPUP * 3),
            None,
            "still up: another popup in its place doesn't fade again"
        );
        assert_eq!(motion.popup(false, true, start + POPUP * 4), None);
        let again = start + POPUP * 5;
        assert_eq!(
            motion.popup(true, true, again),
            Some(Duration::ZERO),
            "a new one"
        );
    }

    #[test]
    fn a_popup_that_isnt_animated_wakes_nothing() {
        let mut motion = Motion::default();
        let start = Instant::now();
        assert_eq!(motion.popup(true, false, start), None);
        assert_eq!(motion.next_frame(start, None), None, "no frames to draw");
        // Turned on while it's up: it came up before, so it doesn't fade.
        assert_eq!(motion.popup(true, true, start + FRAME), None);
        assert_eq!(motion.next_frame(start + FRAME, None), None);
    }

    #[test]
    fn frames_go_on_until_one_shows_everything_done() {
        let mut motion = Motion::default();
        let start = Instant::now();
        assert_eq!(motion.next_frame(start, Some(1)), None, "nothing moves");
        motion.focused = Some(start);
        motion.arrived(1, 10, start);
        assert_eq!(motion.next_frame(start, Some(1)), Some(start + FRAME));
        assert_eq!(motion.lights(start + LIGHTS / 2), Some(LIGHTS / 2));

        // The lights are done, but the frame showing it may not be drawn.
        let after_lights = start + LIGHTS;
        assert_eq!(motion.lights(after_lights), None);
        assert!(motion.next_frame(after_lights, Some(1)).is_some());
        assert!(motion.focused.is_some());
        let later = after_lights + FRAME;
        assert_eq!(
            motion.next_frame(later, Some(1)),
            Some(later + GLOW_FRAME),
            "only a glow left, which needs fewer frames"
        );
        assert_eq!(motion.focused, None);

        assert_eq!(motion.glowing(1, after_lights), [(10, LIGHTS)]);
        assert!(motion.glowing(2, after_lights).is_empty(), "another chat's");
        assert!(
            motion.next_frame(start + GLOW, Some(1)).is_some(),
            "its last frame"
        );
        assert_eq!(motion.next_frame(start + GLOW + GLOW_FRAME, Some(1)), None);
        assert!(motion.glowing(1, start).is_empty(), "dropped");
    }

    #[test]
    fn leaving_a_chat_stops_its_glows() {
        let mut motion = Motion::default();
        let start = Instant::now();
        motion.arrived(1, 10, start);
        assert!(motion.next_frame(start, Some(1)).is_some());
        assert_eq!(motion.next_frame(start, Some(2)), None, "another chat open");
        assert!(motion.glowing(1, start).is_empty());
    }

    #[test]
    fn a_busy_chat_keeps_only_the_newest_glowing() {
        let mut motion = Motion::default();
        let now = Instant::now();
        for id in 0..100 {
            motion.arrived(1, id, now);
        }
        let glowing = motion.glowing(1, now);
        assert_eq!(glowing.len(), MAX_GLOWING);
        assert_eq!(glowing[0].0, 100 - MAX_GLOWING as i64);
    }

    #[test]
    fn the_toast_slides_in_then_out() {
        let shown = Instant::now();
        let until = shown + Duration::from_secs(2);
        assert_eq!(toast_out(shown, until, shown), 1.0, "out of sight at first");
        let half = toast_out(shown, until, shown + SLIDE / 2);
        assert!(half > 0.0 && half < 0.5, "most of the way in: {half}");
        assert_eq!(toast_out(shown, until, shown + SLIDE), 0.0);
        assert_eq!(toast_out(shown, until, shown + Duration::from_secs(1)), 0.0);
        assert!(
            toast_out(shown, until, until - SLIDE / 2) > 0.0,
            "on its way out"
        );
        assert_eq!(toast_out(shown, until, until), 1.0);

        assert!(toast_frame(shown, until, shown).is_some());
        assert!(toast_frame(shown, until, shown + SLIDE + FRAME).is_none());
        assert!(toast_frame(shown, until, until - SLIDE / 2).is_some());
    }
}
