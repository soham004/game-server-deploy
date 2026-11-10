//! Reading one log file, newest lines first in pages, with a filter. The file is streamed and
//! decompressed as it is read, never held in memory: only the page being built is.
//!
//! A line without its own level (a stack trace, usually) takes the level of the line before it, so
//! asking for errors brings their stack traces along.

use std::{
    collections::VecDeque,
    io::{self, BufRead, BufReader, Read, Seek},
};

use flate2::read::MultiGzDecoder;
use gsd_core::{
    files::{FilesError, ServerFiles},
    game::LogLevel,
};
use regex::{Regex, RegexBuilder};
use serde::Serialize;

use super::{LogFile, crash, has_suffix};
use crate::log_parser::{parse_line, strip_ansi};

/// The most lines one page holds.
pub const MAX_PAGE: usize = 1000;
/// Longer lines are cut here; the rest of the line is skipped.
const MAX_LINE_BYTES: usize = 16 * 1024;
/// How much of one file is read, decompressed. A small `.gz` can unpack to a great deal.
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// A search pattern longer than this isn't a search.
const MAX_PATTERN_BYTES: usize = 1000;
/// The `regex` crate runs in linear time, so this only bounds what a pattern can compile to.
const MAX_REGEX_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("there is no such log")]
    NotFound,
    #[error("that search can't be used: {0}")]
    BadSearch(String),
    #[error(transparent)]
    Files(#[from] FilesError),
    #[error("could not read the log")]
    Io(#[from] io::Error),
}

/// Which lines to read.
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Only lines before this one (the page before the last one read); the end of the file if absent.
    pub before: Option<u64>,
    pub limit: usize,
    pub search: Option<String>,
    /// `search` is a regular expression, not plain text.
    pub regex: bool,
    /// Only lines at least this severe: `trace`, `debug`, `info`, `warn`, `error`, `fatal`.
    pub level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogLine {
    /// From 1, in the whole file.
    pub number: u64,
    pub level: LogLevel,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogPage {
    /// In file order.
    pub lines: Vec<LogLine>,
    /// Pass as `before` for the page before this one; absent when nothing earlier matches.
    pub earlier: Option<u64>,
    /// The file is damaged or too big to read to its end, so this isn't all of it.
    pub truncated: bool,
    /// For a crash report, on the first page.
    pub crash: Option<crash::CrashSummary>,
}

/// The compiled search and level.
struct LineFilter {
    search: Option<Regex>,
    level: Option<u8>,
}

/// Severity as a number. A line of unknown level is below everything.
fn rank(level: LogLevel) -> u8 {
    match level {
        LogLevel::Unknown => 0,
        LogLevel::Trace => 1,
        LogLevel::Debug => 2,
        LogLevel::Info => 3,
        LogLevel::Warn => 4,
        LogLevel::Error => 5,
        LogLevel::Fatal => 6,
    }
}

impl LineFilter {
    fn new(query: &LogQuery) -> Result<Self, LogError> {
        let level = match query.level.as_deref().filter(|level| !level.is_empty()) {
            None => None,
            Some("trace") => Some(rank(LogLevel::Trace)),
            Some("debug") => Some(rank(LogLevel::Debug)),
            Some("info") => Some(rank(LogLevel::Info)),
            Some("warn") => Some(rank(LogLevel::Warn)),
            Some("error") => Some(rank(LogLevel::Error)),
            Some("fatal") => Some(rank(LogLevel::Fatal)),
            Some(other) => return Err(LogError::BadSearch(format!("no such level: {other}"))),
        };
        let search = match query.search.as_deref().filter(|search| !search.is_empty()) {
            None => None,
            Some(search) if search.len() > MAX_PATTERN_BYTES => {
                return Err(LogError::BadSearch("it is too long".into()));
            }
            Some(search) => {
                let pattern = if query.regex {
                    search.to_owned()
                } else {
                    regex::escape(search)
                };
                let compiled = RegexBuilder::new(&pattern)
                    .case_insensitive(true)
                    .size_limit(MAX_REGEX_BYTES)
                    .build()
                    .map_err(|error| LogError::BadSearch(error.to_string()))?;
                Some(compiled)
            }
        };
        Ok(Self { search, level })
    }

    fn matches(&self, text: &str, level: LogLevel) -> bool {
        self.level.is_none_or(|wanted| rank(level) >= wanted)
            && self.search.as_ref().is_none_or(|search| search.is_match(text))
    }
}

/// Reads a page of `log`. The search is checked before the file is touched.
pub async fn read(files: &ServerFiles, log: &LogFile, query: LogQuery) -> Result<LogPage, LogError> {
    let filter = LineFilter::new(&query)?;
    let (file, _) = files.open_file(log.path()).await?;
    let mut file = file.into_std().await;
    let compressed = has_suffix(&log.name, ".gz");
    let wants_summary = log.kind.is_crash() && query.before.is_none();
    tokio::task::spawn_blocking(move || {
        let summary = if wants_summary {
            let summary = crash::summarize_file(&mut file)?;
            file.rewind()?;
            Some(summary)
        } else {
            None
        };
        let source: Box<dyn Read + Send> = if compressed {
            Box::new(MultiGzDecoder::new(BufReader::new(file)))
        } else {
            Box::new(file)
        };
        let mut page = read_page(BufReader::new(source), &filter, query.before, query.limit);
        page.crash = summary;
        Ok(page)
    })
    .await
    .map_err(io::Error::other)?
}

fn read_page(mut reader: impl BufRead, filter: &LineFilter, before: Option<u64>, limit: usize) -> LogPage {
    let limit = limit.clamp(1, MAX_PAGE);
    let mut lines: VecDeque<LogLine> = VecDeque::with_capacity(limit.min(64));
    let mut more_before = false;
    let mut truncated = false;
    let mut number = 0;
    let mut level = LogLevel::Unknown;
    let mut bytes_read = 0;
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match read_line_capped(&mut reader, &mut buffer) {
            Ok(0) => break,
            Ok(read) => bytes_read += read as u64,
            // A damaged or still-growing `.gz`: what came before it is still worth showing.
            Err(_) => {
                truncated = true;
                break;
            }
        }
        if bytes_read > MAX_READ_BYTES {
            truncated = true;
            break;
        }
        number += 1;
        if before.is_some_and(|before| number >= before) {
            break;
        }
        let text = String::from_utf8_lossy(&buffer);
        let text = text.trim_end_matches(['\r', '\n']);
        let parsed = parse_line(text);
        if parsed.level != LogLevel::Unknown {
            level = parsed.level;
        }
        let clean = strip_ansi(text);
        if !filter.matches(&clean, level) {
            continue;
        }
        if lines.len() == limit {
            lines.pop_front();
            more_before = true;
        }
        lines.push_back(LogLine {
            number,
            level,
            text: clean.into_owned(),
        });
    }
    let earlier = if more_before {
        lines.front().map(|line| line.number)
    } else {
        None
    };
    LogPage {
        lines: lines.into(),
        earlier,
        truncated,
        crash: None,
    }
}

/// Reads one line into `line`, keeping at most [`MAX_LINE_BYTES`] of it. Returns the bytes consumed
/// (0 at the end). A log with no line ends can't fill the memory.
fn read_line_capped(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<usize> {
    let mut consumed = 0;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(consumed);
        }
        let end = available.iter().position(|byte| *byte == b'\n');
        let take = end.map_or(available.len(), |position| position + 1);
        let room = MAX_LINE_BYTES.saturating_sub(line.len());
        line.extend_from_slice(&available[..take.min(room)]);
        reader.consume(take);
        consumed += take;
        if end.is_some() {
            return Ok(consumed);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::{Cursor, Write};

    use flate2::{Compression, write::GzEncoder};

    use super::*;

    const LOG: &str = "\
[12:00:00] [main/INFO]: Starting
[12:00:01] [main/WARN]: Careful
[12:00:02] [main/ERROR]: Broke
java.lang.Error: boom
\tat com.example.Thing.run(Thing.java:1)
[12:00:03] [main/INFO]: Done (3.0s)! For help, type \"help\"
";

    fn page(text: &str, query: &LogQuery) -> LogPage {
        let filter = LineFilter::new(query).map_err(|error| error.to_string()).unwrap();
        read_page(
            Cursor::new(text.as_bytes().to_vec()),
            &filter,
            query.before,
            query.limit,
        )
    }

    fn query() -> LogQuery {
        LogQuery {
            limit: 100,
            ..LogQuery::default()
        }
    }

    fn numbers(page: &LogPage) -> Vec<u64> {
        page.lines.iter().map(|line| line.number).collect()
    }

    #[test]
    fn pages_come_from_the_end_and_go_back() {
        let tail = page(LOG, &LogQuery { limit: 2, ..query() });
        assert_eq!(numbers(&tail), [5, 6]);
        assert_eq!(tail.earlier, Some(5));

        let before = page(
            LOG,
            &LogQuery {
                limit: 2,
                before: tail.earlier,
                ..query()
            },
        );
        assert_eq!(numbers(&before), [3, 4]);
        assert_eq!(before.earlier, Some(3));

        let first = page(
            LOG,
            &LogQuery {
                limit: 2,
                before: before.earlier,
                ..query()
            },
        );
        assert_eq!(numbers(&first), [1, 2]);
        assert_eq!(first.earlier, None, "nothing before the first line");
    }

    #[test]
    fn asking_for_errors_brings_their_stack_traces() {
        let errors = page(
            LOG,
            &LogQuery {
                level: Some("error".into()),
                ..query()
            },
        );
        assert_eq!(numbers(&errors), [3, 4, 5]);
        assert!(errors.lines.iter().all(|line| line.level == LogLevel::Error));

        let warnings = page(
            LOG,
            &LogQuery {
                level: Some("warn".into()),
                ..query()
            },
        );
        assert_eq!(numbers(&warnings), [2, 3, 4, 5]);
    }

    #[test]
    fn search_is_plain_text_ignoring_case_unless_it_is_a_regex() {
        let plain = page(
            LOG,
            &LogQuery {
                search: Some("BROKE".into()),
                ..query()
            },
        );
        assert_eq!(numbers(&plain), [3]);

        // Plain text, so these are not special characters.
        let dots = page(
            LOG,
            &LogQuery {
                search: Some("3.0s)".into()),
                ..query()
            },
        );
        assert_eq!(numbers(&dots), [6]);

        let regex = page(
            LOG,
            &LogQuery {
                search: Some(r"Thing\.java:\d+".into()),
                regex: true,
                ..query()
            },
        );
        assert_eq!(numbers(&regex), [5]);

        let filtered = page(
            LOG,
            &LogQuery {
                search: Some("e".into()),
                limit: 1,
                ..query()
            },
        );
        assert_eq!(numbers(&filtered), [6]);
        assert_eq!(filtered.earlier, Some(6), "the page before is of matches too");
    }

    #[test]
    fn a_bad_search_or_level_is_refused() {
        for bad in [
            LogQuery {
                search: Some("(".into()),
                regex: true,
                ..query()
            },
            LogQuery {
                search: Some("x".repeat(MAX_PATTERN_BYTES + 1)),
                ..query()
            },
            LogQuery {
                level: Some("loud".into()),
                ..query()
            },
            // Compiles to far more than the limit.
            LogQuery {
                search: Some(r"(\w{1000}){1000}".into()),
                regex: true,
                ..query()
            },
        ] {
            assert!(
                matches!(LineFilter::new(&bad), Err(LogError::BadSearch(_))),
                "{bad:?}"
            );
        }
        assert!(
            LineFilter::new(&LogQuery {
                search: Some("(".into()),
                ..query()
            })
            .is_ok(),
            "plain text"
        );
    }

    #[test]
    fn colours_are_removed_and_long_lines_cut() {
        let long = "x".repeat(MAX_LINE_BYTES * 3);
        let text = format!("\u{1b}[31mred\u{1b}[0m\n{long}\nafter\n");
        let read = page(&text, &query());
        assert_eq!(read.lines[0].text, "red");
        assert_eq!(read.lines[1].text.len(), MAX_LINE_BYTES);
        assert_eq!(
            read.lines[2].text, "after",
            "the rest of the long line is skipped"
        );
        assert_eq!(numbers(&read), [1, 2, 3]);
    }

    #[test]
    fn not_quite_text_is_still_shown() {
        let mut bytes = b"[12:00:00] [main/INFO]: caf\xc3\xa9\r\nbad \xff byte".to_vec();
        bytes.push(b'\n');
        let filter = LineFilter::new(&query())
            .map_err(|error| error.to_string())
            .unwrap();
        let read = read_page(Cursor::new(bytes), &filter, None, 10);
        assert!(read.lines[0].text.ends_with("café"));
        assert!(read.lines[1].text.starts_with("bad "));
    }

    #[test]
    fn a_compressed_log_reads_the_same_and_a_damaged_one_up_to_the_damage() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(LOG.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        let filter = LineFilter::new(&query())
            .map_err(|error| error.to_string())
            .unwrap();

        let read = read_page(
            BufReader::new(MultiGzDecoder::new(Cursor::new(compressed.clone()))),
            &filter,
            None,
            100,
        );
        assert_eq!(numbers(&read), [1, 2, 3, 4, 5, 6]);
        assert!(!read.truncated);

        let damaged = &compressed[..compressed.len() - 12];
        let read = read_page(
            BufReader::new(MultiGzDecoder::new(Cursor::new(damaged.to_vec()))),
            &filter,
            None,
            100,
        );
        assert!(read.truncated);
    }
}
