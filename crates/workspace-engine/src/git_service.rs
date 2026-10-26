use crate::audit::AuditLog;
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::path_policy::PathPolicy;
use crate::secret_scanner::SecretScanner;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFileStatus {
    pub path: String,
    pub raw: String,
    pub staged: bool,
    pub worktree: bool,
    pub untracked: bool,
    pub conflicted: bool,
    /// The source path of a rename, when Git reported one. The commit
    /// preparation represents that source as a deletion candidate too
    /// (spec 35 §5.2), so the old path cannot be silently left behind.
    pub renamed_from: Option<String>,
    /// The source path of a copy. A copy leaves the source in place, so it
    /// produces no deletion candidate.
    pub copied_from: Option<String>,
}

/// `git status` as a read sees it (spec 60 §5.4): entries naming a restricted
/// path are dropped from `files` and `raw` alike, and `clean` is judged after
/// that, so no field says a restricted file changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub clean: bool,
    pub exit_code: i32,
    pub raw: String,
    pub stderr: String,
    pub files: Vec<GitFileStatus>,
    /// Entries past the read caps, left out of `files` and `raw`.
    pub omitted: usize,
}

/// The read caps a git read honours, spec 47 §5.5's `max_read_lines` and
/// `max_file_bytes`; whichever binds first cuts the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitReadCaps {
    pub max_lines: usize,
    pub max_bytes: u64,
}

impl GitReadCaps {
    pub fn from_config(config: &Config) -> Self {
        Self {
            max_lines: config.max_read_lines,
            max_bytes: config.max_file_bytes,
        }
    }
}

/// What `read_git_diff` asks of [`GitService::diff_in`] beyond the root
/// (spec 60 §5.2). The default is the diff `read_git_diff` always returned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffOptions {
    /// The index against `HEAD` (or the merge base) instead of the working tree.
    pub staged: bool,
    /// Limit the diff to these root-relative paths or directories.
    pub paths: Vec<String>,
    /// `--stat` instead of hunks.
    pub stat: bool,
    /// Compare against `merge-base(base, HEAD)` instead of the index.
    pub base: Option<String>,
}

/// One reachable worktree as `list_worktrees` reports it (spec 60 §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeListingEntry {
    /// Repository-relative path, absolute when outside the repository root.
    pub path: String,
    /// The branch checked out; `None` when detached.
    pub branch: Option<String>,
    /// Short `HEAD`.
    pub head: Option<String>,
    /// Changed files, restricted paths not counted.
    pub changed_files: usize,
    /// Commits this worktree is ahead of `base`; `None` when there is no base.
    pub ahead: Option<usize>,
    /// Commits this worktree is behind `base`.
    pub behind: Option<usize>,
}

/// `list_worktrees`: every reachable worktree, capped at 20 with the rest
/// counted but not listed (spec 60 §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeListing {
    /// The effective `base` the ahead/behind counts are against, or `None`
    /// when the main worktree is detached and no `base` was given.
    pub base: Option<String>,
    pub entries: Vec<WorktreeListingEntry>,
    /// Reachable worktrees past the cap, counted but not listed.
    pub omitted: usize,
}

/// The most `list_worktrees` lists; the rest are counted, not enumerated, so a
/// repository with many worktrees cannot turn one read into dozens of git
/// processes (spec 60 §5.2).
const WORKTREES_LISTED_CAP: usize = 20;

/// Refuses a `base` that is not a branch name under `git check-ref-format
/// --allow-onelevel`'s rules, or that starts with `-`. Checked in Rust before
/// any git process starts, so a refused `base` never reaches git, which is
/// what keeps `--output=<file>` from being an option and `main..; rm` from
/// being anything at all. Stricter than git where it is cheap to be: no
/// leading `/` and no `@`.
fn validate_base(base: &str) -> Result<()> {
    let invalid = base.is_empty()
        || base.starts_with('-')
        || base.starts_with('/')
        || base.ends_with('/')
        || base.ends_with('.')
        || base.contains("..")
        || base.contains("@{")
        || base == "@"
        || base
            .chars()
            .any(|c| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
        || base
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"));
    if invalid {
        return Err(ClientError::InvalidInput(format!(
            "`{base}` is not a branch name git would accept as `base`"
        )));
    }
    Ok(())
}

/// A root-relative path as a pathspec that matches it and nothing else: no
/// glob, no magic, whatever bytes it holds.
fn literal_pathspec(path: &[u8]) -> Vec<u8> {
    let mut pathspec = b":(top,literal)".to_vec();
    pathspec.extend_from_slice(path);
    pathspec
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

/// `git -C <root>` for a read (spec 60 §5.4). A read never takes `index.lock`
/// from an agent working in the same checkout: `--no-optional-locks` stops
/// `status` refreshing the index, and `diff.autoRefreshIndex=false` stops
/// `diff` doing the same, which it does whatever `--no-optional-locks` says.
/// No fsmonitor hook runs either; config could point it anywhere. Every git
/// process this module starts begins here.
fn read_git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "diff.autoRefreshIndex=false",
            "-C",
        ])
        .arg(root)
        .stdin(Stdio::null());
    command
}

fn git_output(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ok(read_git(root).args(args).output()?)
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
    path_policy: PathPolicy,
    caps: GitReadCaps,
}

impl GitService {
    pub fn new(
        audit_log: AuditLog,
        scanner: SecretScanner,
        path_policy: PathPolicy,
        caps: GitReadCaps,
    ) -> Self {
        Self {
            audit_log,
            scanner,
            path_policy,
            caps,
        }
    }

    pub fn status(&self, root_path: impl AsRef<Path>) -> Result<GitStatus> {
        self.status_in(root_path, None)
    }

    /// Every changed and untracked **file**, with no untracked directory
    /// collapsed, for a commit's candidate set (spec 35 §5.2).
    ///
    /// `status` is the read a model sees: it is capped and it collapses an
    /// untracked directory into one entry, which a commit cannot use because a
    /// directory is not a blob. This runs `-uall` and `-z`, so a path holding a
    /// newline or non-ASCII bytes arrives as itself rather than C-quoted, and
    /// applies the same restricted-path filter. Not capped: a commit that
    /// silently omitted a candidate would be worse than a long list, and the
    /// caller's UI is the place to bound presentation.
    pub fn changes(&self, root_path: impl AsRef<Path>) -> Result<Vec<GitFileStatus>> {
        let root = root_path.as_ref();
        let output = git_output(
            root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        let exit_code = output.status.code().unwrap_or(-1);
        if exit_code != 0 {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let files = parse_porcelain_z(&output.stdout, &|path| self.path_is_restricted(path));
        self.audit_log.record(
            "git_status_read",
            &[
                ("actor", "system".to_string()),
                ("resourcePath", root.to_string_lossy().to_string()),
                ("status", "complete".to_string()),
                ("exitCode", exit_code.to_string()),
                ("scope", "commit_candidates".to_string()),
                ("fileCount", files.len().to_string()),
            ],
        )?;
        Ok(files)
    }

    /// [`Self::status`] of a worktree's root, recording `worktree` (its
    /// display path) in the audit record; `None` is the session's own checkout.
    pub fn status_in(
        &self,
        root_path: impl AsRef<Path>,
        worktree: Option<&str>,
    ) -> Result<GitStatus> {
        let output = git_output(root_path.as_ref(), &["status", "--porcelain=v1"])?;
        let exit_code = output.status.code().unwrap_or(-1);
        let full = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let entries: Vec<&str> = full
            .lines()
            .filter(|line| !line.is_empty() && !self.porcelain_entry_is_restricted(line))
            .collect();
        let shown = self.lines_within_caps(&entries);
        let omitted = entries.len() - shown;
        // Rebuilt only when something was dropped, so an ordinary status
        // stays byte-identical to git's.
        let raw = if shown == full.lines().filter(|line| !line.is_empty()).count() {
            full.clone()
        } else {
            entries[..shown]
                .iter()
                .map(|line| format!("{line}\n"))
                .collect()
        };
        let status = GitStatus {
            clean: exit_code == 0 && entries.is_empty(),
            exit_code,
            files: if exit_code == 0 {
                parse_porcelain(&raw)
            } else {
                Vec::new()
            },
            raw,
            stderr,
            omitted,
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
                ("worktree", worktree.unwrap_or_default().to_string()),
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

    /// The working tree's (or with `staged`, the index's) diff, with
    /// restricted paths left out and the output capped (spec 60 §5.4).
    pub fn diff(&self, root_path: impl AsRef<Path>, staged: bool) -> Result<String> {
        self.diff_in(
            root_path,
            None,
            &DiffOptions {
                staged,
                ..DiffOptions::default()
            },
        )
    }

    /// [`Self::diff`] with `read_git_diff`'s options, recording `worktree`
    /// (its display path) in the audit record; `None` is the session's own
    /// checkout. Default options run exactly the git commands `diff` always
    /// ran, so its output is unchanged.
    ///
    /// Restricted paths are judged by [`PathPolicy::is_restricted`], the rule
    /// `read_file` refuses by, which a git pathspec cannot express. So the
    /// changed paths are listed first, without rename pairing so both sides of
    /// a rename are judged, and when any is restricted the content diff runs
    /// over the others as literal pathspecs. When none is, it runs over
    /// `options.paths` alone and, without `paths`, is byte-identical to `git
    /// diff`; a restricted file that starts changing between the two git calls
    /// is the accepted gap. Without the index refresh (`read_git`) the listing
    /// can also name a file whose stat changed but whose content did not; it
    /// only widens the pathspec, and the content diff prints nothing for it.
    ///
    /// `base` and `paths` are model input. Both are checked before git runs:
    /// `base` against git's ref-name rules, so it cannot become an option, and
    /// each path through [`PathPolicy`], so it cannot leave `root_path`.
    pub fn diff_in(
        &self,
        root_path: impl AsRef<Path>,
        worktree: Option<&str>,
        options: &DiffOptions,
    ) -> Result<String> {
        let root = root_path.as_ref();
        if let Some(base) = &options.base {
            validate_base(base)?;
        }
        let scope = self.diff_scope(root, &options.paths)?;
        let merge_base = match &options.base {
            Some(base) => Some(self.merge_base(root, base)?),
            None => None,
        };
        let mut args = vec!["diff"];
        if options.staged {
            args.push("--staged");
        }
        args.extend(["--no-ext-diff", "--no-textconv"]);
        let mut listing_args = args.clone();
        listing_args.extend(["--name-status", "-z", "--no-renames"]);
        if options.stat {
            args.push("--stat");
        }
        // Options end here: the merge base and pathspecs after it are never
        // read as options, whatever they contain.
        let revision: Vec<&str> = match &merge_base {
            Some(sha) => vec!["--end-of-options", sha.as_str()],
            None => Vec::new(),
        };
        let with_paths = |command: &mut Command, paths: &[Vec<u8>]| {
            if !paths.is_empty() {
                command.arg("--");
                for path in paths {
                    command.arg(OsStr::from_bytes(path));
                }
            }
        };
        let mut listing = read_git(root);
        listing.args(&listing_args).args(&revision);
        with_paths(&mut listing, &scope);
        let listing = listing.output()?;
        let output = if !listing.status.success() {
            listing
        } else {
            let changed = parse_name_status_z(&listing.stdout);
            let kept: Vec<&[u8]> = changed
                .iter()
                .copied()
                .filter(|path| !self.path_is_restricted(&String::from_utf8_lossy(path)))
                .collect();
            if kept.is_empty() && !changed.is_empty() {
                // Everything changed is restricted: an empty diff, as if
                // nothing had changed, without a second git call.
                std::process::Output {
                    stdout: Vec::new(),
                    ..listing
                }
            } else {
                let pathspecs = if kept.len() == changed.len() {
                    scope
                } else {
                    kept.into_iter().map(literal_pathspec).collect()
                };
                let mut command = read_git(root);
                command.args(&args).args(&revision);
                with_paths(&mut command, &pathspecs);
                command.output()?
            }
        };
        let exit_code = output.status.code().unwrap_or(-1);
        self.audit_log.record(
            "git_diff_read",
            &[
                ("actor", "system".to_string()),
                ("resourcePath", root.to_string_lossy().to_string()),
                (
                    "status",
                    if exit_code == 0 { "complete" } else { "failed" }.to_string(),
                ),
                ("exitCode", exit_code.to_string()),
                ("staged", options.staged.to_string()),
                ("worktree", worktree.unwrap_or_default().to_string()),
            ],
        )?;
        if exit_code != 0 {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let raw_diff = String::from_utf8_lossy(&output.stdout);
        // Redact before cutting: a cut through a secret could leave a prefix
        // the scanner no longer recognises.
        let redacted = self.scanner.redact(raw_diff.as_ref()).text;
        Ok(self.cap_diff(redacted))
    }

    /// `paths` as root-relative literal pathspecs, each resolved through
    /// [`PathPolicy`] so `..`, an absolute path or a symlink cannot reach
    /// outside `root`. A path need not exist: a deleted file is still in the
    /// diff.
    fn diff_scope(&self, root: &Path, paths: &[String]) -> Result<Vec<Vec<u8>>> {
        paths
            .iter()
            .map(|path| {
                let resolved = self
                    .path_policy
                    .resolve_for_write(root, path)
                    .map_err(|_| {
                        ClientError::AccessDenied(format!(
                            "Diff path `{path}` resolves outside the checkout being read"
                        ))
                    })?;
                Ok(match resolved.relative_path.as_str() {
                    "" | "." => b":(top)".to_vec(),
                    relative => literal_pathspec(relative.as_bytes()),
                })
            })
            .collect()
    }

    /// `merge-base(base, HEAD)` as a full SHA, so the diff compares against
    /// the point the branch left `base` rather than `base`'s current tip.
    fn merge_base(&self, root: &Path, base: &str) -> Result<String> {
        let output = git_output(root, &["merge-base", "--end-of-options", base, "HEAD"])?;
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || sha.is_empty() {
            return Err(ClientError::Git(format!(
                "No merge base between `{base}` and HEAD: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(sha)
    }

    /// How many of `lines` fit under both caps, counting each with its newline.
    fn lines_within_caps(&self, lines: &[&str]) -> usize {
        let max_bytes = usize::try_from(self.caps.max_bytes).unwrap_or(usize::MAX);
        let mut bytes = 0usize;
        lines
            .iter()
            .take(self.caps.max_lines)
            .take_while(|line| {
                bytes = bytes.saturating_add(line.len() + 1);
                bytes <= max_bytes
            })
            .count()
    }

    /// `diff` cut to the caps, ending in a line that says what was cut: the
    /// lines shown of the total, and the files whose header was not reached
    /// (spec 47 §5.5's shape).
    fn cap_diff(&self, diff: String) -> String {
        let lines: Vec<&str> = diff.lines().collect();
        let shown = self.lines_within_caps(&lines);
        if shown == lines.len() {
            return diff;
        }
        let cut_by = if shown == self.caps.max_lines {
            "lines"
        } else {
            "bytes"
        };
        let files = |lines: &[&str]| {
            lines
                .iter()
                .filter(|line| line.starts_with("diff --git ") || line.starts_with("diff --cc "))
                .count()
        };
        let total_files = files(&lines);
        let shown_files = files(&lines[..shown]);
        let mut capped: String = lines[..shown]
            .iter()
            .map(|line| format!("{line}\n"))
            .collect();
        capped.push_str(&format!(
            "[Diff truncated by {cut_by}: {shown} of {} lines shown; {} of {total_files} files not shown.]\n",
            lines.len(),
            total_files - shown_files,
        ));
        capped
    }

    /// Whether `relative_path`, or any directory above it, is restricted. A
    /// path inside a restricted directory is one `list_directory` never
    /// reaches, so a git read does not show it either. A trailing `/` is how
    /// porcelain names an untracked directory.
    fn path_is_restricted(&self, relative_path: &str) -> bool {
        let (path, is_directory) = match relative_path.strip_suffix('/') {
            Some(directory) => (directory, true),
            None => (relative_path, false),
        };
        self.path_policy.is_restricted(path, is_directory)
            || path
                .match_indices('/')
                .any(|(at, _)| self.path_policy.is_restricted(&path[..at], true))
    }

    /// Whether a `--porcelain=v1` line names a restricted path on either side
    /// of a rename. A path containing ` -> ` is ambiguous in this format, so
    /// the whole field is judged as well as each side.
    fn porcelain_entry_is_restricted(&self, line: &str) -> bool {
        let field = line.get(3..).unwrap_or_default();
        std::iter::once(field)
            .chain(field.split(" -> "))
            .any(|path| self.path_is_restricted(&unquote_porcelain_path(path)))
    }

    /// Every worktree git has registered for the repository at `root_path`,
    /// main worktree first, as `git worktree list --porcelain -z` lists them.
    ///
    /// Not audited: this answers which roots a read may reach, and the read
    /// that follows records itself (spec 60 §5.5). No optional lock is taken
    /// and no fsmonitor hook runs, because the worktrees listed may be in use
    /// by another agent.
    pub fn worktrees(
        &self,
        root_path: impl AsRef<Path>,
    ) -> Result<Vec<crate::worktree_access::WorktreeEntry>> {
        let output = git_output(
            root_path.as_ref(),
            &["worktree", "list", "--porcelain", "-z"],
        )?;
        if !output.status.success() {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(crate::worktree_access::parse_worktree_list(&output.stdout))
    }

    /// The branch checked out in the main worktree, which `base` defaults to
    /// (spec 60 §5.2). Git lists the main worktree first, and `git worktree
    /// list` resolves it from any worktree of the repository. `None` when it
    /// is detached or bare, so callers omit `ahead`/`behind` rather than guess.
    fn main_worktree_branch(&self, repository_root: &Path) -> Result<Option<String>> {
        Ok(self
            .worktrees(repository_root)?
            .into_iter()
            .next()
            .and_then(|entry| entry.branch))
    }

    /// `(ahead, behind)` of `root`'s HEAD against `base`, the two counts of
    /// `rev-list --left-right --count base...HEAD`: the left count is the
    /// commits `base` has that HEAD does not (behind), the right the reverse
    /// (ahead).
    fn ahead_behind(&self, root: &Path, base: &str) -> Result<(usize, usize)> {
        let output = git_output(
            root,
            &[
                "rev-list",
                "--left-right",
                "--count",
                "--end-of-options",
                &format!("{base}...HEAD"),
            ],
        )?;
        if !output.status.success() {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut counts = text
            .split_whitespace()
            .map(|n| n.parse::<usize>().unwrap_or(0));
        // `--left-right --count` prints the left count (behind) then the right
        // count (ahead); the return is `(ahead, behind)`.
        let behind = counts.next().unwrap_or(0);
        let ahead = counts.next().unwrap_or(0);
        Ok((ahead, behind))
    }

    /// The number of changed files `status --porcelain=v1` lists, restricted
    /// paths not counted (spec 60 §5.2). A count, not paths, so the read caps
    /// do not apply and nothing of a restricted file's name leaves here.
    fn changed_file_count(&self, root: &Path) -> Result<usize> {
        let output = git_output(root, &["status", "--porcelain=v1"])?;
        if !output.status.success() {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        let full = String::from_utf8_lossy(&output.stdout);
        Ok(full
            .lines()
            .filter(|line| !line.is_empty() && !self.porcelain_entry_is_restricted(line))
            .count())
    }

    /// Every reachable worktree, as `list_worktrees` reports them (spec 60
    /// §5.2). `base` is the model's override, validated before any git process
    /// runs; the default is the main worktree's branch, and when there is no
    /// base the ahead/behind fields are `None`. One git process for the list,
    /// then `status`, `rev-parse` and `rev-list` per listed worktree, capped at
    /// 20 with the rest counted. Not audited per worktree: the tool records one
    /// `git_worktrees_listed`.
    pub fn list_worktrees(
        &self,
        repository_root: &Path,
        base: Option<&str>,
    ) -> Result<WorktreeListing> {
        if let Some(base) = base {
            validate_base(base)?;
        }
        let entries = self.worktrees(repository_root)?;
        let default_base = entries.first().and_then(|entry| entry.branch.clone());
        let base = base.map(str::to_string).or(default_base);
        let classified = crate::worktree_access::classify_worktrees(
            &self.path_policy,
            repository_root,
            entries,
        )?;
        let reachable: Vec<_> = classified
            .iter()
            .filter(|worktree| worktree.refusal.is_none())
            .collect();
        let omitted = reachable.len().saturating_sub(WORKTREES_LISTED_CAP);
        let mut listed = Vec::new();
        for worktree in reachable.into_iter().take(WORKTREES_LISTED_CAP) {
            let root = worktree
                .canonical
                .as_deref()
                .expect("a reachable worktree has a canonical path");
            let head = git_line(root, &["rev-parse", "--short", "--verify", "-q", "HEAD"])?;
            let changed_files = self.changed_file_count(root)?;
            let (ahead, behind) = match &base {
                Some(base) => {
                    let (ahead, behind) = self.ahead_behind(root, base)?;
                    (Some(ahead), Some(behind))
                }
                None => (None, None),
            };
            listed.push(WorktreeListingEntry {
                path: worktree.display.clone(),
                branch: worktree.entry.branch.clone(),
                head,
                changed_files,
                ahead,
                behind,
            });
        }
        self.audit_log.record(
            "git_worktrees_listed",
            &[
                ("actor", "system".to_string()),
                (
                    "resourcePath",
                    repository_root.to_string_lossy().to_string(),
                ),
                ("count", listed.len().to_string()),
                ("omitted", omitted.to_string()),
            ],
        )?;
        Ok(WorktreeListing {
            base,
            entries: listed,
            omitted,
        })
    }

    /// The commits `base..HEAD` (or `HEAD`'s history without a base) as
    /// `short-sha subject` lines, newest first, capped (spec 60 §5.2). `base`
    /// defaults to the main worktree's branch; when there is none, the whole
    /// history is read. `max_count` narrows the configured line cap, never
    /// raises it. A `--oneline` log names no paths, so nothing restricted can
    /// leak; drivers and fsmonitor stay off (spec 60 §5.4).
    pub fn log_in(
        &self,
        root_path: impl AsRef<Path>,
        worktree: Option<&str>,
        base: Option<&str>,
        max_count: Option<usize>,
    ) -> Result<String> {
        let root = root_path.as_ref();
        if let Some(base) = base {
            validate_base(base)?;
        }
        let base = match base {
            Some(base) => Some(base.to_string()),
            None => self.main_worktree_branch(root)?,
        };
        let revision = match &base {
            Some(base) => format!("{base}..HEAD"),
            None => "HEAD".to_string(),
        };
        let max_commits = max_count
            .unwrap_or(self.caps.max_lines)
            .min(self.caps.max_lines);
        let max_commits = max_commits.to_string();
        let output = git_output(
            root,
            &[
                "log",
                "--oneline",
                "--no-ext-diff",
                "--no-textconv",
                "-n",
                &max_commits,
                "--end-of-options",
                &revision,
            ],
        )?;
        let exit_code = output.status.code().unwrap_or(-1);
        self.audit_log.record(
            "git_log_read",
            &[
                ("actor", "system".to_string()),
                ("resourcePath", root.to_string_lossy().to_string()),
                (
                    "status",
                    if exit_code == 0 { "complete" } else { "failed" }.to_string(),
                ),
                ("exitCode", exit_code.to_string()),
                ("worktree", worktree.unwrap_or_default().to_string()),
            ],
        )?;
        if exit_code != 0 {
            return Err(ClientError::Git(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let raw = String::from_utf8_lossy(&output.stdout);
        // Redact before cutting, so a cut cannot leave a secret's prefix.
        let redacted = self.scanner.redact(raw.as_ref()).text;
        Ok(self.cap_log(redacted))
    }

    /// `log` cut to the read caps when they bind, ending in a line that says
    /// what was cut. The commit count is already bound by `-n`, so in practice
    /// only the byte cap cuts here (spec 60 §5.4).
    fn cap_log(&self, log: String) -> String {
        let lines: Vec<&str> = log.lines().collect();
        let shown = self.lines_within_caps(&lines);
        if shown == lines.len() {
            return log;
        }
        let cut_by = if shown == self.caps.max_lines {
            "lines"
        } else {
            "bytes"
        };
        let mut capped: String = lines[..shown]
            .iter()
            .map(|line| format!("{line}\n"))
            .collect();
        capped.push_str(&format!(
            "[Log truncated by {cut_by}: {shown} of {} commits shown.]\n",
            lines.len()
        ));
        capped
    }

    /// The full `HEAD` SHA, or `None` on an unborn branch. Used by commit
    /// recovery to answer whether an interrupted commit landed (spec 35 §5.7).
    /// Not audited: the sweep that reads it records its own findings.
    pub fn head_commit(&self, root_path: impl AsRef<Path>) -> Result<Option<String>> {
        git_line(root_path.as_ref(), &["rev-parse", "--verify", "-q", "HEAD"])
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

/// The paths `diff --name-status -z --no-renames` printed: a status field and
/// one path per entry, each NUL-terminated.
fn parse_name_status_z(raw: &[u8]) -> Vec<&[u8]> {
    raw.split(|&byte| byte == 0)
        .skip(1)
        .step_by(2)
        .filter(|path| !path.is_empty())
        .collect()
}

/// A porcelain path as git wrote it, with C-style quoting undone. Git quotes
/// a path holding a control character, `"`, `\\` or (by default) any non-ASCII
/// byte, so `".env\303\251"` must be judged as the name it stands for.
fn unquote_porcelain_path(path: &str) -> String {
    let Some(inner) = path
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return path.to_string();
    };
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' || index + 1 == bytes.len() {
            out.push(bytes[index]);
            index += 1;
            continue;
        }
        let escaped = bytes[index + 1];
        let octal = bytes
            .get(index + 1..index + 4)
            .filter(|digits| digits.iter().all(|digit| (b'0'..=b'7').contains(digit)));
        if let Some(digits) = octal {
            out.push(digits.iter().fold(0u8, |value, digit| {
                value.wrapping_mul(8).wrapping_add(digit - b'0')
            }));
            index += 4;
            continue;
        }
        out.push(match escaped {
            b'a' => 0x07,
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 0x0b,
            other => other,
        });
        index += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `-z` porcelain `changes` reads: one record per changed file, with a
/// rename or copy followed by its source path. A rename keeps its source so
/// the commit can carry the deletion; a copy does not, because the source
/// stays in place. Both sides of a pair are judged against the restricted
/// matcher, and a pair is dropped whole when either is restricted.
fn parse_porcelain_z(raw: &[u8], is_restricted: &dyn Fn(&str) -> bool) -> Vec<GitFileStatus> {
    let mut records = raw
        .split(|&byte| byte == 0)
        .filter(|record| !record.is_empty());
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let index_status = record[0];
        let worktree_status = record[1];
        let renamed = index_status == b'R' || worktree_status == b'R';
        let copied = index_status == b'C' || worktree_status == b'C';
        let original = if renamed || copied {
            records
                .next()
                .map(|path| String::from_utf8_lossy(path).into_owned())
                .filter(|path| !path.is_empty())
        } else {
            None
        };
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        if is_restricted(&path) || original.as_deref().is_some_and(is_restricted) {
            continue;
        }
        let pair = format!("{}{}", index_status as char, worktree_status as char);
        files.push(GitFileStatus {
            path,
            raw: String::from_utf8_lossy(record).into_owned(),
            staged: index_status != b' ' && index_status != b'?',
            worktree: worktree_status != b' ',
            untracked: index_status == b'?' && worktree_status == b'?',
            conflicted: ["AA", "DD", "AU", "UD", "UA", "DU", "UU"].contains(&pair.as_str()),
            renamed_from: if renamed { original.clone() } else { None },
            copied_from: if copied { original } else { None },
        });
    }
    files
}

fn parse_porcelain(raw: &str) -> Vec<GitFileStatus> {
    raw.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let index_status = line.as_bytes().first().copied().unwrap_or(b' ');
            let worktree_status = line.as_bytes().get(1).copied().unwrap_or(b' ');
            let path = line.get(3..).unwrap_or_default();
            let (path, original) = match path.split_once(" -> ") {
                Some((old, new)) => (new.to_string(), Some(old.to_string())),
                None => (path.to_string(), None),
            };
            let pair = format!("{}{}", index_status as char, worktree_status as char);
            let renamed = index_status == b'R' || worktree_status == b'R';
            let copied = index_status == b'C' || worktree_status == b'C';
            GitFileStatus {
                path,
                raw: line.to_string(),
                staged: index_status != b' ' && index_status != b'?',
                worktree: worktree_status != b' ',
                untracked: index_status == b'?' && worktree_status == b'?',
                conflicted: ["AA", "DD", "AU", "UD", "UA", "DU", "UU"].contains(&pair.as_str()),
                renamed_from: if renamed { original.clone() } else { None },
                copied_from: if copied { original } else { None },
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

    /// A rename keeps its source so the commit can carry the deletion; a copy
    /// does not, because its source stays in place (spec 35 §5.3).
    #[test]
    fn a_rename_record_keeps_its_source_and_a_copy_record_does_not() {
        let rename = parse_porcelain_z(b"R  b.txt\0a.txt\0", &|_| false);
        assert_eq!(rename.len(), 1);
        assert_eq!(rename[0].path, "b.txt");
        assert_eq!(rename[0].renamed_from.as_deref(), Some("a.txt"));
        assert_eq!(rename[0].copied_from, None);

        let copy = parse_porcelain_z(b"C  c.txt\0a.txt\0", &|_| false);
        assert_eq!(copy.len(), 1);
        assert_eq!(copy[0].path, "c.txt");
        assert_eq!(copy[0].renamed_from, None);
        assert_eq!(copy[0].copied_from.as_deref(), Some("a.txt"));

        // The worktree column is a rename too: ` R`, `RM` and `RD` all carry
        // the source record and must keep it.
        for record in [
            b" R b.txt\0a.txt\0".as_slice(),
            b"RM b.txt\0a.txt\0".as_slice(),
            b"RD b.txt\0a.txt\0".as_slice(),
        ] {
            let parsed = parse_porcelain_z(record, &|_| false);
            assert_eq!(parsed.len(), 1, "{record:?}");
            assert_eq!(parsed[0].renamed_from.as_deref(), Some("a.txt"));
        }

        // A restricted source drops the whole pair: committing one half of a
        // rename would be worse than not offering it.
        let restricted = parse_porcelain_z(b"R  b.txt\0.env\0", &|path| path == ".env");
        assert!(restricted.is_empty());
    }

    #[test]
    fn porcelain_paths_are_unquoted_before_they_are_judged() {
        assert_eq!(unquote_porcelain_path("plain.txt"), "plain.txt");
        assert_eq!(unquote_porcelain_path("\".env.\\303\\251\""), ".env.é");
        assert_eq!(unquote_porcelain_path("\"a\\tb\\\"c\\\\d\""), "a\tb\"c\\d");
        // Out-of-range octal wraps rather than panicking.
        assert_eq!(unquote_porcelain_path("\"\\777\""), "\u{FFFD}");
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
            let service = GitService::new(
                AuditLog::new(&data_dir, true, scanner.clone()),
                scanner,
                PathPolicy::unrestricted(),
                GitReadCaps::from_config(&Config::default()),
            );
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
