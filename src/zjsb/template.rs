//! Tiny template language for the strip and compact outputs, ported unchanged
//! from `zj-strip-bridge`.
//!
//! * `{field}` inserts a value, `{field:22}` truncates it to 22 characters,
//!   `{a|b|c}` uses the first non-empty value, `{{` and `}}` are literal braces.
//! * Everything outside braces passes through unchanged (it is opendeck-strip
//!   markup such as `[b]..[/b]` or `[c=red]..[/c]`, plus `\n` for a line break).
//! * A line whose placeholders are all empty is dropped.
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
/// separator, no markup, truncated on a character boundary.
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
    out.replace('[', "[[")
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
        if lines.len() > MAX_LINES {
            return Err(format!("more than {MAX_LINES} lines"));
        }
        Ok(Self { lines })
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    pub fn render(&self, ctx: &Ctx) -> String {
        let mut out = Vec::new();
        for line in &self.lines {
            let mut s = String::new();
            let mut fields = 0;
            let mut filled = 0;
            for p in line {
                match p {
                    Part::Lit(l) => s.push_str(l),
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
            if fields == 0 || filled > 0 {
                out.push(s);
            }
        }
        out.join("\\n")
    }
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
}
