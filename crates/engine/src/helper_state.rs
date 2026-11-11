//! Crash-safe record of the pids isolated by the elevated helper.
//!
//! The list lives in `%APPDATA%` so uninstall and `--restore` can undo the
//! helper's changes even after the helper process was killed. One decimal pid
//! per line keeps the format readable and dependency-free.

use std::fs;
use std::path::PathBuf;

/// Path of the helper's isolation journal.
fn state_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(|appdata| {
            PathBuf::from(appdata)
                .join("flux-fps")
                .join("helper-state.txt")
        })
        .unwrap_or_else(|| PathBuf::from("flux-fps-helper-state.txt"))
}

/// Parses a journal; unparseable lines are ignored so a truncated write can
/// never prevent the app from starting.
fn parse_pids(text: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect()
}

/// Loads the pids the helper last isolated.
pub fn load() -> Vec<u32> {
    match fs::read_to_string(state_path()) {
        Ok(text) => parse_pids(&text),
        Err(_) => Vec::new(),
    }
}

/// Rewrites the journal. Failures are non-fatal: the in-memory set remains
/// authoritative for the running helper.
pub fn save(pids: impl IntoIterator<Item = u32>) {
    let text: String = pids.into_iter().map(|pid| format!("{pid}\n")).collect();
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, text);
}

/// Empties the journal after a successful restore.
pub fn clear() {
    save(std::iter::empty::<u32>());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_lines_and_skips_garbage() {
        assert_eq!(parse_pids("12\n34\n"), vec![12, 34]);
        assert_eq!(parse_pids("12\nnope\n-1\n34"), vec![12, 34]);
        assert!(parse_pids("").is_empty());
        assert!(parse_pids("4294967296\n").is_empty());
    }

    #[test]
    fn journal_lines_are_newline_terminated() {
        let text: String = [1u32, 2]
            .into_iter()
            .map(|pid| format!("{pid}\n"))
            .collect();
        assert_eq!(text, "1\n2\n");
        assert_eq!(parse_pids(&text), vec![1, 2]);
    }
}
