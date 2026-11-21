use crate::color::Rgb;
use crate::status::SpanSpec;

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub mono: bool,
    pub color: Option<Rgb>,
    pub bg: Option<Rgb>,
    /// Multiple of the slot's base font size.
    pub size: f32,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            text: String::new(),
            bold: false,
            italic: false,
            underline: false,
            mono: false,
            color: None,
            bg: None,
            size: 1.0,
        }
    }
}

impl Run {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    Bold,
    Italic,
    Underline,
    Mono,
    Color,
    Background,
    Size,
}

#[derive(Clone, Debug, PartialEq)]
struct State {
    bold: bool,
    italic: bool,
    underline: bool,
    mono: bool,
    color: Option<Rgb>,
    bg: Option<Rgb>,
    size: f32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            mono: false,
            color: None,
            bg: None,
            size: 1.0,
        }
    }
}

impl State {
    fn run(&self, text: String) -> Run {
        Run {
            text,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            mono: self.mono,
            color: self.color,
            bg: self.bg,
            size: self.size,
        }
    }
}

fn same_style(a: &Run, b: &Run) -> bool {
    a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.mono == b.mono
        && a.color == b.color
        && a.bg == b.bg
        && (a.size - b.size).abs() < f32::EPSILON
}

fn push_text(runs: &mut Vec<Run>, state: &State, text: &str) {
    if text.is_empty() {
        return;
    }
    let run = state.run(text.to_owned());
    if let Some(last) = runs.last_mut()
        && same_style(last, &run)
    {
        last.text.push_str(text);
        return;
    }
    runs.push(run);
}

/// Parse one line of BBCode-style markup. Tags do not outlive the line.
pub fn parse_line(input: &str, plain: bool) -> Vec<Run> {
    if plain {
        return vec![Run::plain(input)];
    }

    let mut runs = Vec::new();
    let mut state = State::default();
    let mut stack: Vec<(Tag, State)> = Vec::new();
    let mut text = String::new();
    let mut chars = input.char_indices().peekable();

    while let Some((_, c)) = chars.next() {
        if c != '[' {
            text.push(c);
            continue;
        }
        if chars.peek().is_some_and(|(_, next)| *next == '[') {
            chars.next();
            text.push('[');
            continue;
        }

        let rest = &input[chars.peek().map_or(input.len(), |(i, _)| *i)..];
        let Some(end) = rest.find(']') else {
            text.push('[');
            continue;
        };
        let tag = &rest[..end];
        if let Some(updated) = apply_tag(&mut stack, tag, &state) {
            push_text(&mut runs, &state, &text);
            text.clear();
            state = updated;
        } else {
            text.push('[');
            text.push_str(tag);
            text.push(']');
        }
        // Skip the tag's contents plus ']'.
        for _ in 0..=tag.chars().count() {
            chars.next();
        }
    }
    push_text(&mut runs, &state, &text);
    runs
}

/// Returns the new state when `tag` is recognised and balanced.
fn apply_tag(stack: &mut Vec<(Tag, State)>, tag: &str, state: &State) -> Option<State> {
    let tag = tag.trim();
    if let Some(name) = tag.strip_prefix('/') {
        let wanted = match name {
            "b" => Tag::Bold,
            "i" => Tag::Italic,
            "u" => Tag::Underline,
            "mono" => Tag::Mono,
            "c" => Tag::Color,
            "bg" => Tag::Background,
            "s" => Tag::Size,
            _ => return None,
        };
        return match stack.last() {
            Some((top, previous)) if *top == wanted => {
                let previous = previous.clone();
                stack.pop();
                Some(previous)
            }
            _ => None,
        };
    }

    let (kind, next) = if tag == "b" {
        (
            Tag::Bold,
            State {
                bold: true,
                ..state.clone()
            },
        )
    } else if tag == "i" {
        (
            Tag::Italic,
            State {
                italic: true,
                ..state.clone()
            },
        )
    } else if tag == "u" {
        (
            Tag::Underline,
            State {
                underline: true,
                ..state.clone()
            },
        )
    } else if tag == "mono" {
        (
            Tag::Mono,
            State {
                mono: true,
                ..state.clone()
            },
        )
    } else if let Some(value) = tag.strip_prefix("c=") {
        (
            Tag::Color,
            State {
                color: Some(Rgb::parse(value)?),
                ..state.clone()
            },
        )
    } else if let Some(value) = tag.strip_prefix("bg=") {
        (
            Tag::Background,
            State {
                bg: Some(Rgb::parse(value)?),
                ..state.clone()
            },
        )
    } else {
        let value = tag.strip_prefix("s=")?;
        let size: f32 = value.trim().parse().ok()?;
        (
            Tag::Size,
            State {
                size: size.clamp(0.5, 3.0),
                ..state.clone()
            },
        )
    };
    stack.push((kind, state.clone()));
    Some(next)
}

/// Parse a full multi-line string. Tags are closed at each line end.
pub fn parse_text(text: &str, plain: bool) -> Vec<Vec<Run>> {
    text.split('\n')
        .map(|line| parse_line(line, plain))
        .collect()
}

/// Build lines from explicit spans. Span styles do carry across newlines.
pub fn spans_to_lines(spans: &[SpanSpec]) -> Vec<Vec<Run>> {
    let mut lines: Vec<Vec<Run>> = vec![Vec::new()];
    for span in spans {
        let color = span.color.as_deref().and_then(Rgb::parse);
        let bg = span.bg.as_deref().and_then(Rgb::parse);
        let size = span.size.unwrap_or(1.0).clamp(0.5, 3.0);
        let mut parts = span.text.split('\n').peekable();
        while let Some(part) = parts.next() {
            let run = Run {
                text: part.to_owned(),
                bold: span.bold,
                italic: span.italic,
                underline: span.underline,
                mono: span.mono,
                color,
                bg,
                size,
            };
            let line = lines.last_mut().expect("lines is never empty");
            if let Some(last) = line.last_mut()
                && same_style(last, &run)
            {
                last.text.push_str(&run.text);
            } else if !run.text.is_empty() {
                line.push(run);
            }
            if parts.peek().is_some() {
                lines.push(Vec::new());
            }
        }
    }
    lines
}

pub fn plain_of_runs(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

pub fn plain_of_lines(lines: &[Vec<Run>]) -> String {
    lines
        .iter()
        .map(|line| plain_of_runs(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `|` splits the strip into independent segments. Tags cannot carry over a split.
pub fn split_segments(text: &str) -> Vec<&str> {
    text.split('|').collect()
}

/// Turn user text into markup that renders literally (`set --plain`).
pub fn escape_markup(text: &str) -> String {
    text.replace('[', "[[")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::type_complexity)]
    fn styles(runs: &[Run]) -> Vec<(&str, bool, bool, bool, Option<Rgb>, Option<Rgb>, f32)> {
        runs.iter()
            .map(|r| {
                (
                    r.text.as_str(),
                    r.bold,
                    r.italic,
                    r.underline,
                    r.color,
                    r.bg,
                    r.size,
                )
            })
            .collect()
    }

    #[test]
    fn plain_text() {
        let runs = parse_line("hello [b]world[/b]", false);
        assert_eq!(
            styles(&runs),
            vec![
                ("hello ", false, false, false, None, None, 1.0),
                ("world", true, false, false, None, None, 1.0)
            ]
        );
    }

    #[test]
    fn nesting() {
        let runs = parse_line("[b]a[u]b[/u]c[/b]", false);
        assert!(runs[0].bold && !runs[0].underline);
        assert_eq!(runs[0].text, "a");
        assert!(runs[1].bold && runs[1].underline);
        assert_eq!(runs[1].text, "b");
        assert!(runs[2].bold && !runs[2].underline);
        assert_eq!(runs[2].text, "c");
    }

    #[test]
    fn escapes_literal_bracket() {
        let runs = parse_line("a [[b] c", false);
        assert_eq!(plain_of_runs(&runs), "a [b] c");
    }

    #[test]
    fn unknown_tags_are_literal() {
        let runs = parse_line("[wat]x[/wat]", false);
        assert_eq!(plain_of_runs(&runs), "[wat]x[/wat]");
    }

    #[test]
    fn unclosed_tags_end_at_line_end() {
        let lines = parse_text("[b]one\ntwo", false);
        assert!(lines[0][0].bold);
        assert!(!lines[1][0].bold);
    }

    #[test]
    fn colors_and_sizes() {
        let runs = parse_line("[c=#ff8800]a[/c][bg=red]b[/bg][s=1.5]c[/s]", false);
        assert_eq!(runs[0].color, Some(Rgb(255, 136, 0)));
        assert_eq!(runs[1].bg, Some(Rgb(255, 80, 80)));
        assert!((runs[2].size - 1.5).abs() < f32::EPSILON);
        let runs = parse_line("[s=99]c[/s]", false);
        assert!((runs[0].size - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn mismatched_close_is_literal() {
        let runs = parse_line("[b]x[/i]", false);
        assert_eq!(plain_of_runs(&runs), "x[/i]");
        assert!(runs[0].bold);
    }

    #[test]
    fn per_segment_split() {
        assert_eq!(split_segments("a|b|"), vec!["a", "b", ""]);
        assert_eq!(split_segments("a\nb"), vec!["a\nb"]);
    }

    #[test]
    fn spans_and_plain() {
        let spans = vec![
            SpanSpec {
                text: "a\nb".into(),
                bold: true,
                ..Default::default()
            },
            SpanSpec {
                text: "c".into(),
                color: Some("#f00".into()),
                ..Default::default()
            },
        ];
        let lines = spans_to_lines(&spans);
        assert_eq!(lines.len(), 2);
        assert_eq!(plain_of_lines(&lines), "a\nbc");
        assert!(lines[0][0].bold && lines[1][0].bold);
        assert_eq!(lines[1][1].color, Some(Rgb(255, 0, 0)));
    }

    #[test]
    fn escape_markup_roundtrip() {
        let escaped = escape_markup("[x]");
        let runs = parse_line(&escaped, false);
        assert_eq!(plain_of_runs(&runs), "[x]");
    }
}
