//! Very small per-line highlighter. It does not track multi-line strings or
//! block comments; good enough for reading code next to an audit log.

use ratatui::{
    style::{Color, Style},
    text::Span,
};

const COMMENT: Style = Style::new().fg(Color::DarkGray);
const STRING: Style = Style::new().fg(Color::Green);
const NUMBER: Style = Style::new().fg(Color::Yellow);
const KEYWORD: Style = Style::new().fg(Color::Magenta);

const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl", "trait", "match", "if",
    "else", "for", "while", "loop", "return", "break", "continue", "const", "static", "async",
    "await", "unsafe", "def", "class", "import", "from", "as", "with", "try", "except", "finally",
    "raise", "lambda", "yield", "pass", "function", "var", "new", "delete", "throw", "catch",
    "switch", "case", "default", "public", "private", "protected", "void", "int", "char", "long",
    "short", "float", "double", "bool", "true", "false", "null", "None", "True", "False", "nil",
    "self", "this", "func", "defer", "package", "interface", "type", "extends", "implements",
    "final", "namespace", "using", "echo", "elif", "fi", "then", "do", "done", "SELECT", "FROM",
    "WHERE", "INSERT", "UPDATE", "DELETE",
];

fn comment_prefix(ext: &str) -> &'static str {
    match ext {
        "py" | "rb" | "sh" | "bash" | "pl" | "ex" | "exs" => "#",
        "sql" | "lua" | "hs" => "--",
        _ => "//",
    }
}

fn matches_at(ch: &[char], i: usize, pat: &str) -> bool {
    pat.chars().enumerate().all(|(k, p)| ch.get(i + k) == Some(&p))
}

/// End index (exclusive) of the string literal starting at `start`.
/// A lone `'` with no closing quote is not a string (think Rust lifetimes).
fn string_end(ch: &[char], start: usize) -> Option<usize> {
    let q = ch[start];
    let mut i = start + 1;
    while i < ch.len() {
        if ch[i] == '\\' {
            i += 2;
        } else if ch[i] == q {
            return Some(i + 1);
        } else {
            i += 1;
        }
    }
    if q == '\'' {
        None
    } else {
        Some(ch.len())
    }
}

fn flush(out: &mut Vec<Span<'static>>, plain: &mut String) {
    if !plain.is_empty() {
        out.push(Span::raw(std::mem::take(plain)));
    }
}

pub fn line(src: &str, ext: &str) -> Vec<Span<'static>> {
    let cmt = comment_prefix(ext);
    let t = src.trim_start();
    if cmt == "//" && (t.starts_with("/*") || t.starts_with("* ") || t.starts_with("*/")) {
        return vec![Span::styled(src.to_string(), COMMENT)];
    }

    let ch: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut i = 0;

    while i < ch.len() {
        let c = ch[i];

        if matches_at(&ch, i, cmt) {
            flush(&mut out, &mut plain);
            out.push(Span::styled(ch[i..].iter().collect::<String>(), COMMENT));
            return out;
        }

        if c == '"' || c == '\'' || c == '`' {
            if let Some(end) = string_end(&ch, i) {
                flush(&mut out, &mut plain);
                out.push(Span::styled(ch[i..end].iter().collect::<String>(), STRING));
                i = end;
                continue;
            }
        }

        if c.is_ascii_digit() {
            let s = i;
            while i < ch.len() && (ch[i].is_alphanumeric() || ch[i] == '.' || ch[i] == '_') {
                i += 1;
            }
            flush(&mut out, &mut plain);
            out.push(Span::styled(ch[s..i].iter().collect::<String>(), NUMBER));
            continue;
        }

        if c.is_alphabetic() || c == '_' {
            let s = i;
            while i < ch.len() && (ch[i].is_alphanumeric() || ch[i] == '_') {
                i += 1;
            }
            let word: String = ch[s..i].iter().collect();
            if KEYWORDS.contains(&word.as_str()) {
                flush(&mut out, &mut plain);
                out.push(Span::styled(word, KEYWORD));
            } else {
                plain.push_str(&word);
            }
            continue;
        }

        plain.push(c);
        i += 1;
    }

    flush(&mut out, &mut plain);
    out
}
