//! What the sidebar asks git, and how git's answers are read. Every question runs git
//! itself, so the sidebar agrees with the command line on what is ignored, renamed or
//! changed; the reading of an answer is kept apart from the asking, so it is tested on
//! captured output.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How many commits one `git log` reads; the next page is read as the cursor nears the end.
pub const LOG_PAGE: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    Modified,
    Added,
    Deleted,
    Renamed,
}

impl Change {
    pub fn letter(self) -> &'static str {
        match self {
            Change::Modified => "M",
            Change::Added => "A",
            Change::Deleted => "D",
            Change::Renamed => "R",
        }
    }

    /// What one `git status` entry says of a file, both columns read as one: staged or not
    /// makes no difference to a list of what changed.
    fn from_status(index: u8, worktree: u8) -> Self {
        let either = |code: u8| index == code || worktree == code;
        if either(b'?') {
            Change::Added
        } else if either(b'D') {
            Change::Deleted
        } else if either(b'R') {
            Change::Renamed
        } else if either(b'A') || either(b'C') {
            Change::Added
        } else {
            Change::Modified
        }
    }

    /// What a `--name-status` letter says of a file in a commit.
    fn from_name_status(status: u8) -> Self {
        match status {
            b'A' | b'C' => Change::Added,
            b'D' => Change::Deleted,
            b'R' => Change::Renamed,
            _ => Change::Modified,
        }
    }
}

/// A file git reports as changed, in the working tree or in a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub change: Change,
    /// Where a renamed file came from, relative to the repository's top: a diff asked with
    /// both paths reads as a rename, with one alone as a new file.
    pub from: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    /// The committer's date, which a rebase renews, so ages read in the list's order.
    pub time: i64,
    pub subject: String,
    /// The file the commit was reached by (a file's history, a blame), relative to the
    /// repository's top and named as it was in that commit.
    pub file: Option<String>,
    /// Where that file came from when the commit renamed it.
    pub file_from: Option<String>,
}

/// Who last changed a line, and in which commit; no commit when the change is not
/// committed yet.
#[derive(Debug, PartialEq, Eq)]
pub struct Blame {
    pub author: String,
    pub commit: Option<Commit>,
}

/// A line to blame: the file, its line from 0, and the buffer's text, which is what the line
/// numbers count.
pub struct BlameRequest {
    pub path: PathBuf,
    pub line: usize,
    pub contents: String,
}

pub type Answer<T> = Result<T, String>;

/// A pathspec naming one path from the repository's top, taken literally.
pub fn pathspec(top_relative: &str) -> String {
    format!(":(top,literal){top_relative}")
}

/// Where `root` sits inside its repository, as git spells it: `""` or `"a/b/"`.
pub fn prefix(root: &Path) -> Answer<String> {
    let prefix = run(root, &["rev-parse", "--show-prefix"])?;
    Ok(String::from_utf8_lossy(&prefix).trim_end().to_string())
}

/// What `git status` names below `root`, ignore rules included.
pub fn status(root: &Path) -> Answer<Vec<ChangedFile>> {
    let prefix = prefix(root)?;
    let status = run(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    parse_status(&status, &prefix, root)
}

/// One page of history, newest first, starting `skip` commits down from HEAD.
pub fn log(root: &Path, skip: usize) -> Answer<Vec<Commit>> {
    let skip = format!("--skip={skip}");
    let count = format!("--max-count={LOG_PAGE}");
    let log = run(root, &[LOG, "-z", "--abbrev=7", LOG_FORMAT, &skip, &count])?;
    parse_log(&log)
}

/// The whole history of one file below `root`, followed across renames, each commit
/// carrying the name the file had in it. Read whole: `--follow` miscounts `--skip`.
pub fn file_log(root: &Path, path: &Path) -> Answer<Vec<Commit>> {
    let path = path.to_string_lossy();
    let log = run(
        root,
        &[
            LOG,
            "-z",
            "--abbrev=7",
            LOG_FORMAT,
            "--follow",
            "--name-status",
            "--",
            &path,
        ],
    )?;
    parse_log(&log)
}

const LOG: &str = "log";
const LOG_FORMAT: &str = "--format=%H%x1f%h%x1f%ct%x1f%s";

/// What one commit changed below `root` (a merge against its first parent), and where the
/// root sits inside the repository, which the diffs are then asked with.
pub fn commit_files(root: &Path, hash: &str) -> Answer<(String, Vec<ChangedFile>)> {
    let prefix = prefix(root)?;
    let listing = run(
        root,
        &[
            "show",
            "--format=",
            "--name-status",
            "-z",
            "-M",
            "--diff-merges=first-parent",
            "--no-color",
            hash,
        ],
    )?;
    let files = parse_name_status(&listing, &prefix, root)?;
    Ok((prefix, files))
}

/// A commit's patch narrowed to `pathspecs`, under the commit's own header, as `git show`
/// prints it.
pub fn show(root: &Path, hash: &str, pathspecs: &[String]) -> Answer<String> {
    let mut args = vec![
        "show",
        "--format=medium",
        "--no-color",
        "--no-ext-diff",
        "-M",
        "--diff-merges=first-parent",
        hash,
        "--",
    ];
    args.extend(pathspecs.iter().map(String::as_str));
    let patch = run(root, &args)?;
    Ok(String::from_utf8_lossy(&patch).into_owned())
}

/// Blames one line of the buffer's text as git would the file with that text in it: a line
/// changed since the last commit belongs to no commit.
pub fn blame(root: &Path, request: &BlameRequest) -> Answer<Blame> {
    let range = format!("{0},{0}", request.line + 1);
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "blame",
            "--porcelain",
            "-L",
            &range,
            "--contents",
            "-",
            "--",
        ])
        .arg(&request.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("git: {err}"))?;
    // git reads the whole text before it answers, and the answer for one line is short, so
    // writing it all first cannot wait on a full pipe.
    let written = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(request.contents.as_bytes()),
        None => Ok(()),
    };
    let output = child
        .wait_with_output()
        .map_err(|err| format!("git blame: {err}"))?;
    // A git that gave up before reading says why; the broken pipe it leaves behind does not.
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().next().unwrap_or("failed").to_string();
        return Err(format!("git blame: {reason}"));
    }
    written.map_err(|err| format!("git blame: {err}"))?;
    parse_blame(&String::from_utf8_lossy(&output.stdout))
}

fn run(dir: &Path, args: &[&str]) -> Answer<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|err| format!("git: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().next().unwrap_or("failed").to_string();
        return Err(format!("git {}: {reason}", args[0]));
    }
    Ok(output.stdout)
}

/// Reads `git status --porcelain=v1 -z`: paths are relative to the repository's top, and
/// only those under `prefix`, the root's place in it, are kept, as paths below `root`.
fn parse_status(status: &[u8], prefix: &str, root: &Path) -> Answer<Vec<ChangedFile>> {
    let mut files = Vec::new();
    let mut entries = status.split(|byte| *byte == 0);
    while let Some(entry) = entries.next() {
        if entry.is_empty() {
            continue;
        }
        if entry.len() < 4 {
            return Err(format!(
                "git status: unreadable entry {:?}",
                String::from_utf8_lossy(entry)
            ));
        }
        let change = Change::from_status(entry[0], entry[1]);
        // A rename or a copy carries the path it came from as the next entry.
        let renamed = matches!(entry[0], b'R' | b'C') || matches!(entry[1], b'R' | b'C');
        let from = if renamed {
            entries
                .next()
                .map(|from| String::from_utf8_lossy(from).into_owned())
        } else {
            None
        };
        let path = String::from_utf8_lossy(&entry[3..]);
        let Some(inside) = path.strip_prefix(prefix) else {
            continue;
        };
        files.push(ChangedFile {
            path: root.join(inside),
            change,
            from,
        });
    }
    Ok(files)
}

/// Reads a `git log -z` in `LOG_FORMAT`, with or without `--name-status`: with it, a
/// commit's record is followed by the followed file's letter, its source when renamed,
/// and its path.
fn parse_log(log: &[u8]) -> Answer<Vec<Commit>> {
    let mut commits: Vec<Commit> = Vec::new();
    // A --name-status line follows its commit's record after a newline.
    let mut records = log
        .split(|byte| *byte == 0)
        .map(|record| record.strip_prefix(b"\n").unwrap_or(record));
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if !record.contains(&0x1f) {
            let from = if matches!(record[0], b'R' | b'C') {
                records.next()
            } else {
                None
            };
            let (Some(commit), Some(path)) = (commits.last_mut(), records.next()) else {
                return Err(format!(
                    "git log: unreadable entry {:?}",
                    String::from_utf8_lossy(record)
                ));
            };
            commit.file = Some(String::from_utf8_lossy(path).into_owned());
            commit.file_from = from.map(|from| String::from_utf8_lossy(from).into_owned());
            continue;
        }
        let record = String::from_utf8_lossy(record);
        let mut fields = record.splitn(4, '\x1f');
        let (Some(hash), Some(short), Some(time), Some(subject)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(format!("git log: unreadable entry {record:?}"));
        };
        let Ok(time) = time.parse() else {
            return Err(format!("git log: unreadable date {time:?}"));
        };
        commits.push(Commit {
            hash: hash.to_string(),
            short: short.to_string(),
            time,
            subject: subject.to_string(),
            file: None,
            file_from: None,
        });
    }
    Ok(commits)
}

/// Reads a `--name-status -z` listing: a letter, the source when renamed or copied, and
/// the path; only paths under `prefix` are kept, as paths below `root`.
fn parse_name_status(listing: &[u8], prefix: &str, root: &Path) -> Answer<Vec<ChangedFile>> {
    let mut files = Vec::new();
    let mut entries = listing.split(|byte| *byte == 0);
    while let Some(status) = entries.next() {
        let Some(letter) = status.first().copied() else {
            continue;
        };
        let from = if matches!(letter, b'R' | b'C') {
            let Some(from) = entries.next() else {
                return Err("git show: a rename without its source".into());
            };
            Some(String::from_utf8_lossy(from).into_owned())
        } else {
            None
        };
        let Some(path) = entries.next() else {
            return Err(format!(
                "git show: {} names no file",
                String::from_utf8_lossy(status)
            ));
        };
        let path = String::from_utf8_lossy(path);
        let Some(inside) = path.strip_prefix(prefix) else {
            continue;
        };
        files.push(ChangedFile {
            path: root.join(inside),
            change: Change::from_name_status(letter),
            from,
        });
    }
    Ok(files)
}

/// Reads one line of `git blame --porcelain`: the hash, then headers up to the tab that
/// starts the line's text. An all-zero hash is a line no commit holds.
fn parse_blame(answer: &str) -> Answer<Blame> {
    let mut lines = answer.lines();
    let hash = lines
        .next()
        .and_then(|first| first.split(' ').next())
        .ok_or("git blame: no answer")?;
    let mut author = "";
    let mut time = "";
    let mut subject = "";
    let mut file = "";
    for line in lines.take_while(|line| !line.starts_with('\t')) {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "author" => author = value,
            "committer-time" => time = value,
            "summary" => subject = value,
            "filename" => file = value,
            _ => {}
        }
    }
    let commit = if hash.bytes().all(|byte| byte == b'0') {
        None
    } else {
        let Ok(time) = time.parse() else {
            return Err(format!("git blame: unreadable date {time:?}"));
        };
        Some(Commit {
            hash: hash.to_string(),
            short: hash.chars().take(7).collect(),
            time,
            subject: subject.to_string(),
            file: Some(file.to_string()),
            file_from: None,
        })
    };
    Ok(Blame {
        author: author.to_string(),
        commit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/repo/sub")
    }

    #[test]
    fn status_keeps_what_is_below_the_root_and_reads_a_rename() {
        let status =
            b" M sub/a.txt\0?? sub/new.txt\0R  sub/moved.txt\0sub/old.txt\0 D other/gone.txt\0";
        let files = parse_status(status, "sub/", &root()).unwrap();
        assert_eq!(
            files,
            vec![
                ChangedFile {
                    path: root().join("a.txt"),
                    change: Change::Modified,
                    from: None,
                },
                ChangedFile {
                    path: root().join("new.txt"),
                    change: Change::Added,
                    from: None,
                },
                ChangedFile {
                    path: root().join("moved.txt"),
                    change: Change::Renamed,
                    from: Some("sub/old.txt".into()),
                },
            ]
        );
    }

    #[test]
    fn status_refuses_an_entry_it_cannot_read() {
        assert!(parse_status(b"M\0", "", &root()).is_err());
    }

    #[test]
    fn log_reads_a_page_of_commits() {
        let log = b"abc123full\x1fabc123f\x1f1700000000\x1ffirst: subject\0def456full\x1fdef456f\x1f1699999999\x1fsecond\0";
        let commits = parse_log(log).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short, "abc123f");
        assert_eq!(commits[0].time, 1700000000);
        assert_eq!(commits[0].subject, "first: subject");
        assert_eq!(commits[0].file, None);
        assert_eq!(commits[1].subject, "second");
    }

    #[test]
    fn a_file_log_names_the_file_as_each_commit_had_it() {
        let log = b"h1\x1fh1\x1f10\x1fmoved it\0\nR100\0old/name.rs\0new/name.rs\0h2\x1fh2\x1f9\x1fwrote it\0\nA\0old/name.rs\0";
        let commits = parse_log(log).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].file.as_deref(), Some("new/name.rs"));
        assert_eq!(commits[0].file_from.as_deref(), Some("old/name.rs"));
        assert_eq!(commits[1].file.as_deref(), Some("old/name.rs"));
        assert_eq!(commits[1].file_from, None);
    }

    #[test]
    fn log_refuses_an_unreadable_date() {
        assert!(parse_log(b"h\x1fh\x1fyesterday\x1fs\0").is_err());
    }

    #[test]
    fn a_commit_listing_reads_letters_sources_and_paths() {
        let listing = b"M\0sub/a.rs\0R090\0sub/from.rs\0sub/to.rs\0A\0elsewhere.rs\0";
        let files = parse_name_status(listing, "sub/", &root()).unwrap();
        assert_eq!(
            files,
            vec![
                ChangedFile {
                    path: root().join("a.rs"),
                    change: Change::Modified,
                    from: None,
                },
                ChangedFile {
                    path: root().join("to.rs"),
                    change: Change::Renamed,
                    from: Some("sub/from.rs".into()),
                },
            ]
        );
    }

    #[test]
    fn blame_reads_the_commit_of_a_line() {
        let answer = "4da9363b7cb5cab71da15e39617e64a4a4127c5a 10 10 1\n\
            author Santiago Corredoira\n\
            author-mail <s@example.com>\n\
            committer-time 1786727071\n\
            summary docs: the subject\n\
            filename CLAUDE.md\n\
            \tthe line itself\n";
        let blame = parse_blame(answer).unwrap();
        assert_eq!(blame.author, "Santiago Corredoira");
        let commit = blame.commit.unwrap();
        assert_eq!(commit.short, "4da9363");
        assert_eq!(commit.time, 1786727071);
        assert_eq!(commit.subject, "docs: the subject");
        assert_eq!(commit.file.as_deref(), Some("CLAUDE.md"));
    }

    #[test]
    fn blame_of_an_uncommitted_line_has_no_commit() {
        let answer = "0000000000000000000000000000000000000000 3 3 1\n\
            author Not Committed Yet\n\
            committer-time 1786727071\n\
            summary Version of CLAUDE.md from standard input\n\
            filename CLAUDE.md\n\
            \tnew line\n";
        let blame = parse_blame(answer).unwrap();
        assert_eq!(blame.author, "Not Committed Yet");
        assert_eq!(blame.commit, None);
    }
}
