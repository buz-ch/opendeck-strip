//! Tiny template language for the strip and compact outputs, ported unchanged
//! from `zj-strip-bridge`.
//!
//! * `{field}` inserts a value, `{field:22}` truncates it to 22 characters,
//!   `{a|b|c}` uses the first non-empty value, `{{` and `}}` are literal braces.
//! * Everything outside braces passes through unchanged (it is opendeck-strip
//!   markup such as `[b]..[/b]` or `[c=red]..[/c]`, plus `\n` for a line break).
//! * A line whose placeholders are all empty is dropped; `render_cells` keeps
//!   it as an empty cell.
//! * Values are sanitised; template text is trusted but may not contain `|`,
//!   because that would add a strip segment.
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Lit(String),
    Field {
        names: Vec<String>,
        width: Option<usize>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Template {
    lines: Vec<Vec<Part>>,
}

pub type Ctx = BTreeMap<&'static str, String>;

const DEFAULT_WIDTH: usize = 64;
const MAX_LINES: usize = 8;

/// Make untrusted text safe inside a strip string: no controls, no segment
/// separator, no markup, no escape sequences, truncated on a character boundary.
pub fn sanitize(text: &str, max_chars: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() || c == '|' { ' ' } else { c })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = cleaned.chars().take(max_chars).collect();
    if cleaned.chars().count() > max_chars && max_chars > 1 {
        out.pop();
        out.push('…');
    }
    // `apply_set` unescapes `\n`, `\t` and `\\`, so a backslash must survive that.
    out.replace('\\', "\\\\").replace('[', "[[")
}

impl Template {
    /// Parse and validate against the fields the caller can supply.
    pub fn parse(text: &str, allowed: &[&str]) -> Result<Self, String> {
        // Both a real newline (TOML turns `\n` into one) and a literal `\n`
        // separate lines; output always uses the literal form.
        let normalized = text.replace("\\n", "\n");
        let mut lines = Vec::new();
        for raw in normalized.split('\n') {
            lines.push(parse_line(raw, allowed)?);
        }
        // An empty line renders nothing and is not kept, so that the empty
        // template is `Template::default()`.
        lines.retain(|l| !l.is_empty());
        if lines.len() > MAX_LINES {
            return Err(format!("more than {MAX_LINES} lines"));
        }
        Ok(Self { lines })
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    /// Number of template lines kept by `parse`.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn render(&self, ctx: &Ctx) -> String {
        self.lines
            .iter()
            .filter_map(|line| render_line(line, ctx, Tabs::Keep))
            .collect::<Vec<_>>()
            .join("\\n")
    }

    /// One string per kept line; a line whose placeholders are all empty is "".
    /// A tab in template text, literal or the two characters `\t`, is a space,
    /// so one template line is one grid cell.
    pub fn render_cells(&self, ctx: &Ctx) -> Vec<String> {
        self.lines
            .iter()
            .map(|line| render_line(line, ctx, Tabs::Space).unwrap_or_default())
            .collect()
    }
}

/// How a tab in template text is rendered.
#[derive(Clone, Copy)]
enum Tabs {
    Keep,
    Space,
}

/// The line, or `None` when every placeholder on it is empty.
fn render_line(line: &[Part], ctx: &Ctx, tabs: Tabs) -> Option<String> {
    let mut s = String::new();
    let mut fields = 0;
    let mut filled = 0;
    for p in line {
        match p {
            Part::Lit(l) => match tabs {
                Tabs::Keep => s.push_str(l),
                Tabs::Space => s.push_str(&untab(l)),
            },
            Part::Field { names, width } => {
                fields += 1;
                let v = names
                    .iter()
                    .filter_map(|n| ctx.get(n.as_str()))
                    .find(|v| !v.trim().is_empty());
                if let Some(v) = v {
                    filled += 1;
                    s.push_str(&sanitize(v, width.unwrap_or(DEFAULT_WIDTH)));
                }
            }
        }
    }
    (fields == 0 || filled > 0).then_some(s)
}

/// A tab becomes one space; `\\t` is an escaped backslash and a `t` and stays.
fn untab(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\t' => out.push(' '),
            '\\' => match chars.next() {
                Some('t') => out.push(' '),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            c => out.push(c),
        }
    }
    out
}

fn parse_line(raw: &str, allowed: &[&str]) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut it = raw.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '{' if it.peek() == Some(&'{') => {
                it.next();
                lit.push('{');
            }
            '}' if it.peek() == Some(&'}') => {
                it.next();
                lit.push('}');
            }
            '{' => {
                let mut body = String::new();
                loop {
                    match it.next() {
                        Some('}') => break,
                        Some(ch) => body.push(ch),
                        None => return Err(format!("unclosed `{{` in `{raw}`")),
                    }
                }
                if !lit.is_empty() {
                    parts.push(Part::Lit(std::mem::take(&mut lit)));
                }
                parts.push(parse_field(&body, allowed)?);
            }
            '}' => return Err(format!("stray `}}` in `{raw}` (use `}}}}`)")),
            '|' => return Err("`|` outside braces would add a strip segment".into()),
            c => lit.push(c),
        }
    }
    if !lit.is_empty() {
        parts.push(Part::Lit(lit));
    }
    Ok(parts)
}

fn parse_field(body: &str, allowed: &[&str]) -> Result<Part, String> {
    let (names, width) = match body.rsplit_once(':') {
        Some((n, w)) => {
            let w: usize = w
                .trim()
                .parse()
                .map_err(|_| format!("bad width in `{{{body}}}`"))?;
            if w == 0 {
                return Err(format!("width 0 in `{{{body}}}`"));
            }
            (n, Some(w))
        }
        None => (body, None),
    };
    let names: Vec<String> = names.split('|').map(|n| n.trim().to_string()).collect();
    for n in &names {
        if !allowed.contains(&n.as_str()) {
            return Err(format!(
                "unknown field `{n}` (known: {})",
                allowed.join(", ")
            ));
        }
    }
    Ok(Part::Field { names, width })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[&str] = &["a", "b", "c"];

    fn ctx(pairs: &[(&'static str, &str)]) -> Ctx {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn fields_width_fallback_and_escapes() {
        let t = Template::parse("{a:3}|x", ALL);
        assert!(t.is_err(), "`|` outside braces is rejected");
        let t = Template::parse("{{{a:4}}} {b|c}", ALL).unwrap();
        assert_eq!(t.render(&ctx(&[("a", "abcdefgh"), ("c", "z")])), "{abc…} z");
    }

    #[test]
    fn empty_lines_are_dropped_but_literal_lines_stay() {
        let t = Template::parse("head\\n{a}\\n{b|c}\nfoot", ALL).unwrap(); // literal and real newline
        assert_eq!(t.render(&ctx(&[("b", "x")])), "head\\nx\\nfoot");
    }

    #[test]
    fn parse_errors() {
        for bad in ["{nope}", "{a", "a}", "{a:0}", "{a:x}", "{a|nope}"] {
            assert!(Template::parse(bad, ALL).is_err(), "{bad}");
        }
    }

    #[test]
    fn values_cannot_inject_markup_or_segments() {
        let t = Template::parse("[b]{a}[/b]", ALL).unwrap();
        let out = t.render(&ctx(&[("a", "|[c=red]x\n\u{1b}")]));
        assert!(!out.contains('|') && !out.contains('\n') && out.contains("[[c=red]"));
    }

    #[test]
    fn more_than_eight_lines_are_refused() {
        assert!(Template::parse("a\\nb\\nc\\nd\\ne\\nf\\ng\\nh", ALL).is_ok());
        assert!(Template::parse("a\\nb\\nc\\nd\\ne\\nf\\ng\\nh\\ni", ALL).is_err());
    }

    #[test]
    fn line_count_is_the_kept_line_count() {
        assert_eq!(Template::default().line_count(), 0);
        assert_eq!(
            Template::parse("a\\n\\nb\\nc", ALL).unwrap().line_count(),
            3,
            "a blank line is removed before counting"
        );
        assert_eq!(
            Template::parse(" \\n{a}", ALL).unwrap().line_count(),
            2,
            "a line holding one space is a spacer"
        );
    }

    #[test]
    fn render_cells_keeps_a_line_whose_placeholders_are_all_empty() {
        let t = Template::parse("head\\n{a} tail\\n{b|c}\\nfoot", ALL).unwrap();
        assert_eq!(
            t.render_cells(&Ctx::new()),
            ["head", "", "", "foot"],
            "a line whose placeholders are all empty keeps its position, without its text"
        );
        assert_eq!(t.render(&Ctx::new()), "head\\nfoot");
        // A filled line stays in both.
        assert_eq!(
            t.render_cells(&ctx(&[("a", "x")])),
            ["head", "x tail", "", "foot"]
        );
        assert_eq!(t.render(&ctx(&[("a", "x")])), "head\\nx tail\\nfoot");
    }

    #[test]
    fn render_cells_turns_tabs_into_spaces_and_leaves_escaped_ones_alone() {
        let t = Template::parse("[b]a\\tb[/b]\\nc\td", ALL).unwrap();
        assert_eq!(t.render_cells(&Ctx::new()), ["[b]a b[/b]", "c d"]);
        assert_eq!(
            t.render(&Ctx::new()),
            "[b]a\\tb[/b]\\nc\td",
            "`render` keeps its behaviour"
        );
        // An escaped backslash then `t` is text, not a tab.
        let t = Template::parse("a\\\\tb", ALL).unwrap();
        assert_eq!(t.render_cells(&Ctx::new()), ["a\\\\tb"]);
    }

    #[test]
    fn values_cannot_make_cells() {
        let t = Template::parse("{a} <{b}>", ALL).unwrap();
        let cells = t.render_cells(&ctx(&[("a", "x\ty"), ("b", "x\ty")]));
        assert_eq!(cells, ["x y <x y>"], "a tab in a value is a space");
        let cells = t.render_cells(&ctx(&[("a", "x\\ty"), ("b", "x\\ty")]));
        assert_eq!(cells, ["x\\\\ty <x\\\\ty>"], "`\\t` in a value stays text");
    }
}
