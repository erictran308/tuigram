//! Polls: drawn as text in their bubble, with how people voted once you
//! have (as in Telegram), and Enter to vote.

use tdlib_rs::enums::PollType;
use tdlib_rs::types;

use crate::messages::without_spoilers;
use crate::text;

/// Cells of the bar that shows an answer's share of the votes.
const BAR: usize = 10;

/// A poll, as its message shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Poll {
    pub question: String,
    pub answers: Vec<Answer>,
    pub voters: i32,
    /// Voters can pick more than one answer.
    pub several: bool,
    /// A quiz has one right answer.
    pub quiz: bool,
    /// The right answer of a quiz, once Telegram says: after you answered,
    /// or once it's closed.
    pub correct: Option<usize>,
    pub anonymous: bool,
    pub closed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    pub voters: i32,
    pub percent: i32,
    /// You picked it.
    pub chosen: bool,
}

impl Poll {
    pub fn new(poll: &types::Poll) -> Self {
        let line = |text: &types::FormattedText| {
            text::clean(&without_spoilers(text))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (several, quiz, correct) = match &poll.r#type {
            PollType::Regular(r) => (r.allow_multiple_answers, false, None),
            PollType::Quiz(q) => (false, true, usize::try_from(q.correct_option_id).ok()),
        };
        Poll {
            question: line(&poll.question),
            answers: poll
                .options
                .iter()
                .map(|o| Answer {
                    text: line(&o.text),
                    voters: o.voter_count,
                    percent: o.vote_percentage.clamp(0, 100),
                    chosen: o.is_chosen,
                })
                .collect(),
            voters: poll.total_voter_count,
            several,
            quiz,
            correct,
            anonymous: poll.is_anonymous,
            closed: poll.is_closed,
        }
    }

    pub fn voted(&self) -> bool {
        self.answers.iter().any(|a| a.chosen)
    }

    /// Why Enter can't vote in it, if it can't.
    pub fn cant_vote(&self) -> Option<&'static str> {
        if self.closed {
            Some("This poll is closed")
        } else if self.quiz && self.voted() {
            Some("You answered this quiz")
        } else {
            None
        }
    }

    /// The poll as its bubble's text: the question, what kind of poll it
    /// is, then the answers, with how they're doing once you voted or it
    /// closed, as Telegram shows them.
    pub fn text(&self) -> String {
        let kind = match (self.anonymous, self.quiz) {
            (true, true) => "Anonymous quiz",
            (true, false) => "Anonymous poll",
            (false, true) => "Quiz",
            (false, false) => "Poll",
        };
        let votes = match self.voters {
            1 => "1 vote".to_string(),
            n => format!("{n} votes"),
        };
        let mut about = vec![kind.to_string()];
        if self.several {
            about.push("pick several".into());
        }
        about.push(votes);
        if self.closed {
            about.push("closed".into());
        }
        let mut lines = vec![format!("📊 {}", self.question), about.join(" · ")];
        let results = self.voted() || self.closed;
        for (i, answer) in self.answers.iter().enumerate() {
            let mark = match () {
                _ if results && self.correct == Some(i) => "✓",
                _ if answer.chosen && self.quiz => "✗",
                _ if answer.chosen => "●",
                _ => "○",
            };
            lines.push(format!("{mark} {}", answer.text));
            if results {
                let filled = (answer.percent as usize * BAR + 50) / 100;
                let bar = format!("{}{}", "█".repeat(filled), "░".repeat(BAR - filled));
                lines.push(format!("  {bar} {}%", answer.percent));
            }
        }
        lines.join("\n")
    }
}

/// Enter on a poll: its answers to vote for. With several allowed, Space
/// ticks them and Enter sends the ticked ones.
pub struct VoteMenu {
    pub message_id: i64,
    pub question: String,
    pub answers: Vec<String>,
    pub ticked: Vec<bool>,
    pub several: bool,
    /// You voted, so a last row takes it back.
    pub can_retract: bool,
    pub selected: usize,
}

/// What Enter in the vote popup does.
#[derive(Debug, PartialEq, Eq)]
pub enum Vote {
    /// Vote for these answers, by index.
    For(Vec<i32>),
    Retract,
}

impl VoteMenu {
    pub fn new(message_id: i64, poll: &Poll) -> Self {
        Self {
            message_id,
            question: poll.question.clone(),
            answers: poll.answers.iter().map(|a| a.text.clone()).collect(),
            ticked: poll.answers.iter().map(|a| a.chosen).collect(),
            several: poll.several,
            can_retract: poll.voted(),
            // On your answer, so it's clear what you voted for.
            selected: poll.answers.iter().position(|a| a.chosen).unwrap_or(0),
        }
    }

    /// Rows: the answers, then "take back" if you voted.
    pub fn rows(&self) -> usize {
        self.answers.len() + usize::from(self.can_retract)
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.rows().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Space: ticks or unticks the answer under the cursor, in a poll that
    /// takes several.
    pub fn tick(&mut self) {
        if self.several
            && let Some(ticked) = self.ticked.get_mut(self.selected)
        {
            *ticked = !*ticked;
        }
    }

    /// Enter: the answer under the cursor, or the ticked ones in a poll
    /// that takes several (the one under the cursor if none are).
    pub fn vote(&self) -> Vote {
        if self.selected >= self.answers.len() {
            return Vote::Retract;
        }
        let ticked: Vec<i32> = (0..self.answers.len())
            .filter(|&i| self.ticked[i])
            .map(|i| i as i32)
            .collect();
        if self.several && !ticked.is_empty() {
            Vote::For(ticked)
        } else {
            Vote::For(vec![self.selected as i32])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poll(chosen: Option<usize>, closed: bool) -> Poll {
        let answer = |text: &str, voters, percent, i| Answer {
            text: text.into(),
            voters,
            percent,
            chosen: chosen == Some(i),
        };
        Poll {
            question: "Lunch?".into(),
            answers: vec![answer("Pizza", 5, 62, 0), answer("Sushi", 3, 38, 1)],
            voters: 8,
            several: false,
            quiz: false,
            correct: None,
            anonymous: true,
            closed,
        }
    }

    #[test]
    fn results_show_once_you_voted_or_the_poll_closed() {
        assert_eq!(
            poll(None, false).text(),
            "📊 Lunch?\nAnonymous poll · 8 votes\n○ Pizza\n○ Sushi"
        );
        assert_eq!(
            poll(Some(1), false).text(),
            "📊 Lunch?\nAnonymous poll · 8 votes\n○ Pizza\n  ██████░░░░ 62%\n● Sushi\n  ████░░░░░░ 38%"
        );
        assert!(
            poll(None, true)
                .text()
                .contains("closed\n○ Pizza\n  ██████")
        );
    }

    #[test]
    fn a_quiz_marks_the_right_answer_and_yours_if_wrong() {
        let mut quiz = poll(Some(1), false);
        quiz.quiz = true;
        quiz.correct = Some(0);
        let text = quiz.text();
        assert!(
            text.contains("✓ Pizza") && text.contains("✗ Sushi"),
            "{text}"
        );
        assert_eq!(quiz.cant_vote(), Some("You answered this quiz"));
        assert_eq!(poll(None, true).cant_vote(), Some("This poll is closed"));
        assert_eq!(poll(Some(0), false).cant_vote(), None, "a vote can change");
    }

    #[test]
    fn the_vote_popup_picks_one_ticks_several_or_takes_yours_back() {
        let mut menu = VoteMenu::new(1, &poll(None, false));
        assert_eq!(menu.rows(), 2);
        menu.move_by(1);
        assert_eq!(menu.vote(), Vote::For(vec![1]));
        menu.tick();
        assert_eq!(
            menu.vote(),
            Vote::For(vec![1]),
            "nothing to tick in a one-answer poll"
        );

        let mut several = poll(Some(0), false);
        several.several = true;
        let mut menu = VoteMenu::new(1, &several);
        assert_eq!(menu.selected, 0, "on your answer");
        assert_eq!(menu.rows(), 3, "with take back");
        menu.move_by(1);
        menu.tick();
        assert_eq!(menu.vote(), Vote::For(vec![0, 1]));
        menu.move_by(5);
        assert_eq!(menu.vote(), Vote::Retract);
    }
}
