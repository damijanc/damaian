use crate::audit::AuditLog;
use crate::error::{ClientError, Result};
use crate::secret_scanner::SecretScanner;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFileStatus {
    pub path: String,
    pub raw: String,
    pub staged: bool,
    pub worktree: bool,
    pub untracked: bool,
    pub conflicted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub clean: bool,
    pub exit_code: i32,
    pub raw: String,
    pub stderr: String,
    pub files: Vec<GitFileStatus>,
}

/// A GitHub repository parsed from the `origin` remote.
///
/// The fields are private so the only way to obtain one is
/// [`parse_github_remote`], which validates both parts: the remote is
/// repository-controlled input, and [`GitHubRepo::url`] must never carry
/// anything but a checked owner and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepo {
    owner: String,
    name: String,
}

impl GitHubRepo {
    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Rebuilt from the validated parts; the raw remote string is never used.
    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }
}

/// What the repo bar shows: branch or short SHA, working-tree line counts
/// against `HEAD`, and the GitHub origin when there is one.
///
/// A folder that is not a git repository yields the default (all `None`/0)
/// rather than an error, so callers can render "folder only" without a
/// special case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSummary {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub insertions: u64,
    pub deletions: u64,
    pub github: Option<GitHubRepo>,
}

/// Parses an `origin` URL into a GitHub owner and repository name.
///
/// Accepts only `https://github.com/o/r`, `git@github.com:o/r` and
/// `ssh://git@github.com/o/r`, each with an optional `.git` suffix. Owner and
/// name must match `[A-Za-z0-9_.-]+` and may not be `.` or `..`. Anything
/// else (another host, a port, other userinfo, extra path segments, a query)
/// yields `None`.
pub fn parse_github_remote(url: &str) -> Option<GitHubRepo> {
    let path = [
        "https://github.com/",
        "git@github.com:",
        "ssh://git@github.com/",
    ]
    .iter()
    .find_map(|prefix| url.strip_prefix(prefix))?;
    let (owner, name) = path.split_once('/')?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    (valid(owner) && valid(name)).then(|| GitHubRepo {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// Untracked files larger than this are not counted: reading them to count
/// lines would make every repo-bar refresh pay for build artefacts.
const UNTRACKED_COUNT_MAX_BYTES: u64 = 1024 * 1024;
/// Git's own binary heuristic: a NUL byte in the first 8 KiB.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

fn git_output(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ok(Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?)
}

/// Trimmed stdout of a successful command, or `None` when it failed or was empty.
fn git_line(root: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = git_output(root, args)?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((output.status.success() && !line.is_empty()).then_some(line))
}

/// `(insertions, deletions)` from `git diff --numstat`, skipping binary rows.
fn sum_numstat(raw: &str) -> (u64, u64) {
    raw.lines().fold((0, 0), |(ins, del), line| {
        let mut fields = line.split('\t');
        match (
            fields.next().and_then(|f| f.parse::<u64>().ok()),
            fields.next().and_then(|f| f.parse::<u64>().ok()),
        ) {
            (Some(added), Some(removed)) => (ins + added, del + removed),
            _ => (ins, del),
        }
    })
}

/// Lines in a working-tree text file, counted the way numstat counts an
/// added file. Symlinks, non-files, oversized and binary files count as 0.
fn count_text_lines(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_file() || meta.len() > UNTRACKED_COUNT_MAX_BYTES {
        return 0;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return 0;
    };
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return 0;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    match bytes.last() {
        Some(b'\n') | None => newlines,
        Some(_) => newlines + 1,
    }
}

/// Sums [`count_text_lines`] over the NUL-separated paths `ls-files -z` printed.
fn count_listed_files(root: &Path, raw: &[u8]) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let paths: std::collections::BTreeSet<&[u8]> =
        raw.split(|&b| b == 0).filter(|p| !p.is_empty()).collect();
    paths
        .into_iter()
        .map(|p| count_text_lines(&root.join(std::ffi::OsStr::from_bytes(p))))
        .sum()
}

#[derive(Debug, Clone)]
pub struct GitService {
    audit_log: AuditLog,
    scanner: SecretScanner,
}

impl GitService {
    pub fn new(audit_log: AuditLog, scanner: SecretScanner) -> Self {
        Self { audit_log, scanner }
    }

    pub fn status(&self, root_path: impl AsRef<Path>) -> Result<GitStatus> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root_path.as_ref())
            .arg("status")
            .arg("--porcelain=v1")
            .output()?;
        let exit_code = output.status.code().unwrap_or(-1);
        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let status = GitStatus {
            clean: exit_code == 0 && raw.trim().is_empty(),
            exit_code,
            files: if exit_code == 0 {
                parse_porcelain(&raw)
            } else {
                Vec::new()
            },
            raw,
            stderr,
        };
        self.audit_log.record(
            "git_status_read",
            &[
                ("actor", "system".to_string()),
                (
                    "resourcePath",
                    root_path.as_ref().to_string_lossy().to_string(),
                ),
                (
                    "status",
                    if exit_code == 0 { "complete" } else { "failed" }.to_string(),
                ),
                ("exitCode", exit_code.to_string()),
                ("fileCount", status.files.len().to_string()),
            ],
        )?;
        Ok(status)
    }

    /// Branch, short `HEAD`, working-tree line counts against `HEAD` and the
    /// GitHub origin, for the repo bar.
    ///
    /// A folder that is not a git repository returns `GitSummary::default()`
    /// rather than an error. The raw `origin` URL is parsed and dropped: only
    /// the validated [`GitHubRepo`] leaves this function, and it is not audited.
    pub fn summary(&self, root_path: impl AsRef<Path>) -> Result<GitSummary> {
        let root = root_path.as_ref();
        let probe = git_output(root, &["rev-parse", "--is-inside-work-tree"])?;
        let exit_code = probe.status.code().unwrap_or(-1);
        let summary = if probe.status.success() {
            self.read_summary(root)?
        } else {
            GitSummary::default()
        };
        self.audit_log.record(
            "git_status_read",
            &[
                ("actor", "system".to_string()),
                ("resourcePath", root.to_string_lossy().to_string()),
                (
                    "status",
                    if exit_code == 0 { "complete" } else { "failed" }.to_string(),
                ),
                ("exitCode", exit_code.to_string()),
                ("scope", "summary".to_string()),
            ],
        )?;
        Ok(summary)
    }

    fn read_summary(&self, root: &Path) -> Result<GitSummary> {
        let branch = git_line(root, &["symbolic-ref", "--short", "-q", "HEAD"])?;
        let head = git_line(root, &["rev-parse", "--short", "--verify", "-q", "HEAD"])?;
        let (insertions, deletions) = if head.is_some() {
            let numstat = git_output(
                root,
                &[
                    "diff",
                    "--numstat",
                    "--no-ext-diff",
                    "--no-textconv",
                    "HEAD",
                ],
            )?;
            let (ins, del) = if numstat.status.success() {
                sum_numstat(&String::from_utf8_lossy(&numstat.stdout))
            } else {
                (0, 0)
            };
            let untracked =
                git_output(root, &["ls-files", "-z", "--others", "--exclude-standard"])?;
            (ins + count_listed_files(root, &untracked.stdout), del)
        } else {
            // No commits yet: nothing to diff against, so everything in the
            // index or the working tree is an insertion.
            let files = git_output(
                root,
                &[
                    "ls-files",
                    "-z",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                ],
            )?;
            (count_listed_files(root, &files.stdout), 0)
        };
        let github = git_line(root, &["remote", "get-url", "origin"])?
            .and_then(|url| parse_github_remote(&url));
        Ok(GitSummary {
            branch,
            head,
            insertions,
            deletions,
            github,
        })
    }

    pub fn diff(&self, root_path: impl AsRef<Path>, staged: bool) -> Result<String> {
        let mut command = Command::new("git");
        command.arg("-C").arg(root_path.as_ref()).arg("diff");
        if staged {
            command.arg("--staged");
        }
        let output = command.output()?;
        let exit_code = output.status.code().unwrap_or(-1);
        self.audit_log.record(
            "git_diff_read",
            &[
                ("actor", "system".to_string()),
                (
                    "resourcePath",
                    root_path.as_ref().to_string_lossy().to_string(),
                ),
                (
                    "status",
                    if exit_code == 0 { "complete" } else { "failed" }.to_string(),
                ),
                ("exitCode", exit_code.to_string()),
                ("staged", staged.to_string()),
            ],
        )?;
        if exit_code != 0 {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let raw_diff = String::from_utf8_lossy(&output.stdout);
        Ok(self.scanner.redact(raw_diff.as_ref()).text)
    }

    pub fn suggest_commit_message(&self, summary: &str, changed_files: &[String]) -> String {
        let normalized = summary.trim().trim_end_matches(['.', '!', '?']);
        if normalized.is_empty() {
            "chore: update workspace changes".to_string()
        } else if changed_files.len() == 1 {
            format!("chore: {} {normalized}", changed_files[0])
        } else {
            format!("chore: {normalized}")
        }
    }
}

fn parse_porcelain(raw: &str) -> Vec<GitFileStatus> {
    raw.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let index_status = line.as_bytes().first().copied().unwrap_or(b' ');
            let worktree_status = line.as_bytes().get(1).copied().unwrap_or(b' ');
            let path = line.get(3..).unwrap_or_default();
            let path = path.split(" -> ").last().unwrap_or(path).to_string();
            let pair = format!("{}{}", index_status as char, worktree_status as char);
            GitFileStatus {
                path,
                raw: line.to_string(),
                staged: index_status != b' ' && index_status != b'?',
                worktree: worktree_status != b' ',
                untracked: index_status == b'?' && worktree_status == b'?',
                conflicted: ["AA", "DD", "AU", "UD", "UA", "DU", "UU"].contains(&pair.as_str()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn repo(owner: &str, name: &str) -> Option<GitHubRepo> {
        Some(GitHubRepo {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    #[test]
    fn github_remote_accepts_https_with_and_without_dot_git() {
        assert_eq!(
            parse_github_remote("https://github.com/damijanc/damaian-client"),
            repo("damijanc", "damaian-client")
        );
        assert_eq!(
            parse_github_remote("https://github.com/damijanc/damaian-client.git"),
            repo("damijanc", "damaian-client")
        );
    }

    #[test]
    fn github_remote_accepts_scp_style_ssh_with_and_without_dot_git() {
        assert_eq!(
            parse_github_remote("git@github.com:o_1/r.x-y"),
            repo("o_1", "r.x-y")
        );
        assert_eq!(
            parse_github_remote("git@github.com:o_1/r.x-y.git"),
            repo("o_1", "r.x-y")
        );
    }

    #[test]
    fn github_remote_accepts_ssh_url_with_and_without_dot_git() {
        assert_eq!(
            parse_github_remote("ssh://git@github.com/o/r"),
            repo("o", "r")
        );
        assert_eq!(
            parse_github_remote("ssh://git@github.com/o/r.git"),
            repo("o", "r")
        );
    }

    #[test]
    fn github_remote_url_is_rebuilt_from_the_parsed_parts() {
        let parsed = parse_github_remote("git@github.com:damijanc/damaian-client.git").unwrap();
        assert_eq!(parsed.owner(), "damijanc");
        assert_eq!(parsed.name(), "damaian-client");
        assert_eq!(parsed.url(), "https://github.com/damijanc/damaian-client");
    }

    #[test]
    fn github_remote_rejects_other_hosts_and_lookalikes() {
        for url in [
            "https://gitlab.com/o/r",
            "https://github.com.evil.example/o/r",
            "git@github.com.evil.example:o/r",
            "ssh://git@github.com.evil.example/o/r",
            "https://evil.example/github.com/o/r",
            "https://GitHub.com/o/r",
            "http://github.com/o/r",
            "https://github.com:443/o/r",
            "ssh://git@github.com:22/o/r",
            "file:///tmp/github.com/o/r",
            "/srv/git/o/r.git",
            "",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url} should be rejected");
        }
    }

    #[test]
    fn github_remote_rejects_userinfo_other_than_git() {
        for url in [
            "https://user:pw@github.com/o/r",
            "https://git@github.com/o/r",
            "ssh://user@github.com/o/r",
            "ssh://github.com/o/r",
            "user@github.com:o/r",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url} should be rejected");
        }
    }

    #[test]
    fn github_remote_rejects_extra_or_missing_path_segments() {
        for url in [
            "https://github.com/o/r/tree/main",
            "https://github.com/o/r/",
            "https://github.com/o",
            "https://github.com/",
            "git@github.com:o/r/x.git",
            "ssh://git@github.com/o/r/x",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url} should be rejected");
        }
    }

    #[test]
    fn github_remote_rejects_empty_or_dot_parts() {
        for url in [
            "https://github.com//r",
            "git@github.com:/r",
            "https://github.com/o/",
            "https://github.com/o/.git",
            "https://github.com/../r",
            "https://github.com/o/..",
            "https://github.com/./r",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url} should be rejected");
        }
    }

    #[test]
    fn github_remote_rejects_characters_outside_the_allowed_set() {
        for url in [
            "https://github.com/o/r?tab=readme",
            "https://github.com/o/r#readme",
            "https://github.com/o%2F/r",
            "https://github.com/o/r name",
            "https://github.com/o/r\n",
            " https://github.com/o/r",
            "https://github.com/ö/r",
            "git@github.com:o/r;rm -rf",
            "https://github.com/o/r\\x",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url:?} should be rejected");
        }
    }

    struct Fixture {
        repository: PathBuf,
        data_dir: PathBuf,
        service: GitService,
    }

    impl Fixture {
        /// The audit log lives beside the repository, not in it, so it never
        /// shows up as an untracked file in the counts under test.
        fn new(name: &str) -> Self {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should work")
                .as_nanos();
            let base = std::env::temp_dir().join(format!(
                "damaian-git-summary-{name}-{now}-{}",
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let repository = base.join("repo");
            let data_dir = base.join("data");
            fs::create_dir_all(&repository).unwrap();
            let scanner = SecretScanner::new(Vec::new());
            let service = GitService::new(AuditLog::new(&data_dir, true, scanner.clone()), scanner);
            Self {
                repository,
                data_dir,
                service,
            }
        }

        fn git_init(&self) {
            self.git(&["init", "--quiet", "--initial-branch=main"]);
            self.git(&["config", "user.email", "test@damaian.local"]);
            self.git(&["config", "user.name", "Damaian Test"]);
            self.git(&["config", "commit.gpgsign", "false"]);
            self.git(&["config", "gc.auto", "0"]);
            self.git(&["config", "maintenance.auto", "false"]);
        }

        fn commit_all(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "--quiet", "-m", message]);
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.repository)
                .args(args)
                .output()
                .expect("git should run");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
            let path = self.repository.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        fn summary(&self) -> GitSummary {
            self.service
                .summary(&self.repository)
                .expect("summary should not error")
        }

        fn audit(&self) -> String {
            fs::read_to_string(self.data_dir.join("audit").join("events.jsonl")).unwrap_or_default()
        }
    }

    #[test]
    fn git_service_summary_counts_edits_and_untracked_lines_on_a_branch() {
        let fixture = Fixture::new("branch");
        fixture.git_init();
        fixture.write("a.txt", "one\ntwo\nthree\n");
        fixture.write("b.txt", "keep\n");
        fixture.commit_all("initial");
        fixture.git(&["checkout", "--quiet", "-b", "feature/repo-bar"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "git@github.com:damijanc/damaian-client.git",
        ]);

        // a.txt: three lines become one changed line -> +1 -3.
        fixture.write("a.txt", "ONE\n");
        // Untracked: three lines, the last without a trailing newline.
        fixture.write("new/c.txt", "x\ny\nz");

        let summary = fixture.summary();
        assert_eq!(summary.branch.as_deref(), Some("feature/repo-bar"));
        let head = fixture.git(&["rev-parse", "--short", "HEAD"]);
        assert_eq!(summary.head.as_deref(), Some(head.as_str()));
        assert_eq!(summary.insertions, 1 + 3);
        assert_eq!(summary.deletions, 3);
        let github = summary.github.expect("a GitHub origin should be parsed");
        assert_eq!(github.url(), "https://github.com/damijanc/damaian-client");

        let audit = fixture.audit();
        assert!(audit.contains("git_status_read"), "{audit}");
        assert!(
            !audit.contains("git@github.com"),
            "the raw remote is not audited"
        );
    }

    #[test]
    fn git_service_summary_reports_no_branch_on_a_detached_head() {
        let fixture = Fixture::new("detached");
        fixture.git_init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("initial");
        let head = fixture.git(&["rev-parse", "--short", "HEAD"]);
        fixture.git(&["checkout", "--quiet", "--detach", "HEAD"]);

        let summary = fixture.summary();
        assert_eq!(summary.branch, None);
        assert_eq!(summary.head.as_deref(), Some(head.as_str()));
        assert_eq!((summary.insertions, summary.deletions), (0, 0));
    }

    #[test]
    fn git_service_summary_counts_every_file_as_an_insertion_with_no_commits() {
        let fixture = Fixture::new("unborn");
        fixture.git_init();
        fixture.write("staged.txt", "a\nb\n");
        fixture.git(&["add", "staged.txt"]);
        fixture.write("untracked.txt", "c\n");

        let summary = fixture.summary();
        assert_eq!(summary.branch.as_deref(), Some("main"));
        assert_eq!(summary.head, None);
        assert_eq!(summary.insertions, 3);
        assert_eq!(summary.deletions, 0);
        assert_eq!(summary.github, None);
    }

    #[test]
    fn git_service_summary_is_empty_for_a_folder_that_is_not_a_git_repository() {
        let fixture = Fixture::new("not-git");
        fixture.write("a.txt", "one\ntwo\n");

        assert_eq!(fixture.summary(), GitSummary::default());
        assert!(fixture.audit().contains("git_status_read"));
    }

    #[test]
    fn git_service_summary_skips_binary_and_oversized_files() {
        let fixture = Fixture::new("binary");
        fixture.git_init();
        fixture.write("image.bin", [0u8, 1, 2, 3, b'\n', 0]);
        fixture.write("a.txt", "one\n");
        fixture.commit_all("initial");

        // A tracked binary edit shows as `-\t-` in numstat and is skipped.
        fixture.write("image.bin", [0u8, 9, 9, b'\n', b'\n', 0, 7]);
        // Untracked: a NUL in the first 8 KiB, and a text file over 1 MiB.
        let mut nul_early = vec![b'x'; 100];
        nul_early[50] = 0;
        nul_early.extend_from_slice(b"\nline\n");
        fixture.write("untracked.bin", nul_early);
        fixture.write("huge.txt", "y\n".repeat(600 * 1024));
        // One untracked text line that should count.
        fixture.write("small.txt", "z\n");

        let summary = fixture.summary();
        assert_eq!(summary.insertions, 1);
        assert_eq!(summary.deletions, 0);
    }

    #[test]
    fn git_service_summary_ignores_a_non_github_origin() {
        let fixture = Fixture::new("lookalike");
        fixture.git_init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("initial");
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com.evil.example/o/r",
        ]);

        assert_eq!(fixture.summary().github, None);
    }
}
