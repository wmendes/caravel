//! Where things are in a lane file's sources, and errors that point there:
//!
//! ```text
//! error: [env.testnet] extends "bse", which the lane file doesn't have
//!   --> lanes/acme/lane.toml:42:11
//!    |
//! 42 | extends = "bse"
//!    |           ^^^^^
//!    = help: did you mean "base"?
//! ```

use std::fmt;
use std::ops::Range;
use std::path::PathBuf;

/// One loaded file.
#[derive(Clone, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub text: String,
}

/// Every file a lane file loaded (itself first, then its includes).
#[derive(Clone, Debug, Default)]
pub struct Sources(pub Vec<Source>);

/// A byte range in one of the sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub file: usize,
    pub range: Range<usize>,
}

impl Sources {
    pub fn add(&mut self, path: PathBuf, text: String) -> usize {
        self.0.push(Source { path, text });
        self.0.len() - 1
    }

    /// 1-based line and column of a byte offset.
    pub fn line_col(&self, file: usize, at: usize) -> (usize, usize) {
        let text = &self.0[file].text;
        let at = at.min(text.len());
        let before = &text[..at];
        let line = before.matches('\n').count() + 1;
        let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        (line, col)
    }

    fn line_text(&self, file: usize, line: usize) -> &str {
        self.0[file].text.lines().nth(line - 1).unwrap_or("")
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub message: String,
    pub span: Option<Span>,
    pub notes: Vec<String>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
            notes: Vec::new(),
            help: None,
        }
    }

    pub fn at(mut self, span: Option<Span>) -> Self {
        self.span = span;
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

/// Every problem found in a lane file, with the sources to show them in.
#[derive(Clone, Debug)]
pub struct Error {
    pub sources: Sources,
    pub diagnostics: Vec<Diagnostic>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, d) in self.diagnostics.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{}", d.message)?;
            if let Some(s) = &d.span {
                let (line, col) = self.sources.line_col(s.file, s.range.start);
                let width = line.to_string().len();
                let pad = " ".repeat(width);
                let text = self.sources.line_text(s.file, line);
                let (_, end_col) = self.sources.line_col(s.file, s.range.end);
                let carets = if self.sources.line_col(s.file, s.range.end).0 == line {
                    end_col.saturating_sub(col).max(1)
                } else {
                    text.chars().count().saturating_sub(col - 1).max(1)
                };
                write!(
                    f,
                    "\n{pad}--> {}:{line}:{col}\n{pad} |\n{line} | {text}\n{pad} | {}{}",
                    self.sources.0[s.file].path.display(),
                    " ".repeat(col - 1),
                    "^".repeat(carets)
                )?;
            }
            for n in &d.notes {
                write!(f, "\n  = note: {n}")?;
            }
            if let Some(h) = &d.help {
                write!(f, "\n  = help: {h}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// The closest of `known` to `word`, if it is close enough to be a typo.
pub fn did_you_mean<'a>(word: &str, known: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    known
        .into_iter()
        .map(|k| (distance(word, k), k))
        .filter(|(d, k)| {
            *d <= (k.len().max(word.len()) / 3).max(if word.len() > 3 { 2 } else { 1 })
        })
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(ca != *cb))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering() {
        let mut s = Sources::default();
        let f = s.add("lane.toml".into(), "[env.a]\nextends = \"bse\"\n".into());
        let at = s.0[f].text.find("\"bse\"").unwrap();
        let e = Error {
            sources: s,
            diagnostics: vec![Diagnostic::new(
                "[env.a] extends \"bse\", which the lane file doesn't have",
            )
            .at(Some(Span {
                file: f,
                range: at..at + 5,
            }))
            .help("did you mean \"base\"?")],
        };
        assert_eq!(
            e.to_string(),
            "[env.a] extends \"bse\", which the lane file doesn't have\n --> lane.toml:2:11\n  |\n2 | extends = \"bse\"\n  |           ^^^^^\n  = help: did you mean \"base\"?"
        );
    }

    #[test]
    fn typos() {
        assert_eq!(did_you_mean("bse", ["base", "testnet"]), Some("base"));
        assert_eq!(
            did_you_mean("testnte", ["base", "testnet"]),
            Some("testnet")
        );
        assert_eq!(did_you_mean("prod", ["base", "testnet"]), None);
        assert_eq!(did_you_mean("bsae", ["base", "local"]), Some("base"));
        assert_eq!(did_you_mean("lcoal", ["base", "local"]), Some("local"));
        assert_eq!(did_you_mean("ab", ["xy"]), None);
    }
}
