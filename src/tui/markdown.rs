//! Builds the audit log as markdown and renders the small subset we use
//! (headings, bullets, fenced code, **bold**, `code`) into ratatui lines.

use std::fmt::Write as _;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::model::{self, Finding, Severity};

pub fn sev_color(s: Severity) -> Color {
    match s {
        Severity::Critical => Color::Red,
        Severity::High => Color::LightRed,
        Severity::Medium => Color::Yellow,
        Severity::Low => Color::Blue,
        Severity::Info => Color::Gray,
    }
}

pub fn report(rel: &str, findings: &[Finding], source: &str) -> String {
    let src: Vec<&str> = source.lines().collect();
    let mut md = String::new();
    let _ = writeln!(md, "# {rel}\n");

    if findings.is_empty() {
        md.push_str("No issues found.\n");
        return md;
    }

    let mut counts = [0usize; 5];
    for f in findings {
        counts[f.severity as usize] += 1;
    }
    let names = ["info", "low", "medium", "high", "critical"];
    let parts: Vec<String> = (0..5)
        .rev()
        .filter(|&i| counts[i] > 0)
        .map(|i| format!("{} {}", counts[i], names[i]))
        .collect();
    let _ = writeln!(md, "**{} findings:** {}\n", findings.len(), parts.join(", "));

    for (n, f) in findings.iter().enumerate() {
        let _ = writeln!(md, "## {}. [{}] {}", n + 1, f.severity.as_str().to_uppercase(), f.title);
        match f.end_line {
            Some(e) if e > f.line => {
                let _ = writeln!(md, "Lines {}-{}\n", f.line, e);
            }
            _ => {
                let _ = writeln!(md, "Line {}\n", f.line);
            }
        }
        if !f.detail.trim().is_empty() {
            let _ = writeln!(md, "{}\n", f.detail.trim());
        }
        if let Some(e) = &f.fix {
            let _ = writeln!(md, "**Suggested fix** (lines {}-{}):\n", e.start_line, e.end_line);
            md.push_str("```diff\n");
            for l in src.iter().skip(e.start_line - 1).take(e.end_line - e.start_line + 1) {
                let _ = writeln!(md, "-{l}");
            }
            for l in e.new_text.lines() {
                let _ = writeln!(md, "+{l}");
            }
            md.push_str("```\n\n");
        }
    }

    let edits: Vec<&model::Edit> = findings.iter().filter_map(|f| f.fix.as_ref()).collect();
    if !edits.is_empty() {
        let patched = model::apply_edits(source, &edits);
        let diff = model::unified_diff(rel, source, &patched);
        if !diff.is_empty() {
            md.push_str("## Combined patch\n\nPress `p` to apply. The original is kept as `.orig`.\n\n```diff\n");
            md.push_str(&diff);
            if !diff.ends_with('\n') {
                md.push('\n');
            }
            md.push_str("```\n");
        }
    }
    md
}

pub fn render(md: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code = false;
    let mut lang = String::new();

    for raw in md.lines() {
        if let Some(rest) = raw.trim_start().strip_prefix("```") {
            if in_code {
                in_code = false;
            } else {
                in_code = true;
                lang = rest.trim().to_string();
            }
            continue;
        }
        if in_code {
            out.push(code_line(raw, &lang));
            continue;
        }
        if let Some(h) = raw.strip_prefix("## ") {
            let color = ["CRITICAL", "HIGH", "MEDIUM", "LOW", "INFO"]
                .iter()
                .zip([Severity::Critical, Severity::High, Severity::Medium, Severity::Low, Severity::Info])
                .find(|(tag, _)| h.contains(&format!("[{tag}]")))
                .map_or(Color::White, |(_, s)| sev_color(s));
            out.push(Line::styled(
                h.to_string(),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ));
        } else if let Some(h) = raw.strip_prefix("# ") {
            out.push(Line::styled(
                h.to_string(),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ));
        } else if let Some(item) = raw.strip_prefix("- ").or_else(|| raw.strip_prefix("* ")) {
            let mut spans = vec![Span::raw("  \u{2022} ")];
            spans.extend(inline(item));
            out.push(Line::from(spans));
        } else if raw.trim().is_empty() {
            out.push(Line::raw(""));
        } else {
            out.push(Line::from(inline(raw)));
        }
    }
    out
}

fn code_line(raw: &str, lang: &str) -> Line<'static> {
    let style = if lang == "diff" {
        if raw.starts_with("+++") || raw.starts_with("---") {
            Style::new().add_modifier(Modifier::BOLD)
        } else if raw.starts_with('+') {
            Style::new().fg(Color::Green)
        } else if raw.starts_with('-') {
            Style::new().fg(Color::Red)
        } else if raw.starts_with('@') {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::Gray)
        }
    } else {
        Style::new().fg(Color::Gray)
    };
    Line::from(vec![Span::raw("  "), Span::styled(raw.to_string(), style)])
}

fn inline(s: &str) -> Vec<Span<'static>> {
    fn flush(spans: &mut Vec<Span<'static>>, buf: &mut String, bold: bool, code: bool) {
        if buf.is_empty() {
            return;
        }
        let mut st = Style::new();
        if bold {
            st = st.add_modifier(Modifier::BOLD);
        }
        if code {
            st = st.fg(Color::Green);
        }
        spans.push(Span::styled(std::mem::take(buf), st));
    }

    let mut spans = Vec::new();
    let mut buf = String::new();
    let (mut bold, mut code) = (false, false);
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '`' => {
                flush(&mut spans, &mut buf, bold, code);
                code = !code;
            }
            '*' if !code && it.peek() == Some(&'*') => {
                it.next();
                flush(&mut spans, &mut buf, bold, code);
                bold = !bold;
            }
            _ => buf.push(c),
        }
    }
    flush(&mut spans, &mut buf, bold, code);
    spans
}
