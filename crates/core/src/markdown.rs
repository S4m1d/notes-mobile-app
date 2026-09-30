//! Line-level markdown classification for the editor. The editor shows every
//! line rendered except the one being edited, so rendering works per line:
//! this module decides the block kind of each line, the UI styles it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Blank,
    Paragraph,
    /// Heading level 1-6.
    Heading(u8),
    Bullet,
    Ordered,
    TaskOpen,
    TaskDone,
    Quote,
    /// A ``` line opening or closing a code block.
    Fence,
    /// A line inside a fenced code block.
    Code,
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineInfo {
    pub kind: Kind,
    /// Nesting level derived from leading whitespace (2 spaces or a tab per level).
    pub indent: u8,
    /// List marker to display, e.g. "3." for ordered items.
    pub marker: String,
    /// The text after the block marker; still contains inline markdown.
    pub content: String,
}

pub const TASK_PREFIX: &str = "- [ ] ";

fn indent_of(line: &str) -> (u8, &str) {
    let mut columns = 0;
    let mut rest = line;
    loop {
        if let Some(r) = rest.strip_prefix(' ') {
            columns += 1;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('\t') {
            columns += 2;
            rest = r;
        } else {
            break;
        }
    }
    ((columns / 2).min(12) as u8, rest)
}

fn strip_bullet(s: &str) -> Option<&str> {
    ["- ", "* ", "+ "].iter().find_map(|m| s.strip_prefix(m))
}

/// Splits "12. text" into ("12.", "text").
fn strip_ordered(s: &str) -> Option<(&str, &str)> {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let rest = &s[digits..];
    if rest.starts_with(". ") || rest.starts_with(") ") {
        Some((&s[..digits + 1], &rest[2..]))
    } else {
        None
    }
}

fn strip_task(item: &str) -> Option<(bool, &str)> {
    for (mark, done) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
        if let Some(rest) = item.strip_prefix(mark) {
            if rest.is_empty() || rest.starts_with(' ') {
                return Some((done, rest.trim_start()));
            }
        }
    }
    None
}

fn is_rule(s: &str) -> bool {
    let s = s.trim();
    s.len() >= 3 && ['-', '*', '_'].iter().any(|c| s.chars().all(|x| x == *c))
}

fn classify_line(line: &str) -> LineInfo {
    let (indent, rest) = indent_of(line);
    let info = |kind, marker: &str, content: &str| LineInfo {
        kind,
        indent,
        marker: marker.to_string(),
        content: content.to_string(),
    };
    if rest.trim().is_empty() {
        return info(Kind::Blank, "", "");
    }
    if is_rule(rest) {
        return info(Kind::Rule, "", "");
    }
    let hashes = rest.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && rest[hashes..].starts_with(' ') {
        return info(Kind::Heading(hashes as u8), "", rest[hashes..].trim());
    }
    if let Some(item) = strip_bullet(rest) {
        return match strip_task(item) {
            Some((true, text)) => info(Kind::TaskDone, "", text),
            Some((false, text)) => info(Kind::TaskOpen, "", text),
            None => info(Kind::Bullet, "•", item),
        };
    }
    if let Some((marker, text)) = strip_ordered(rest) {
        return info(Kind::Ordered, marker, text);
    }
    if let Some(text) = rest.strip_prefix('>') {
        return info(Kind::Quote, "", text.trim_start());
    }
    info(Kind::Paragraph, "", rest)
}

/// Classifies all lines of a note, tracking fenced code blocks across lines.
pub fn classify<S: AsRef<str>>(lines: &[S]) -> Vec<LineInfo> {
    let mut in_code = false;
    lines
        .iter()
        .map(|line| {
            let line = line.as_ref();
            if line.trim_start().starts_with("```") {
                in_code = !in_code;
                LineInfo { kind: Kind::Fence, indent: 0, marker: String::new(), content: line.trim().to_string() }
            } else if in_code {
                LineInfo { kind: Kind::Code, indent: 0, marker: String::new(), content: line.to_string() }
            } else {
                classify_line(line)
            }
        })
        .collect()
}

/// Flips the checkbox of a task line; returns `None` for non-task lines.
pub fn toggle_task(line: &str) -> Option<String> {
    let (_, rest) = indent_of(line);
    let item = strip_bullet(rest)?;
    let (done, _) = strip_task(item)?;
    let box_start = line.len() - item.len();
    let replacement = if done { "[ ]" } else { "[x]" };
    Some(format!("{}{}{}", &line[..box_start], replacement, &line[box_start + 3..]))
}

/// What pressing Enter on `line` should start the next line with, so lists
/// and task lists continue on their own. `None` means the line is an empty
/// list item: Enter should clear it instead of continuing the list.
pub fn continuation(line: &str) -> Option<String> {
    let (_, rest) = indent_of(line);
    let lead = &line[..line.len() - rest.len()];
    if let Some(item) = strip_bullet(rest) {
        let bullet = &rest[..2];
        return match strip_task(item) {
            Some((_, "")) => None,
            Some(_) => Some(format!("{lead}{bullet}[ ] ")),
            None if item.trim().is_empty() => None,
            None => Some(format!("{lead}{bullet}")),
        };
    }
    if let Some((marker, text)) = strip_ordered(rest) {
        if text.trim().is_empty() {
            return None;
        }
        let (number, delimiter) = marker.split_at(marker.len() - 1);
        let next = number.parse::<u64>().unwrap_or(0) + 1;
        return Some(format!("{lead}{next}{delimiter} "));
    }
    if let Some(text) = rest.strip_prefix('>') {
        return if text.trim().is_empty() { None } else { Some(format!("{lead}> ")) };
    }
    Some(lead.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<Kind> {
        let lines: Vec<&str> = text.split('\n').collect();
        classify(&lines).into_iter().map(|l| l.kind).collect()
    }

    #[test]
    fn classifies_blocks() {
        let text = "# Title\n\nplain *text*\n- item\n  - nested\n- [ ] open\n- [x] done\n1. first\n> quote\n---\n#tag";
        assert_eq!(
            kinds(text),
            vec![
                Kind::Heading(1),
                Kind::Blank,
                Kind::Paragraph,
                Kind::Bullet,
                Kind::Bullet,
                Kind::TaskOpen,
                Kind::TaskDone,
                Kind::Ordered,
                Kind::Quote,
                Kind::Rule,
                Kind::Paragraph,
            ]
        );
        let info = classify(&["  - [ ] buy milk"]);
        assert_eq!(info[0].indent, 1);
        assert_eq!(info[0].content, "buy milk");
        assert_eq!(classify(&["12. twelve"])[0].marker, "12.");
    }

    #[test]
    fn tracks_code_fences() {
        assert_eq!(
            kinds("```rust\n# not a heading\n- not a list\n```\n# heading"),
            vec![Kind::Fence, Kind::Code, Kind::Code, Kind::Fence, Kind::Heading(1)]
        );
    }

    #[test]
    fn toggles_tasks() {
        assert_eq!(toggle_task("- [ ] a").as_deref(), Some("- [x] a"));
        assert_eq!(toggle_task("  * [x] a").as_deref(), Some("  * [ ] a"));
        assert_eq!(toggle_task("- plain"), None);
        assert_eq!(toggle_task("- [ ]"), Some("- [x]".into()));
    }

    #[test]
    fn continues_lists() {
        assert_eq!(continuation("- [x] done").as_deref(), Some("- [ ] "));
        assert_eq!(continuation("  - item").as_deref(), Some("  - "));
        assert_eq!(continuation("9. nine").as_deref(), Some("10. "));
        assert_eq!(continuation("> quote").as_deref(), Some("> "));
        assert_eq!(continuation("plain").as_deref(), Some(""));
        assert_eq!(continuation("- [ ] "), None);
        assert_eq!(continuation("- "), None);
    }
}
