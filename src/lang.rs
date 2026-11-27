//! Interface languages: English, Russian and Vietnamese (`Settings.language`,
//! picked in `?`). Every word tuigram itself shows is written in English where
//! it's used, through [`t`], [`f`] or [`n`], and each other language has a
//! table of its own keyed by that English (`ru.rs`, `vi.rs`); a word a table
//! lacks shows in English. `tests::every_word_has_its_translation` checks the
//! tables against the code. What people wrote, their names, keys and commands,
//! and Telegram's own words stay as they are.

use std::fmt::Display;

use chrono::{Datelike, Weekday};

mod ru;
mod vi;

/// A language tuigram speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
    Vi,
}

impl Lang {
    /// Every one, in the order `?` lists them.
    pub const ALL: [Lang; 3] = [Lang::En, Lang::Ru, Lang::Vi];

    /// As `Settings.language` keeps it.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
            Lang::Vi => "vi",
        }
    }

    /// The one `code` names; English for anything else.
    pub fn of(code: &str) -> Lang {
        Lang::ALL
            .into_iter()
            .find(|lang| lang.code() == code)
            .unwrap_or(Lang::En)
    }

    /// Its name in itself, as `?` lists it whatever language is set.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
            Lang::Vi => "Tiếng Việt",
        }
    }

    fn text(self, en: &str) -> Option<&'static str> {
        match self {
            Lang::En => None,
            Lang::Ru => ru::text(en),
            Lang::Vi => vi::text(en),
        }
    }

    /// The forms of a count's words keyed by English's many form, one per
    /// [`Lang::form`].
    fn forms(self, en_many: &str) -> Option<&'static [&'static str]> {
        match self {
            Lang::En => None,
            Lang::Ru => ru::forms(en_many),
            Lang::Vi => vi::forms(en_many),
        }
    }

    /// Which of a count's forms `n` takes.
    fn form(self, n: u64) -> usize {
        match self {
            Lang::En => usize::from(n != 1),
            Lang::Ru => match (n % 10, n % 100) {
                (1, r) if r != 11 => 0,
                (2..=4, r) if !(12..=14).contains(&r) => 1,
                _ => 2,
            },
            // Vietnamese words don't change with a count.
            Lang::Vi => 0,
        }
    }
}

#[cfg(not(test))]
static CURRENT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

// Per thread in tests, which run side by side: one in Russian mustn't turn
// another's words.
#[cfg(test)]
thread_local! {
    static CURRENT: std::cell::Cell<Lang> = const { std::cell::Cell::new(Lang::En) };
}

/// Speak `lang` from now on. Set from the settings at startup and when the
/// language is picked in `?`.
pub fn set(lang: Lang) {
    #[cfg(not(test))]
    CURRENT.store(lang as u8, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    CURRENT.with(|c| c.set(lang));
}

/// The language set.
pub fn current() -> Lang {
    #[cfg(not(test))]
    return Lang::ALL[usize::from(CURRENT.load(std::sync::atomic::Ordering::Relaxed))];
    #[cfg(test)]
    CURRENT.with(std::cell::Cell::get)
}

/// `en` in the language set.
pub fn t(en: &'static str) -> &'static str {
    current().text(en).unwrap_or(en)
}

/// `en` where the same English needs other words in some language, `context`
/// saying which ("you": after «Вы»; "several": what several people do). A
/// table without `context|en` gives `en`'s own.
pub fn tc(context: &str, en: &'static str) -> &'static str {
    let lang = current();
    lang.text(&format!("{context}|{en}"))
        .or_else(|| lang.text(en))
        .unwrap_or(en)
}

/// `en` as it is, marked for the check: a word kept in English and put in
/// the language set later, with [`t`], where it's shown.
pub const fn key(en: &'static str) -> &'static str {
    en
}

/// `template` in the language set, its `{name}`s filled from `args`.
pub fn f(template: &'static str, args: &[(&str, &dyn Display)]) -> String {
    fill(t(template), args)
}

/// `count` of something, `one` or `many` in English (`{n}` is the count),
/// in the language set's form for it, its other `{name}`s from `args`.
pub fn n(
    count: i64,
    one: &'static str,
    many: &'static str,
    args: &[(&str, &dyn Display)],
) -> String {
    let lang = current();
    let form = lang.form(count.unsigned_abs());
    let template = match lang.forms(many) {
        Some(forms) => forms.get(form).or(forms.last()).copied().unwrap_or(many),
        None if count.unsigned_abs() == 1 => one,
        None => many,
    };
    let mut all: Vec<(&str, &dyn Display)> = vec![("n", &count)];
    all.extend_from_slice(args);
    fill(template, &all)
}

/// `template`, already in the language set (from [`t`] or [`tc`]), with each
/// `{name}` it has replaced by `args`' value, once: what's put in is never
/// read for more, so a name with braces stays as it is.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let value = after.find('}').and_then(|close| {
            let name = &after[..close];
            args.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| (v.to_string(), close))
        });
        match value {
            Some((value, close)) => {
                out.push_str(&value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

const MONTHS_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The day's short name: "Thu", "чт", "T5".
pub fn weekday(day: Weekday) -> &'static str {
    let i = day.num_days_from_monday() as usize;
    match current() {
        Lang::En => ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][i],
        Lang::Ru => ru::WEEKDAYS[i],
        Lang::Vi => vi::WEEKDAYS[i],
    }
}

/// The day and month: "8 Oct", "8 октября", "8 tháng 10".
pub fn day_month(date: &impl Datelike) -> String {
    let (day, month) = (date.day(), date.month0() as usize);
    match current() {
        Lang::En => format!("{day} {}", MONTHS_EN[month]),
        Lang::Ru => format!("{day} {}", ru::MONTHS[month]),
        Lang::Vi => format!("{day} tháng {}", month + 1),
    }
}

/// The day, month and year: "8 Oct 2026", "8 октября 2026", "8 tháng 10, 2026".
pub fn day_month_year(date: &impl Datelike) -> String {
    let sep = match current() {
        Lang::Vi => ", ",
        Lang::En | Lang::Ru => " ",
    };
    format!("{}{sep}{}", day_month(date), date.year())
}

/// The whole date, as a day's separator says it: "Thu 8 Oct 2026",
/// "чт, 8 октября 2026", "T5, 8 tháng 10, 2026".
pub fn full_date(date: &impl Datelike) -> String {
    let sep = match current() {
        Lang::En => " ",
        Lang::Ru | Lang::Vi => ", ",
    };
    format!("{}{sep}{}", weekday(date.weekday()), day_month_year(date))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use std::collections::BTreeSet;

    /// The string literal starting at `chars[at]` (its `"`): its text, with
    /// escapes and continued lines read, and where it ends.
    fn literal(chars: &[char], at: usize) -> (String, usize) {
        let (mut out, mut i) = (String::new(), at + 1);
        while i < chars.len() && chars[i] != '"' {
            if chars[i] != '\\' {
                out.push(chars[i]);
                i += 1;
                continue;
            }
            i += 1;
            match chars.get(i) {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let close = (i..chars.len()).find(|&j| chars[j] == '}').unwrap();
                    let hex: String = chars[i + 2..close].iter().collect();
                    out.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                    i = close;
                }
                // A line continued: the break and the next line's indent go.
                Some('\n') => {
                    while chars.get(i + 1).is_some_and(|c| c.is_whitespace()) {
                        i += 1;
                    }
                }
                Some(&c) => out.push(c),
                None => {}
            }
            i += 1;
        }
        (out, i + 1)
    }

    /// The first `take` string literals among the arguments of each call to
    /// `call` (which ends in its `(`) in `code`, of calls that have as many.
    fn arguments(code: &[char], call: &str, take: usize) -> Vec<Vec<String>> {
        let call: Vec<char> = call.chars().collect();
        let mut found = Vec::new();
        for start in 0..code.len().saturating_sub(call.len()) {
            if code[start..start + call.len()] != call[..] {
                continue;
            }
            let (mut i, mut depth, mut args) = (start + call.len(), 0, Vec::new());
            while i < code.len() && args.len() < take {
                match code[i] {
                    '"' if depth == 0 => {
                        let (text, end) = literal(code, i);
                        args.push(text);
                        i = end;
                        continue;
                    }
                    '"' => i = literal(code, i).1 - 1,
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' if depth == 0 => break,
                    ')' | ']' | '}' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if args.len() == take {
                found.push(args);
            }
        }
        found
    }

    /// A table's entries: each key with its text, or its forms.
    fn entries(table: &str) -> Vec<(String, Vec<String>)> {
        let code: Vec<char> = table
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .collect();
        let (mut found, mut i) = (Vec::new(), 0);
        while i < code.len() {
            if code[i] != '"' {
                i += 1;
                continue;
            }
            let (key, end) = literal(&code, i);
            let rest: String = code[end..].iter().take(8).collect();
            i = end;
            if !rest.trim_start().starts_with("=>") {
                continue;
            }
            let mut values = Vec::new();
            while i < code.len() && code[i] != ',' && code[i] != '[' {
                if code[i] == '"' {
                    let (text, end) = literal(&code, i);
                    values.push(text);
                    i = end;
                    break;
                }
                i += 1;
            }
            if code.get(i) == Some(&'[') {
                while i < code.len() && code[i] != ']' {
                    if code[i] == '"' {
                        let (text, end) = literal(&code, i);
                        values.push(text);
                        i = end;
                    } else {
                        i += 1;
                    }
                }
            }
            found.push((key, values));
        }
        found
    }

    /// `{name}`s in `text`.
    fn holes(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .filter_map(|s| s.split_once('}').map(|(name, _)| name))
            .filter(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            })
            .collect()
    }

    #[test]
    fn every_word_has_its_translation() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut paths = vec![src.clone()];
        // Each word, and the file it's first used in, to say where.
        let mut words: std::collections::BTreeMap<String, String> = Default::default();
        let mut in_context = BTreeSet::new();
        let mut counts: Vec<(String, String, String)> = Vec::new();
        while let Some(path) = paths.pop() {
            if path.is_dir() {
                paths.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs")
                || path.starts_with(src.join("lang"))
                || path == src.join("lang.rs")
            {
                continue;
            }
            let file = path.strip_prefix(&src).unwrap().display().to_string();
            let code: Vec<char> = std::fs::read_to_string(&path).unwrap().chars().collect();
            let mut found = |word: &String| {
                words.entry(word.clone()).or_insert_with(|| file.clone());
            };
            // t, f and key take the English itself, tc a context first, n
            // the one and the many forms of a count.
            for call in ["lang::t(", "lang::f(", "lang::key("] {
                arguments(&code, call, 1)
                    .iter()
                    .for_each(|args| found(&args[0]));
            }
            for args in arguments(&code, "lang::tc(", 2) {
                in_context.insert(format!("{}|{}", args[0], args[1]));
                found(&args[1]);
            }
            for args in arguments(&code, "lang::n(", 2) {
                counts.push((args[0].clone(), args[1].clone(), file.clone()));
            }
        }
        assert!(
            words.len() > 500,
            "the scan finds the words: {}",
            words.len()
        );

        let mut problems = Vec::new();
        for (lang, file, forms) in [(Lang::Ru, "ru.rs", 3), (Lang::Vi, "vi.rs", 1)] {
            let table = std::fs::read_to_string(src.join("lang").join(file)).unwrap();
            let mut keys = BTreeSet::new();
            for (key, values) in entries(&table) {
                let english = key.split_once('|').map_or(key.as_str(), |(_, en)| en);
                for v in &values {
                    if holes(v) != holes(english) {
                        problems.push(format!("{file}: {key:?} has other {{names}} in {v:?}"));
                    }
                    if lang == Lang::Vi && v.chars().any(|c| ('\u{300}'..='\u{36f}').contains(&c)) {
                        problems.push(format!(
                            "{file}: {v:?} has combining accents, not composed letters"
                        ));
                    }
                }
                if !keys.insert(key.clone()) {
                    problems.push(format!("{file}: {key:?} twice"));
                }
            }
            for (word, used_in) in &words {
                if lang.text(word).is_none() {
                    problems.push(format!("{file}: no {word:?} (in {used_in})"));
                }
            }
            for (one, many, used_in) in &counts {
                match lang.forms(many) {
                    None => problems.push(format!("{file}: no forms for {many:?} (in {used_in})")),
                    Some(f) if f.len() != forms => {
                        problems.push(format!("{file}: {many:?} needs {forms} forms"))
                    }
                    Some(_) if holes(one) != holes(many) => {
                        problems.push(format!("{file}: {one:?} and {many:?} have other {{names}}"))
                    }
                    Some(_) => {}
                }
            }
            let used: BTreeSet<&String> = words
                .keys()
                .chain(&in_context)
                .chain(counts.iter().map(|(_, many, _)| many))
                .collect();
            for key in keys.iter().filter(|key| !used.contains(key)) {
                problems.push(format!("{file}: {key:?} isn't used any more"));
            }
        }
        assert!(
            problems.is_empty(),
            "{} problems:\n{}",
            problems.len(),
            problems.join("\n")
        );
    }

    #[test]
    fn the_check_reads_calls_and_tables_as_rust_writes_them() {
        let code: Vec<char> =
            r#"lang::t("a \"b\"") lang::n(x.len(), "{n} cat", "{n} cats", &[]) lang::t(KEY)"#
                .chars()
                .collect();
        assert_eq!(arguments(&code, "lang::t(", 1), [["a \"b\""]]);
        assert_eq!(arguments(&code, "lang::n(", 2), [["{n} cat", "{n} cats"]]);
        let table = "// \"x\" => no\n        \"a\" => \"b\",\n        \"{n} cats\" => &[\"{n} к\", \"{n} кк\"],\n";
        assert_eq!(
            entries(table),
            [
                ("a".to_string(), vec!["b".to_string()]),
                (
                    "{n} cats".to_string(),
                    vec!["{n} к".to_string(), "{n} кк".to_string()]
                ),
            ]
        );
        assert_eq!(
            holes("{n} of {total}, {Bad} {}"),
            BTreeSet::from(["n", "total"])
        );
    }

    #[test]
    fn russian_counts_take_the_form_for_their_number() {
        for (n, want) in [
            (0, 2),
            (1, 0),
            (2, 1),
            (4, 1),
            (5, 2),
            (11, 2),
            (12, 2),
            (21, 0),
            (22, 1),
            (25, 2),
            (101, 0),
            (111, 2),
            (114, 2),
        ] {
            assert_eq!(Lang::Ru.form(n), want, "{n}");
        }
        assert_eq!(Lang::En.form(1), 0);
        assert_eq!(Lang::En.form(2), 1);
        assert_eq!(Lang::Vi.form(5), 0);
    }

    #[test]
    fn dates_read_as_each_language_writes_them() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
        assert_eq!(full_date(&day), day.format("%a %-d %b %Y").to_string());
        assert_eq!(day_month(&day), day.format("%-d %b").to_string());
        assert_eq!(day_month_year(&day), day.format("%-d %b %Y").to_string());
        set(Lang::Ru);
        assert_eq!(full_date(&day), "чт, 8 октября 2026");
        assert_eq!(day_month(&day), "8 октября");
        set(Lang::Vi);
        assert_eq!(full_date(&day), "T5, 8 tháng 10, 2026");
        assert_eq!(day_month(&day), "8 tháng 10");
        set(Lang::En);
    }

    #[test]
    fn a_template_is_filled_once_and_unknown_braces_stay() {
        let name = "{why}";
        assert_eq!(
            fill("{name} said {why}", &[("name", &name), ("why", &"hi")]),
            "{why} said hi",
            "a name's braces aren't read"
        );
        assert_eq!(fill("a {b} c", &[]), "a {b} c");
        assert_eq!(fill("{n} {", &[("n", &3)]), "3 {");
    }

    #[test]
    fn languages_are_kept_by_their_code_and_listed_by_their_own_name() {
        for lang in Lang::ALL {
            assert_eq!(Lang::of(lang.code()), lang);
        }
        assert_eq!(Lang::of("xx"), Lang::En);
        assert_eq!(Lang::Vi.name(), "Tiếng Việt");
    }
}
