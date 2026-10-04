//! This stream's timeline: what happened and when, as a position in the
//! stream (the VOD), not a clock time. Integrations add lines with the
//! Timeline step; `{timeline}` and `{chapters}` read it back.
//!
//! In memory only, and cleared when a stream starts: it describes one
//! stream, and a restart mid-stream simply starts a shorter one.

use super::host::fmt_clock;

/// Lines kept per stream. A 12-hour stream with a line a minute fits.
const MAX_ENTRIES: usize = 800;
/// YouTube ignores chapters shorter than this.
const MIN_CHAPTER_MS: u64 = 10_000;
/// YouTube only turns a description's timestamps into chapters when there
/// are at least this many.
const MIN_CHAPTERS: usize = 3;
/// What `{chapters}` calls the stream's opening, which YouTube requires.
const FIRST_CHAPTER: &str = "Start";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Note,
    Chapter,
    Highlight,
}

impl Kind {
    pub fn from_id(id: &str) -> Kind {
        match id {
            "chapter" => Kind::Chapter,
            "highlight" => Kind::Highlight,
            _ => Kind::Note,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Kind::Note => "note",
            Kind::Chapter => "chapter",
            Kind::Highlight => "highlight",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    at_ms: u64,
    kind: Kind,
    text: String,
}

#[derive(Default)]
pub struct Timeline {
    entries: Vec<Entry>,
}

impl Timeline {
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Add a line at `at_ms` into the stream. Returns it as `{timeline}`
    /// writes it. Past the cap the oldest plain line goes, so a flood of
    /// notes never pushes out the chapters and highlights.
    pub fn add(&mut self, at_ms: u64, kind: Kind, text: &str) -> String {
        if self.entries.len() >= MAX_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .position(|e| e.kind == Kind::Note)
                .unwrap_or(0);
            self.entries.remove(oldest);
        }
        let entry = Entry {
            at_ms,
            kind,
            text: one_line(text),
        };
        let line = format_line(&entry);
        // Lines can arrive out of order (one waited for the delay): keep
        // them in stream order.
        let at = self.entries.partition_point(|e| e.at_ms <= at_ms);
        self.entries.insert(at, entry);
        line
    }

    pub fn count(&self, kind: Kind) -> usize {
        self.entries.iter().filter(|e| e.kind == kind).count()
    }

    /// Every line, oldest first, within `max_chars`: when it doesn't fit,
    /// the oldest lines give way to a note saying how many.
    pub fn render(&self, max_chars: usize) -> String {
        let lines: Vec<String> = self.entries.iter().map(format_line).collect();
        let mut kept = 0;
        let mut used = 0;
        for line in lines.iter().rev() {
            let cost = line.chars().count() + 1;
            if used + cost > max_chars.saturating_sub(24) {
                break;
            }
            used += cost;
            kept += 1;
        }
        let skipped = lines.len() - kept;
        let mut out = Vec::with_capacity(kept + 1);
        if skipped > 0 {
            out.push(format!("… {skipped} earlier"));
        }
        out.extend(lines.into_iter().skip(skipped));
        out.join("\n")
    }

    /// The chapters, written the way YouTube reads them from a video's
    /// description: the first at 0:00, each at least 10 seconds long, at
    /// least three of them. Fewer is no chapter list at all: YouTube would
    /// ignore it.
    pub fn chapters(&self) -> String {
        let mut kept: Vec<(u64, &str)> = Vec::new();
        for e in self.entries.iter().filter(|e| e.kind == Kind::Chapter) {
            let at = if kept.is_empty() && e.at_ms < MIN_CHAPTER_MS {
                0
            } else {
                e.at_ms
            };
            match kept.last() {
                Some((last, _)) if at < last + MIN_CHAPTER_MS => continue,
                _ => kept.push((at, e.text.as_str())),
            }
        }
        if kept.first().is_some_and(|(at, _)| *at != 0) {
            kept.insert(0, (0, FIRST_CHAPTER));
        }
        if kept.len() < MIN_CHAPTERS {
            return String::new();
        }
        kept.iter()
            .map(|(at, text)| format!("{} {text}", fmt_clock(*at)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The chapter before the latest one, or the stream's opening: what a
    /// stream goes back to after a "Technical difficulties" chapter.
    pub fn previous_chapter(&self) -> String {
        let mut chapters = self
            .entries
            .iter()
            .rev()
            .filter(|e| e.kind == Kind::Chapter);
        chapters.next();
        chapters
            .next()
            .map_or_else(|| FIRST_CHAPTER.to_string(), |e| e.text.clone())
    }
}

fn format_line(e: &Entry) -> String {
    format!("`{}` {}", fmt_clock(e.at_ms), e.text)
}

/// A timeline line is one line: chat and web answers can hold newlines.
fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    flat.trim().chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_kept_in_stream_order() {
        let mut t = Timeline::default();
        assert_eq!(
            t.add(65_000, Kind::Note, "🔴 OBS crashed"),
            "`1:05` 🔴 OBS crashed"
        );
        t.add(5_000, Kind::Highlight, "⭐ Clip");
        assert_eq!(t.render(4000), "`0:05` ⭐ Clip\n`1:05` 🔴 OBS crashed");
        assert_eq!(t.count(Kind::Highlight), 1);
    }

    #[test]
    fn a_long_timeline_drops_its_oldest_lines_first() {
        let mut t = Timeline::default();
        for n in 0..100 {
            t.add(n * 1000, Kind::Note, &format!("line {n}"));
        }
        let shown = t.render(200);
        assert!(shown.chars().count() <= 200, "{shown}");
        assert!(shown.starts_with("… "), "{shown}");
        assert!(shown.ends_with("line 99"));
    }

    #[test]
    fn chapters_follow_youtube_rules() {
        let mut t = Timeline::default();
        assert_eq!(t.chapters(), "");
        t.add(751_000, Kind::Chapter, "Ranked");
        t.add(755_000, Kind::Chapter, "Too soon");
        t.add(3_850_000, Kind::Chapter, "Boss fight");
        t.add(900_000, Kind::Note, "not a chapter");
        assert_eq!(t.chapters(), "0:00 Start\n12:31 Ranked\n1:04:10 Boss fight");
    }

    #[test]
    fn a_chapter_in_the_first_seconds_becomes_the_opening() {
        let mut t = Timeline::default();
        t.add(4_000, Kind::Chapter, "Just chatting");
        t.add(60_000, Kind::Chapter, "Game");
        assert_eq!(t.chapters(), "", "two chapters: YouTube ignores them");
        t.add(120_000, Kind::Chapter, "Ranked");
        assert_eq!(t.chapters(), "0:00 Just chatting\n1:00 Game\n2:00 Ranked");
    }

    #[test]
    fn a_full_timeline_lets_notes_go_before_chapters() {
        let mut t = Timeline::default();
        t.add(0, Kind::Chapter, "Opening");
        for n in 1..MAX_ENTRIES as u64 + 50 {
            t.add(n * 1000, Kind::Note, "back");
        }
        assert_eq!(t.count(Kind::Chapter), 1);
        assert_eq!(t.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn after_a_crash_chapter_the_stream_goes_back_to_the_one_before() {
        let mut t = Timeline::default();
        assert_eq!(t.previous_chapter(), "Start");
        t.add(60_000, Kind::Chapter, "Ranked");
        t.add(90_000, Kind::Chapter, "Technical difficulties");
        assert_eq!(t.previous_chapter(), "Ranked");
    }

    #[test]
    fn lines_stay_on_one_line() {
        let mut t = Timeline::default();
        t.add(0, Kind::Note, "a\nb\r\nc");
        assert_eq!(t.render(100), "`0:00` a b  c");
    }
}
