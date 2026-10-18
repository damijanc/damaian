use crate::config::{CommandAccess, Config};
use crate::error::Result;
use crate::path_policy::PathPolicy;
use crate::repository_map::RootCommand;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandRisk {
    Low,
    Medium,
    High,
    Blocked,
}

impl CommandRisk {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandClassification {
    pub command: String,
    pub risk: CommandRisk,
    pub blocked: bool,
    pub requires_approval: bool,
    pub reasons: Vec<String>,
    pub expected_effects: String,
    pub may_use_network: bool,
}

impl CommandClassification {
    /// What Plan and Review mode allow, and what `command_access=read_only`
    /// allows: low risk, no approval, *and* read-only by its text. The third
    /// check stops the allowlist, which also yields low risk with no approval,
    /// from widening either one (spec 20 `context.md` §5). One function, so the
    /// mode and the profile cannot drift apart.
    pub(crate) fn is_read_only_without_approval(&self) -> bool {
        self.risk == CommandRisk::Low
            && !self.requires_approval
            && is_low_risk_read_only(&self.command)
    }
}

#[derive(Debug, Clone)]
pub struct ProjectCommand {
    pub name: String,
    pub command: String,
    pub risk: CommandRisk,
}

/// Every manifest that gives a directory project commands, in the order
/// `detect_project_commands` checks them. Root detection (spec 24) reads this
/// list, so a directory is a root exactly when it has something to run. A
/// test ties the two together (`every_root_manifest_gives_its_directory_a_command`).
pub const PROJECT_MANIFESTS: [&str; 7] = [
    "package.json",
    "pyproject.toml",
    "pytest.ini",
    "pom.xml",
    "build.gradle",
    "go.mod",
    "Cargo.toml",
];

const READ_ONLY_REASON: &str = "Read-only command";

/// The prefix of the reason an admitted read-only command needs approval
/// because it names a restricted path. Spec 62's `ApprovalCause` will carry
/// this as `RestrictedPath`; until it exists, the reason string does.
pub const RESTRICTED_PATH_REASON: &str = "Command reads a restricted path";

#[derive(Debug, Clone)]
pub struct CommandPolicy {
    config: Config,
}

impl CommandPolicy {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// The policy configuration in effect, so callers that need to reason
    /// about approval settings can do so without reloading it from disk and
    /// risking a different answer than the one this policy is enforcing.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Classifies a command run at the repository root. Every caller that
    /// predates spec 24 passes the root, so this is [`Self::classify_at`]
    /// with the two halves of the location equal.
    pub fn classify(&self, command: &str, working_directory: &Path) -> CommandClassification {
        self.classify_at(command, working_directory, working_directory)
    }

    /// Classifies a command run in `working_directory`, inside the repository
    /// at `repository_root` (spec 24 `context.md` §1). Relative paths in the
    /// command resolve against the directory, but containment is checked
    /// against the repository root, and a directory other than the root only
    /// matches allowlist entries qualified with it (`context.md` §2). So a
    /// root changes where a command runs, never what it may reach or whether
    /// it needs approval.
    pub fn classify_at(
        &self,
        command: &str,
        repository_root: &Path,
        working_directory: &Path,
    ) -> CommandClassification {
        let location = relative_location(repository_root, working_directory);
        let (mut classification, admission) = self.classify_pattern(command, location.as_deref());
        if !classification.blocked && location.is_none() {
            escalate(
                &mut classification,
                "Working directory is outside the selected repository",
            );
        }
        if let Some(admission) = &admission {
            // Spec 64 requirements 3–4: an admitted read-only command's
            // paths, and the directory it runs in, go through the same
            // restricted patterns `read_file` honours. This replaces the
            // outside-root heuristic below for these commands.
            let path_policy = PathPolicy::new(&self.config);
            if let Some(relative) = location.as_deref()
                && let Some(restricted) = restricted_reach(&path_policy, relative, true)
            {
                escalate(
                    &mut classification,
                    &format!("{RESTRICTED_PATH_REASON}: {restricted}"),
                );
            }
            // Requirement 6: a leading `cd <dir>` is held to the same rules
            // as the working directory, and everything after it runs, and
            // so resolves, there. It must name a directory that exists:
            // otherwise `cdable_vars` or `CDPATH` may find one elsewhere.
            let cd_target = admission
                .directory
                .as_deref()
                .map(PathArgument::relative_to_working_directory);
            let directory = match &cd_target {
                Some(target) => normalize_lexically(&working_directory.join(&target.text)),
                None => working_directory.to_path_buf(),
            };
            if let Some(target) = &cd_target
                && !directory.is_dir()
            {
                escalate(
                    &mut classification,
                    &format!(
                        "`cd {}` does not name a directory in the repository",
                        target.text
                    ),
                );
            }
            let checks = cd_target
                .iter()
                .map(|target| (working_directory, target))
                .chain(
                    admission
                        .paths
                        .iter()
                        .map(|path| (directory.as_path(), path)),
                );
            let mut outside_reported = false;
            for (base, path) in checks {
                match check_path_argument(&path_policy, repository_root, base, path) {
                    None => {}
                    Some(PathFinding::OutsideRepository) if !outside_reported => {
                        outside_reported = true;
                        escalate(
                            &mut classification,
                            "Command references a path outside the selected repository",
                        );
                    }
                    Some(PathFinding::OutsideRepository) => {}
                    Some(PathFinding::Restricted(restricted)) => escalate(
                        &mut classification,
                        &format!("{RESTRICTED_PATH_REASON}: {restricted}"),
                    ),
                    Some(PathFinding::RecursiveRoot) => escalate(
                        &mut classification,
                        "`ls -R` of the repository root would list restricted directories too",
                    ),
                }
            }
        } else if !classification.blocked
            && references_path_outside_root(
                &classification.command,
                repository_root,
                working_directory,
            )
        {
            escalate(
                &mut classification,
                "Command references a path outside the selected repository",
            );
        }
        // After the whole classification, so the block never has to invent a
        // risk (spec 31 proposal §4) and sees the outside-root approval, the way
        // Plan mode does. After the allowlist too: Allow Always cannot outrank
        // a profile.
        if !classification.blocked
            && !command_access_permits(self.config.command_access, &classification)
        {
            classification.blocked = true;
            classification.reasons.push(format!(
                "Blocked by permission profile: command_access={}",
                self.config.command_access.as_str()
            ));
        }
        classification
    }

    /// `location` is the working directory relative to the repository root:
    /// `Some("")` at the root, `None` outside the repository. The second
    /// half is the `cd` directory and path arguments of a command admitted
    /// as read-only, which `classify_at` still has to check (spec 64
    /// requirements 3, 4 and 6); `None` for every other command.
    fn classify_pattern(
        &self,
        command: &str,
        location: Option<&str>,
    ) -> (CommandClassification, Option<Admission>) {
        let normalized = command.trim().to_string();
        let verdict = read_only_verdict(&normalized);
        let classification = self.classify_text(normalized, command, location, &verdict);
        let admitted = match verdict {
            ReadOnlyVerdict::Admitted(admission)
                if classification.reasons.first().map(String::as_str) == Some(READ_ONLY_REASON) =>
            {
                Some(admission)
            }
            _ => None,
        };
        (classification, admitted)
    }

    fn classify_text(
        &self,
        normalized: String,
        command: &str,
        location: Option<&str>,
        verdict: &ReadOnlyVerdict,
    ) -> CommandClassification {
        if configured_prefix_matches(&self.config.command_blocklist, &normalized)
            || is_blocked_command(&normalized)
        {
            return CommandClassification {
                command: normalized,
                risk: CommandRisk::Blocked,
                blocked: true,
                requires_approval: true,
                reasons: vec!["Command matches a blocked destructive pattern".to_string()],
                expected_effects: "Blocked by local policy".to_string(),
                may_use_network: may_use_network(command),
            };
        }

        if contains_shell_control(&normalized) {
            // Spec 64 requirement 6: the tokenizer, not this character scan,
            // decides. A read-only pipeline, or an admitted command with `|`
            // inside quotes, passes; every other operator, redirect and
            // substitution is still caught here.
            if matches!(verdict, ReadOnlyVerdict::Admitted(_)) {
                return self.read_only_classification(normalized);
            }
            let mut reasons =
                vec!["Command contains shell control syntax and needs explicit review".to_string()];
            if let ReadOnlyVerdict::Refused(reason) = verdict {
                reasons.push(reason.clone());
            }
            return CommandClassification {
                command: normalized,
                risk: CommandRisk::High,
                blocked: false,
                requires_approval: true,
                reasons,
                expected_effects: "Potential chained or redirected command effects".to_string(),
                may_use_network: may_use_network(command),
            };
        }

        let allowlisted = match location {
            Some("") => configured_exact_matches(&self.config.command_allowlist, &normalized),
            Some(relative) => configured_exact_matches(
                &self.config.command_allowlist,
                &root_qualified_allowlist_entry(relative, &normalized),
            ),
            None => false,
        };
        if allowlisted {
            return CommandClassification {
                command: normalized,
                risk: CommandRisk::Low,
                blocked: false,
                requires_approval: self.config.require_approval_for_all_commands,
                reasons: vec!["Command matches configured allowlist".to_string()],
                expected_effects: "Configured safe command".to_string(),
                may_use_network: false,
            };
        }

        match verdict {
            ReadOnlyVerdict::Admitted(_) => return self.read_only_classification(normalized),
            // A read-only program outside its admitted form: an option that
            // writes or redirects (`--output`, `-C`), or text the shell
            // would rewrite. Its effects are unknown until reviewed.
            ReadOnlyVerdict::Refused(reason) => {
                return CommandClassification {
                    command: normalized,
                    risk: CommandRisk::High,
                    blocked: false,
                    requires_approval: true,
                    reasons: vec![reason.clone()],
                    expected_effects: "Unknown effects until reviewed".to_string(),
                    may_use_network: may_use_network(command),
                };
            }
            ReadOnlyVerdict::NotReadOnly => {}
        }

        if is_validation_command(&normalized) {
            return CommandClassification {
                command: normalized,
                risk: CommandRisk::Medium,
                blocked: false,
                requires_approval: self.config.require_approval_for_all_commands
                    || self.config.require_approval_for_risky_commands,
                reasons: vec![
                    "Validation command may write build, cache, or coverage artifacts".to_string(),
                ],
                expected_effects: "Runs project validation and may create local artifacts"
                    .to_string(),
                may_use_network: false,
            };
        }

        if is_docker_command(&normalized) {
            return CommandClassification {
                command: normalized,
                risk: CommandRisk::High,
                blocked: false,
                requires_approval: true,
                reasons: vec![
                    "Docker command may start containers, mount host paths, mutate images or volumes, expose ports, or use the network".to_string(),
                ],
                expected_effects:
                    "Potential Docker daemon, workspace, network, or background-service effects"
                        .to_string(),
                may_use_network: true,
            };
        }

        if is_high_risk_command(&normalized) {
            return CommandClassification {
                command: normalized,
                risk: CommandRisk::High,
                blocked: false,
                requires_approval: true,
                reasons: vec![
                    "Command may modify dependencies, Git state, permissions, network, or shell state"
                        .to_string(),
                ],
                expected_effects: "Potential workspace or external side effects".to_string(),
                may_use_network: may_use_network(command),
            };
        }

        CommandClassification {
            command: normalized,
            risk: CommandRisk::High,
            blocked: false,
            requires_approval: true,
            reasons: vec!["Unknown command effects".to_string()],
            expected_effects: "Unknown effects until reviewed".to_string(),
            may_use_network: may_use_network(command),
        }
    }

    /// An admitted read-only command or pipeline (spec 64 §5.5): Low, and
    /// needing approval only when every command does.
    fn read_only_classification(&self, normalized: String) -> CommandClassification {
        CommandClassification {
            command: normalized,
            risk: CommandRisk::Low,
            blocked: false,
            requires_approval: self.config.require_approval_for_all_commands,
            reasons: vec![READ_ONLY_REASON.to_string()],
            expected_effects: "Reads workspace or Git metadata".to_string(),
            may_use_network: false,
        }
    }

    pub fn detect_project_commands(
        &self,
        root_path: impl AsRef<Path>,
    ) -> Result<Vec<ProjectCommand>> {
        let root = root_path.as_ref();
        self.detect_in(root, root)
    }

    /// The commands of the project root `root` (repository-relative, `""` for
    /// the repository root), each carrying the directory it runs in. Sorted
    /// by name (spec 24 §5.3).
    pub fn detect_root_commands(
        &self,
        repository_root: &Path,
        root: &str,
    ) -> Result<Vec<RootCommand>> {
        let directory = if root.is_empty() {
            repository_root.to_path_buf()
        } else {
            repository_root.join(root)
        };
        let mut commands: Vec<RootCommand> = self
            .detect_in(repository_root, &directory)?
            .into_iter()
            .map(|command| RootCommand {
                name: command.name,
                command: command.command,
                risk: command.risk,
                working_directory: root.to_string(),
            })
            .collect();
        commands.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.command.cmp(&b.command)));
        Ok(commands)
    }

    /// Detects the commands `directory`'s manifests give it, classified where
    /// they will run: inside `repository_root`, at `directory`. So a grant
    /// qualified with a sub-root counts for that root's commands (spec 24
    /// `context.md` §2).
    fn detect_in(&self, repository_root: &Path, directory: &Path) -> Result<Vec<ProjectCommand>> {
        let mut commands = Vec::new();
        let package_path = directory.join("package.json");
        if let Ok(package_json) = fs::read_to_string(package_path) {
            for name in ["test", "lint", "typecheck", "build", "format"] {
                if package_json.contains(&format!("\"{name}\"")) {
                    let command = format!("npm run {name}");
                    commands.push(ProjectCommand {
                        name: name.to_string(),
                        risk: self.classify_at(&command, repository_root, directory).risk,
                        command,
                    });
                }
            }
            if package_json.contains("\"test\"") {
                commands.push(ProjectCommand {
                    name: "test-shortcut".to_string(),
                    command: "npm test".to_string(),
                    risk: self
                        .classify_at("npm test", repository_root, directory)
                        .risk,
                });
            }
        }

        for (file_name, command) in [
            ("pyproject.toml", "pytest"),
            ("pytest.ini", "pytest"),
            ("pom.xml", "mvn test"),
            ("build.gradle", "gradle test"),
            ("go.mod", "go test ./..."),
            ("Cargo.toml", "cargo test"),
        ] {
            if directory.join(file_name).exists() {
                commands.push(ProjectCommand {
                    name: file_name.to_string(),
                    command: command.to_string(),
                    risk: self.classify_at(command, repository_root, directory).risk,
                });
            }
        }

        Ok(commands)
    }
}

/// Whether the user may permanently allowlist `command` straight from an
/// approval prompt.
///
/// Three exclusions, each for a different reason:
/// - Blocked commands are a policy decision, not an approval decision. There
///   is no approval that makes `rm -rf /` run.
/// - Shell-control commands would silently do nothing: `classify_pattern`
///   consults the allowlist *after* the shell-control gate, so the entry
///   could never match. `command_allowlist` is also pipe-separated on disk
///   ([`crate::config::ConfigOverlay`]), so a piped command cannot even
///   round-trip through the file.
/// - `require_approval_for_all_commands` means "prompt me for everything",
///   which an allowlist entry cannot satisfy — offering the option there
///   would promise something the policy then refuses to honor.
pub fn allow_always_eligible(config: &Config, command: &str, blocked: bool) -> bool {
    !blocked && !config.require_approval_for_all_commands && !contains_shell_control(command.trim())
}

/// Whether `command_access` lets this command run. `Local` judges the text,
/// not `classification.may_use_network`, which the allowlist branch sets to
/// `false`. It is a name heuristic, not a sandbox (spec 31 `context.md` §7).
pub(crate) fn command_access_permits(
    access: CommandAccess,
    classification: &CommandClassification,
) -> bool {
    match access {
        CommandAccess::None => false,
        CommandAccess::ReadOnly => classification.is_read_only_without_approval(),
        CommandAccess::Local => !may_use_network(&classification.command),
        CommandAccess::All => true,
    }
}

fn escalate(classification: &mut CommandClassification, reason: &str) {
    classification.requires_approval = true;
    if classification.risk == CommandRisk::Low {
        classification.risk = CommandRisk::Medium;
    }
    classification.reasons.push(reason.to_string());
}

/// The allowlist entry that authorises `command` in the directory `relative`
/// to the repository root, and nowhere else (spec 24 `context.md` §2).
///
/// The form cannot match a command anyone types: it contains `&&`, and
/// `classify_pattern` returns at the shell-control gate before it consults
/// the allowlist. So it only ever matches through this function, and it
/// still reads in the Settings policy view as the command it authorises.
pub(crate) fn root_qualified_allowlist_entry(relative: &str, command: &str) -> String {
    format!("cd {relative} && {command}")
}

/// `working_directory` relative to `repository_root`, with `/` separators,
/// computed lexically like the outside-root check. `Some("")` is the root
/// itself, and `None` means the directory is outside the repository.
pub(crate) fn relative_location(
    repository_root: &Path,
    working_directory: &Path,
) -> Option<String> {
    let root = normalize_lexically(repository_root);
    let directory = normalize_lexically(working_directory);
    let relative = directory.strip_prefix(&root).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

fn configured_prefix_matches(patterns: &[String], command: &str) -> bool {
    patterns.iter().any(|pattern| command.starts_with(pattern))
}

fn configured_exact_matches(patterns: &[String], command: &str) -> bool {
    patterns.iter().any(|pattern| pattern.trim() == command)
}

fn contains_shell_control(command: &str) -> bool {
    command.contains(';')
        || command.contains('&')
        || command.contains('|')
        || command.contains('`')
        || command.contains('<')
        || command.contains('>')
        || command.contains('\n')
        || command.contains('\r')
        || command.contains("$(")
}

fn is_blocked_command(command: &str) -> bool {
    let trimmed = command.trim();
    let delete_root = trimmed.starts_with("rm -rf /")
        || trimmed == "rm -rf ."
        || trimmed == "rm -rf ./"
        || trimmed == "rm -rf *"
        || trimmed == "rm -rf ~"
        || trimmed == "rm -rf \".\""
        || trimmed == "rm -rf '.'";
    delete_root
        || trimmed.contains("git reset --hard")
        || trimmed.contains("git clean -fd")
        || trimmed.starts_with("dd if=")
        || trimmed.contains(" mkfs")
        || trimmed == "shutdown"
        || trimmed == "reboot"
}

/// Whether `command` is in the admitted read-only form (spec 64 §5.1–§5.2):
/// split into words with no shell expansion, an admitted program, and only
/// that program's admitted options. Says nothing about where its paths
/// lead; `classify_at` checks those against the repository.
pub(crate) fn is_low_risk_read_only(command: &str) -> bool {
    matches!(read_only_verdict(command), ReadOnlyVerdict::Admitted(_))
}

/// What the read-only boundary decides about a command's text (spec 64).
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReadOnlyVerdict {
    /// Not one of the read-only programs; the later branches classify it.
    NotReadOnly,
    /// A read-only program in a form the boundary does not admit, so it
    /// needs approval. The reason is what the user is shown.
    Refused(String),
    /// Admitted; the `cd` directory and each path argument still have to
    /// pass requirements 3 and 4.
    Admitted(Admission),
}

/// What `classify_at` still has to check about an admitted command or
/// pipeline: where it runs and which paths it names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Admission {
    /// The `<dir>` of a leading `cd <dir> &&`, relative to the working
    /// directory. Every path argument resolves against it.
    directory: Option<String>,
    paths: Vec<PathArgument>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathArgument {
    text: String,
    /// The path half of `<rev>:<path>` is relative to the repository's top
    /// level unless it starts with `./` or `../`; everything else is
    /// relative to the working directory, as git and `ls` resolve them.
    from_repository_root: bool,
    /// `ls -R`: the path must not be the repository root (§5.2).
    recursive: bool,
}

impl PathArgument {
    fn relative_to_working_directory(text: &str) -> Self {
        Self {
            text: text.to_string(),
            from_repository_root: false,
            recursive: false,
        }
    }
}

/// The two operators a read-only pipeline may use (spec 64 requirement 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    And,
    Pipe,
}

/// The command line as the shell will see it, or the reason it cannot be
/// known before the shell runs (spec 64 requirement 1).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellLine {
    /// Each simple command's words; `operators[i]` joins `commands[i]` and
    /// `commands[i + 1]`. A command may be empty (`&& ls`), which the
    /// caller refuses.
    commands: Vec<Vec<String>>,
    operators: Vec<Operator>,
    /// The first expansion found anywhere in the line. The words are still
    /// returned, with the expanding characters left in, so the caller can
    /// tell which program was meant.
    expansion: Option<&'static str>,
}

/// Splits `command` into simple commands and words the way a POSIX shell or
/// zsh would, without performing any expansion. `&&` and `|` separate
/// commands. `None` means the text uses syntax this tokenizer does not model
/// (a backslash, an unclosed quote, any other operator: `;`, `||`, `&`,
/// `|&`, a redirect), which is a parsing gap and so must ask rather than run
/// (§5.1).
fn shell_line(command: &str) -> Option<ShellLine> {
    let mut commands = Vec::new();
    let mut operators = Vec::new();
    let mut words = Vec::new();
    let mut expansion = None;
    let mut current = String::new();
    let mut in_word = false;
    // The previous unquoted character, for the positions where `~` expands.
    let mut previous: Option<char> = None;
    let mut characters = command.chars().peekable();
    let note = |found: &'static str, expansion: &mut Option<&'static str>| {
        expansion.get_or_insert(found);
    };
    while let Some(character) = characters.next() {
        match character {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
                previous = None;
                continue;
            }
            '&' | '|' => {
                let operator = match (character, characters.peek()) {
                    ('&', Some('&')) => {
                        characters.next();
                        Operator::And
                    }
                    ('|', Some('|' | '&')) => return None,
                    ('|', _) => Operator::Pipe,
                    _ => return None,
                };
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
                commands.push(std::mem::take(&mut words));
                operators.push(operator);
                previous = None;
                continue;
            }
            '\'' => {
                loop {
                    match characters.next()? {
                        '\'' => break,
                        quoted => current.push(quoted),
                    }
                }
                in_word = true;
                previous = Some('\'');
                continue;
            }
            '"' => {
                loop {
                    match characters.next()? {
                        '"' => break,
                        '$' | '`' => {
                            note("`$` or a backtick inside double quotes", &mut expansion);
                            current.push('$');
                        }
                        '\\' => return None,
                        quoted => current.push(quoted),
                    }
                }
                in_word = true;
                previous = Some('"');
                continue;
            }
            '\\' | ';' | '<' | '>' | '\n' | '\r' => return None,
            control if control.is_control() => return None,
            '$' => note("`$`", &mut expansion),
            '`' => note("a backtick", &mut expansion),
            '*' | '?' | '[' | '{' | '(' | ')' => {
                note("a glob, brace or parenthesis", &mut expansion);
            }
            // Tilde expansion happens at the start of a word and, in
            // assignments and zsh's `magic_equal_subst`, after `=` or `:`.
            // Elsewhere (`HEAD~1`) the shell leaves it alone.
            '~' if !in_word || matches!(previous, Some('=' | ':')) => {
                note("`~`", &mut expansion);
            }
            // zsh expands a word starting with `=` to a program's path
            // (`=ls` is `/bin/ls`), `^` starts a negated glob under
            // `extended_glob`, and `#` starts a comment.
            '=' | '^' | '#' if !in_word => {
                note("`=`, `^` or `#` at the start of a word", &mut expansion);
            }
            _ => {}
        }
        current.push(character);
        in_word = true;
        previous = Some(character);
    }
    if in_word {
        words.push(current);
    }
    commands.push(words);
    Some(ShellLine {
        commands,
        operators,
        expansion,
    })
}

const READ_ONLY_GIT_SUBCOMMANDS: [&str; 4] = ["status", "diff", "log", "show"];

/// git's global options that take the next word as their value, so the
/// subcommand can be found after them.
const GIT_GLOBAL_OPTIONS_WITH_VALUE: [&str; 6] = [
    "-c",
    "-C",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
];

/// Whether `words` names one of the read-only programs, whatever its
/// options. Decides between `Refused` (ask) and `NotReadOnly` (let the
/// later branches decide), so `git -C /tmp commit` keeps its old answer.
fn names_read_only_program(words: &[&str]) -> bool {
    match words.first() {
        Some(&"pwd") | Some(&"ls") => true,
        Some(&"git") => git_subcommand(&words[1..])
            .is_some_and(|(_, subcommand, _)| READ_ONLY_GIT_SUBCOMMANDS.contains(&subcommand)),
        _ => false,
    }
}

/// Splits git's arguments into its global options, the subcommand and the
/// subcommand's arguments.
fn git_subcommand<'a>(arguments: &[&'a str]) -> Option<(Vec<&'a str>, &'a str, Vec<&'a str>)> {
    let mut global = Vec::new();
    let mut index = 0;
    while let Some(word) = arguments.get(index) {
        if !word.starts_with('-') {
            return Some((global, word, arguments[index + 1..].to_vec()));
        }
        global.push(*word);
        index += if GIT_GLOBAL_OPTIONS_WITH_VALUE.contains(word) {
            2
        } else {
            1
        };
    }
    None
}

fn read_only_verdict(command: &str) -> ReadOnlyVerdict {
    let Some(line) = shell_line(command.trim()) else {
        let raw: Vec<&str> = command.split_whitespace().collect();
        return if names_read_only_program(&raw) || raw.first() == Some(&"cd") {
            ReadOnlyVerdict::Refused(
                "Command uses shell syntax the read-only check does not model (a backslash, an \
                 unclosed quote, or an operator other than `&&` and `|`), so it needs review"
                    .to_string(),
            )
        } else {
            ReadOnlyVerdict::NotReadOnly
        };
    };
    let commands: Vec<Vec<&str>> = line
        .commands
        .iter()
        .map(|words| words.iter().map(String::as_str).collect())
        .collect();
    let first = commands[0].as_slice();
    let leading_cd = first.first() == Some(&"cd") && !line.operators.is_empty();
    if !leading_cd && !names_read_only_program(first) {
        return ReadOnlyVerdict::NotReadOnly;
    }
    if let Some(expansion) = line.expansion {
        return ReadOnlyVerdict::Refused(format!(
            "Command uses shell expansion ({expansion}), so the shell would run something other \
             than what was checked"
        ));
    }
    if line.operators.is_empty() {
        return command_verdict(first);
    }
    pipeline_verdict(&commands, &line.operators, leading_cd)
}

/// A read-only pipeline (spec 64 requirement 6, §5.5): an optional leading
/// `cd <dir> &&`, then `&&` segments that are each an admitted command
/// followed by any number of `| <filter>`. The shell gives `|` precedence
/// over `&&`, so `a | b && c` is `(a | b) && c`, which is what this reads.
fn pipeline_verdict(
    commands: &[Vec<&str>],
    operators: &[Operator],
    leading_cd: bool,
) -> ReadOnlyVerdict {
    let mut admission = Admission::default();
    let mut start = 0;
    if leading_cd {
        // `cd -`, `cd` alone, zsh's two-argument `cd old new` and options
        // all go somewhere other than the word they name.
        match commands[0].as_slice() {
            ["cd", directory] if !directory.is_empty() && !directory.starts_with('-') => {
                admission.directory = Some((*directory).to_string());
            }
            _ => {
                return ReadOnlyVerdict::Refused(
                    "Only `cd <dir> &&` with a single directory is admitted before a read-only \
                     command, so the command needs review"
                        .to_string(),
                );
            }
        }
        if operators[0] != Operator::And {
            return ReadOnlyVerdict::Refused(
                "`cd` is admitted only when followed by `&&`, so the command needs review"
                    .to_string(),
            );
        }
        start = 1;
    }
    for (index, words) in commands.iter().enumerate().skip(start) {
        if words.is_empty() {
            return ReadOnlyVerdict::Refused(
                "Command has an operator with nothing on one side, so it needs review".to_string(),
            );
        }
        let after_pipe = index > 0 && operators[index - 1] == Operator::Pipe;
        if after_pipe {
            if !is_stdin_filter(words) {
                return ReadOnlyVerdict::Refused(format!(
                    "`{}` after `|` is not an admitted filter (stdin only, no file arguments), so \
                     the command needs review",
                    words.join(" ")
                ));
            }
            continue;
        }
        if !names_read_only_program(words) {
            return ReadOnlyVerdict::Refused(format!(
                "`{}` is not an admitted read-only command, so the command needs review",
                words.join(" ")
            ));
        }
        match command_verdict(words) {
            ReadOnlyVerdict::Admitted(segment) => admission.paths.extend(segment.paths),
            refused => return refused,
        }
    }
    ReadOnlyVerdict::Admitted(admission)
}

/// The stdin-only filters of spec 64 requirement 7, with their listed
/// options and no file argument, so a filter reads nothing but the output
/// of the admitted command before it.
fn is_stdin_filter(words: &[&str]) -> bool {
    let flags_from = |argument: &str, allowed: &str| {
        argument.strip_prefix('-').is_some_and(|flags| {
            !flags.is_empty() && flags.chars().all(|flag| allowed.contains(flag))
        })
    };
    let (program, arguments) = words
        .split_first()
        .expect("the caller refuses empty commands");
    match *program {
        "head" | "tail" => match arguments {
            ["-n", count] => is_count(count),
            [option] => option.strip_prefix("-n").is_some_and(is_count),
            _ => false,
        },
        "wc" => arguments.iter().all(|argument| flags_from(argument, "lwc")),
        "sort" => arguments.iter().all(|argument| flags_from(argument, "rnu")),
        "uniq" => arguments.iter().all(|argument| *argument == "-c"),
        "cut" => {
            let mut remaining = arguments.iter();
            while let Some(argument) = remaining.next() {
                let (option, attached) = match argument.get(..2) {
                    Some(option @ ("-d" | "-f")) => (option, &argument[2..]),
                    _ => return false,
                };
                let value = if attached.is_empty() {
                    match remaining.next() {
                        Some(value) => *value,
                        None => return false,
                    }
                } else {
                    attached
                };
                let admitted = if option == "-d" {
                    value.chars().count() == 1
                } else {
                    !value.is_empty()
                        && value.chars().all(|character| {
                            character.is_ascii_digit() || matches!(character, ',' | '-')
                        })
                };
                if !admitted {
                    return false;
                }
            }
            true
        }
        "grep" => {
            let (options, patterns): (Vec<&str>, Vec<&str>) = arguments
                .iter()
                .partition(|argument| argument.starts_with('-'));
            options.iter().all(|option| flags_from(option, "ivncEF")) && patterns.len() == 1
        }
        _ => false,
    }
}

/// One admitted program's form (spec 64 §5.2). The caller has checked that
/// `words` names a read-only program and the line has no expansion.
fn command_verdict(words: &[&str]) -> ReadOnlyVerdict {
    match words[0] {
        "pwd" if words.len() == 1 => ReadOnlyVerdict::Admitted(Admission::default()),
        "pwd" => refuse_argument("pwd", words[1]),
        "ls" => ls_verdict(&words[1..]),
        _ => {
            let (global, subcommand, arguments) =
                git_subcommand(&words[1..]).expect("names_read_only_program found a subcommand");
            if let Some(option) = global.iter().find(|option| **option != "--no-pager") {
                return refuse_argument("git", option);
            }
            git_verdict(subcommand, &arguments)
        }
    }
}

fn refuse_argument(program: &str, argument: &str) -> ReadOnlyVerdict {
    ReadOnlyVerdict::Refused(format!(
        "`{argument}` is not an admitted argument for a read-only `{program}`, so the command \
         needs review"
    ))
}

/// `ls` with `-a`, `-l`, `-1`, `-d`, `-h` or `-R`, alone or combined, and
/// paths (spec 64 §5.2).
fn ls_verdict(arguments: &[&str]) -> ReadOnlyVerdict {
    let mut recursive = false;
    let mut paths = Vec::new();
    for argument in arguments {
        if let Some(flags) = argument.strip_prefix('-') {
            if flags.is_empty() || !flags.chars().all(|flag| "al1dhR".contains(flag)) {
                return refuse_argument("ls", argument);
            }
            recursive |= flags.contains('R');
        } else {
            paths.push(PathArgument::relative_to_working_directory(argument));
        }
    }
    if recursive {
        if paths.is_empty() {
            return ReadOnlyVerdict::Refused(
                "`ls -R` without a directory would list the whole repository, restricted \
                 directories included, so it needs review"
                    .to_string(),
            );
        }
        for path in &mut paths {
            path.recursive = true;
        }
    }
    ReadOnlyVerdict::Admitted(Admission {
        directory: None,
        paths,
    })
}

fn is_count(word: &str) -> bool {
    !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_digit())
}

/// The subcommand's options per §5.2, then revisions, `<rev>:<path>`, `--`
/// and paths where the subcommand takes them.
fn git_verdict(subcommand: &str, arguments: &[&str]) -> ReadOnlyVerdict {
    // Spec 64 requirement 5: now that #60's `read_git_diff` exists, `git diff`
    // and `git show` no longer run without approval in the shell. They print
    // file contents, which a command line cannot bound or exclude restricted
    // paths from; the tool does both.
    if matches!(subcommand, "diff" | "show") {
        return ReadOnlyVerdict::Refused(format!(
            "`git {subcommand}` prints file contents, so it needs approval; use the \
             `read_git_diff` tool, which bounds its output and excludes restricted paths"
        ));
    }
    let program = format!("git {subcommand}");
    let mut paths = Vec::new();
    let mut index = 0;
    let mut after_separator = false;
    while let Some(argument) = arguments.get(index).copied() {
        index += 1;
        if after_separator || !argument.starts_with('-') {
            if subcommand == "status" {
                return refuse_argument(&program, argument);
            }
            match git_path_argument(argument, after_separator) {
                Some(path) => paths.extend(path),
                None => return refuse_argument(&program, argument),
            }
            continue;
        }
        if argument == "--" && subcommand != "status" {
            after_separator = true;
            continue;
        }
        let admitted = match subcommand {
            "status" => {
                matches!(
                    argument,
                    "--short"
                        | "--porcelain"
                        | "--porcelain=v1"
                        | "--branch"
                        | "--untracked-files=no"
                ) || argument.strip_prefix('-').is_some_and(|flags| {
                    !flags.is_empty() && flags.chars().all(|flag| matches!(flag, 's' | 'b'))
                })
            }
            "log" => {
                // Spec 64 requirement 5: `-p`/`--patch`/`-u` print file
                // contents; the `read_git_log` tool is the bounded, restricted
                // alternative, so these ask.
                if matches!(argument, "-p" | "--patch" | "-u") {
                    return ReadOnlyVerdict::Refused(format!(
                        "`git log {argument}` prints file contents, so it needs approval; use \
                         the `read_git_log` tool, which bounds its output and excludes restricted \
                         paths"
                    ));
                }
                if argument == "-n" {
                    let value = arguments.get(index).copied().unwrap_or_default();
                    index += 1;
                    is_count(value)
                } else {
                    matches!(
                        argument,
                        "--oneline"
                            | "--stat"
                            | "--name-only"
                            | "--name-status"
                            | "--decorate"
                            | "--graph"
                            | "--pretty"
                    ) || argument.starts_with("--format=")
                        || argument.starts_with("--pretty=")
                        || argument.strip_prefix("--max-count=").is_some_and(is_count)
                        || argument.strip_prefix("-n").is_some_and(is_count)
                        || argument.strip_prefix('-').is_some_and(is_count)
                }
            }
            // `git diff` and `git show` were refused above (requirement 5),
            // before their options were ever parsed. Anything else that
            // reaches here is refused rather than panicking: a classifier
            // that crashes on an unexpected command must fail closed.
            _ => false,
        };
        if !admitted {
            return refuse_argument(&program, argument);
        }
    }
    ReadOnlyVerdict::Admitted(Admission {
        directory: None,
        paths,
    })
}

/// A git positional argument: a revision, a range, `<rev>:<path>` or a
/// path. Every one is path-checked, since git decides between revision and
/// path only when it runs. `None` refuses it: git treats `*`, `?` and `[`
/// in a path as a glob of its own, and a pathspec starting with `:` or
/// containing `(` is magic, so neither is what the text says.
fn git_path_argument(argument: &str, after_separator: bool) -> Option<Option<PathArgument>> {
    if argument.contains(['*', '?', '[', '(']) {
        return None;
    }
    if after_separator {
        if argument.starts_with(':') {
            return None;
        }
        return Some(Some(PathArgument::relative_to_working_directory(argument)));
    }
    let Some((revision, path)) = argument.split_once(':') else {
        return Some(Some(PathArgument::relative_to_working_directory(argument)));
    };
    // `:<path>` and `:<stage>:<path>` name the index.
    let path = match path.split_once(':') {
        Some((stage, staged)) if revision.is_empty() && is_count(stage) => staged,
        _ => path,
    };
    if !revision.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || matches!(character, '.' | '/' | '_' | '-' | '~' | '^' | '@')
    }) || revision.starts_with('-')
    {
        return None;
    }
    if path.is_empty() {
        return Some(None);
    }
    Some(Some(PathArgument {
        text: path.to_string(),
        from_repository_root: !(path.starts_with("./") || path.starts_with("../")),
        recursive: false,
    }))
}

/// Why an admitted command's path argument needs approval (spec 64 §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
enum PathFinding {
    OutsideRepository,
    Restricted(String),
    RecursiveRoot,
}

/// Resolves `argument` the way the program will and checks it against the
/// repository and `restricted_patterns`. Requirement 1 has already refused
/// expansion, so the text is what the program receives. A path that exists
/// is also checked after resolving symlinks, so a link cannot lead out of
/// the repository or into a restricted directory.
fn check_path_argument(
    path_policy: &PathPolicy,
    repository_root: &Path,
    working_directory: &Path,
    argument: &PathArgument,
) -> Option<PathFinding> {
    let base = if argument.from_repository_root {
        repository_root
    } else {
        working_directory
    };
    let candidate = base.join(&argument.text);
    let root = normalize_lexically(repository_root);
    let lexical = normalize_lexically(&candidate);
    let Ok(relative) = lexical.strip_prefix(&root) else {
        return Some(PathFinding::OutsideRepository);
    };
    let mut relatives = vec![slash_separated(relative)];
    if let (Ok(canonical_root), Ok(canonical)) = (
        fs::canonicalize(repository_root),
        fs::canonicalize(&candidate),
    ) {
        match canonical.strip_prefix(&canonical_root) {
            Ok(relative) => relatives.push(slash_separated(relative)),
            Err(_) => return Some(PathFinding::OutsideRepository),
        }
    }
    let is_directory = argument.text.ends_with('/') || candidate.is_dir();
    for relative in &relatives {
        if argument.recursive && relative.is_empty() {
            return Some(PathFinding::RecursiveRoot);
        }
        if let Some(restricted) = restricted_reach(path_policy, relative, is_directory) {
            return Some(PathFinding::Restricted(restricted));
        }
    }
    None
}

/// The restricted path `relative` reaches, if any: the path itself, one of
/// its ancestors, or its contents.
///
/// The default patterns restrict a directory's contents (`secrets/**`), not
/// its name, and `ls secrets` or `git diff -- secrets` reads exactly those
/// contents. So a path whose every child would be restricted counts as
/// restricted. The synthetic child's name cannot occur in a real path, so
/// only patterns that match any name below the path fire.
fn restricted_reach(
    path_policy: &PathPolicy,
    relative: &str,
    is_directory: bool,
) -> Option<String> {
    if relative.is_empty() {
        return None;
    }
    restricted_prefix(path_policy, relative, is_directory).or_else(|| {
        path_policy
            .is_restricted(&format!("{relative}/{SYNTHETIC_CHILD}"), false)
            .then(|| relative.to_string())
    })
}

const SYNTHETIC_CHILD: &str = "\u{0}";

fn slash_separated(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The path itself or the first of its ancestors that `restricted_patterns`
/// matches: a file inside a restricted directory is restricted too.
fn restricted_prefix(
    path_policy: &PathPolicy,
    relative: &str,
    is_directory: bool,
) -> Option<String> {
    let segments: Vec<&str> = relative.split('/').collect();
    (1..=segments.len()).find_map(|length| {
        let prefix = segments[..length].join("/");
        let prefix_is_directory = length < segments.len() || is_directory;
        path_policy
            .is_restricted(&prefix, prefix_is_directory)
            .then_some(prefix)
    })
}

pub(crate) fn is_validation_command(command: &str) -> bool {
    command == "npm test"
        || command.starts_with("npm test ")
        || command.starts_with("npm run test")
        || command.starts_with("npm run lint")
        || command.starts_with("npm run typecheck")
        || command.starts_with("npm run build")
        || command.starts_with("npm run format")
        || command == "pytest"
        || command.starts_with("pytest ")
        || command.starts_with("python -m pytest")
        || command.starts_with("python3 -m pytest")
        || command.starts_with("mvn test")
        || command.starts_with("gradle test")
        || command.starts_with("go test ./...")
        || command.starts_with("cargo test")
}

fn is_docker_command(command: &str) -> bool {
    command == "docker"
        || command.starts_with("docker ")
        || command == "docker-compose"
        || command.starts_with("docker-compose ")
}

fn is_high_risk_command(command: &str) -> bool {
    command.contains("npm install")
        || command.contains("npm i ")
        || command.contains("npm add")
        || command.contains("yarn add")
        || command.contains("yarn install")
        || command.contains("pnpm add")
        || command.contains("pnpm install")
        || command.contains("pip install")
        || command.contains("curl")
        || command.contains("wget")
        || command.contains("chmod")
        || command.contains("chown")
        || command.contains("git commit")
        || command.contains("git push")
        || command.contains("git pull")
        || command.contains("git reset")
        || command.contains("git checkout")
        || command.contains("git switch")
        || command.contains("git merge")
        || command.contains("git rebase")
        || command.contains("git branch")
        || command.starts_with("sh ")
        || command.starts_with("bash ")
        || command.starts_with("zsh ")
}

fn may_use_network(command: &str) -> bool {
    [
        "curl",
        "wget",
        "npm",
        "pnpm",
        "yarn",
        "pip",
        "docker",
        "docker-compose",
        "git pull",
        "git push",
        "git fetch",
        "git clone",
    ]
    .iter()
    .any(|needle| command.contains(needle))
}

// Heuristic, not a security boundary: shell commands aren't sandboxed by path, so this only
// flags likely out-of-repo path arguments for the approval prompt shown to the user.
// Relative tokens resolve against the working directory, but the boundary is
// always the repository root: a sub-root never narrows or widens it.
fn references_path_outside_root(
    command: &str,
    repository_root: &Path,
    working_directory: &Path,
) -> bool {
    command
        .split_whitespace()
        .flat_map(|token| {
            let token = token.trim_matches(|character| matches!(character, '\'' | '"'));
            match token.split_once('=') {
                Some((_, value)) if !value.is_empty() => vec![token, value],
                _ => vec![token],
            }
        })
        .any(|token| token_escapes_root(token, repository_root, working_directory))
}

fn token_escapes_root(token: &str, repository_root: &Path, working_directory: &Path) -> bool {
    if token.is_empty() || token.starts_with('-') {
        return false;
    }
    if !token.contains('/') && !token.contains("..") {
        return false;
    }
    if token.starts_with('~') {
        return true;
    }
    let candidate = Path::new(token);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        working_directory.join(candidate)
    };
    !normalize_lexically(&absolute).starts_with(normalize_lexically(repository_root))
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        Config, allow_always_eligible, contains_shell_control, references_path_outside_root,
    };
    use std::path::Path;

    #[test]
    fn allows_permanent_approval_for_ordinary_commands() {
        let config = Config::default();
        assert!(allow_always_eligible(&config, "npm test", false));
        // High risk is still eligible: the user is explicitly opting in.
        assert!(allow_always_eligible(&config, "git push", false));
        // Leading/trailing whitespace must not change the verdict, since the
        // stored entry is trimmed too.
        assert!(allow_always_eligible(&config, "  cargo test  ", false));
    }

    #[test]
    fn refuses_permanent_approval_for_blocked_commands() {
        let config = Config::default();
        assert!(!allow_always_eligible(&config, "rm -rf /", true));
    }

    #[test]
    fn refuses_permanent_approval_for_shell_control_commands() {
        let config = Config::default();
        // The allowlist is consulted after the shell-control gate, so an entry
        // for any of these could never match.
        assert!(!allow_always_eligible(&config, "cat a.txt | grep b", false));
        assert!(!allow_always_eligible(
            &config,
            "npm test && npm run lint",
            false
        ));
        assert!(!allow_always_eligible(&config, "echo hi > out.txt", false));
        assert!(!allow_always_eligible(&config, "echo $(whoami)", false));
    }

    #[test]
    fn refuses_permanent_approval_when_all_commands_need_approval() {
        let config = Config {
            require_approval_for_all_commands: true,
            ..Config::default()
        };
        assert!(!allow_always_eligible(&config, "npm test", false));
    }

    #[test]
    fn detects_line_breaks_as_shell_control() {
        assert!(contains_shell_control("npm test\ncat /etc/passwd"));
        assert!(contains_shell_control("npm test\rcat /etc/passwd"));
    }

    #[test]
    fn detects_relative_traversal_outside_root() {
        let root = Path::new("/Users/example/project");
        assert!(references_path_outside_root(
            "cat ../secrets/id_rsa",
            root,
            root
        ));
        assert!(references_path_outside_root(
            "ls ../../other-project",
            root,
            root
        ));
    }

    #[test]
    fn detects_absolute_path_outside_root() {
        let root = Path::new("/Users/example/project");
        assert!(references_path_outside_root("cat /etc/passwd", root, root));
        assert!(references_path_outside_root(
            "cat ~/secrets.txt",
            root,
            root
        ));
    }

    /// Spec 64 §6, "Tightening". Each command is classified at a repository
    /// that exists, so symlink resolution and restricted directories are
    /// exercised for real.
    mod read_only_boundary {
        use super::super::{
            CommandClassification, CommandPolicy, CommandRisk, Operator, RESTRICTED_PATH_REASON,
            ShellLine, is_low_risk_read_only, shell_line,
        };
        use crate::config::Config;
        use std::fs;
        use std::path::PathBuf;

        struct Repository {
            root: PathBuf,
        }

        impl Repository {
            fn new(name: &str) -> Self {
                let root = std::env::temp_dir().join(format!(
                    "damaian-read-only-boundary-{name}-{}",
                    std::process::id()
                ));
                let _ = fs::remove_dir_all(&root);
                fs::create_dir_all(root.join("crates/engine")).unwrap();
                fs::create_dir_all(root.join("secrets")).unwrap();
                fs::write(root.join("secrets/token"), "x").unwrap();
                fs::write(root.join("README.md"), "readme").unwrap();
                Self { root }
            }

            fn classify(&self, command: &str) -> CommandClassification {
                CommandPolicy::new(Config::default()).classify(command, &self.root)
            }

            fn classify_in(&self, command: &str, directory: &str) -> CommandClassification {
                CommandPolicy::new(Config::default()).classify_at(
                    command,
                    &self.root,
                    &self.root.join(directory),
                )
            }
        }

        impl Drop for Repository {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.root);
            }
        }

        fn runs_unasked(classification: &CommandClassification) -> bool {
            classification.risk == CommandRisk::Low && !classification.requires_approval
        }

        /// Needs approval, and is refused in Plan and Review, which admit
        /// exactly what `is_read_only_without_approval` does (requirement 8).
        fn assert_needs_approval(classification: &CommandClassification) {
            assert!(
                classification.requires_approval,
                "{}: {:?}",
                classification.command, classification.reasons
            );
            assert!(
                !classification.is_read_only_without_approval(),
                "{} would run in Plan or Review",
                classification.command
            );
        }

        #[test]
        fn options_that_write_a_file_need_approval() {
            let repo = Repository::new("output");
            for command in ["git diff --output=README.md", "git log --output=x"] {
                assert_needs_approval(&repo.classify(command));
            }
        }

        #[test]
        fn shell_expansion_needs_approval() {
            let repo = Repository::new("expansion");
            for command in [
                "ls $HOME/.ssh",
                "git diff --no-index $HOME/x README.md",
                "ls ~",
                "ls *",
                "ls \"$HOME\"",
                "ls =ls",
                "git log --pretty=~/x",
            ] {
                let classification = repo.classify(command);
                assert_needs_approval(&classification);
                assert!(
                    classification
                        .reasons
                        .iter()
                        .any(|reason| reason.contains("shell expansion")),
                    "{command}: {:?}",
                    classification.reasons
                );
            }
            // A backtick is caught earlier, by the shell-control gate.
            assert_needs_approval(&repo.classify("git log --format=`id`"));
        }

        #[test]
        fn restricted_paths_need_approval_with_the_restricted_path_reason() {
            let repo = Repository::new("restricted");
            // Requirement 5 removed `git diff`/`git show` from the read-only
            // list, so their restricted-path checks moved to the tools; `git
            // log -- <path>` still exercises the path half here.
            for (command, restricted) in [
                ("ls secrets/", "secrets"),
                ("ls secrets", "secrets"),
                ("git log -- secrets/token", "secrets/token"),
            ] {
                let classification = repo.classify(command);
                assert_needs_approval(&classification);
                assert!(
                    classification
                        .reasons
                        .contains(&format!("{RESTRICTED_PATH_REASON}: {restricted}")),
                    "{command}: {:?}",
                    classification.reasons
                );
            }
        }

        #[test]
        fn git_options_that_change_what_git_reads_or_runs_need_approval() {
            let repo = Repository::new("git-options");
            for command in [
                "git -c core.pager=x log",
                "git -C /tmp status",
                "git --git-dir=x log",
                "git --work-tree=/tmp status",
                "git --exec-path=/tmp log",
                "git diff --no-index a b",
                "git diff --ext-diff",
                "git show --textconv HEAD",
                "git log --since=yesterday",
                "git status --ignored",
            ] {
                let classification = repo.classify(command);
                assert_needs_approval(&classification);
                assert_eq!(classification.risk, CommandRisk::High, "{command}");
            }
        }

        /// Spec 64 §6, "After #60" (requirement 5): `git diff`, `git show`
        /// and `git log -p`/`--patch`/`-u` ask, and the reason names the
        /// replacement tool.
        #[test]
        fn git_diff_show_and_log_patch_ask_and_name_the_replacement_tool() {
            let repo = Repository::new("after-60");
            for (command, tool) in [
                ("git diff", "read_git_diff"),
                ("git show HEAD", "read_git_diff"),
                ("git show --stat", "read_git_diff"),
                ("git diff --cached -- README.md", "read_git_diff"),
                ("git log -p", "read_git_log"),
                ("git log --patch -n 3", "read_git_log"),
                ("git log -u", "read_git_log"),
            ] {
                let classification = repo.classify(command);
                assert_needs_approval(&classification);
                assert_eq!(classification.risk, CommandRisk::High, "{command}");
                assert!(
                    classification
                        .reasons
                        .iter()
                        .any(|reason| reason.contains(tool)),
                    "{command} must name {tool}: {:?}",
                    classification.reasons
                );
            }
        }

        #[test]
        fn the_everyday_read_only_commands_still_run_unasked() {
            let repo = Repository::new("everyday");
            // Requirement 5: `git diff` and `git show` ask now and moved to
            // the tools (see `git_diff_show_and_log_patch_ask`), so they no
            // longer belong in the still-unasked list.
            for command in [
                "git status --short",
                "git status -sb",
                "git log --oneline -n 5",
                "git log --oneline -5",
                "git --no-pager log --stat main..HEAD",
                "git log --format='%h %s' -- crates",
                "ls crates",
                "ls -la crates/engine",
                "ls -R crates",
                "ls",
                "pwd",
            ] {
                let classification = repo.classify(command);
                assert!(
                    runs_unasked(&classification),
                    "{command}: {:?}",
                    classification.reasons
                );
                assert!(classification.is_read_only_without_approval(), "{command}");
            }
        }

        #[test]
        fn paths_outside_the_repository_need_approval() {
            let repo = Repository::new("outside");
            // Requirement 5: `git diff`/`git show` now refuse for the reason
            // that names `read_git_diff`, before their paths are inspected, so
            // their outside-path checks moved to the tools; `git log` still
            // exercises the path half here.
            for command in ["ls /etc", "ls ../", "git log /etc/passwd"] {
                let classification = repo.classify(command);
                assert_needs_approval(&classification);
                assert!(
                    classification
                        .reasons
                        .iter()
                        .any(|reason| reason.contains("outside the selected repository")),
                    "{command}: {:?}",
                    classification.reasons
                );
            }
        }

        /// `<rev>:<path>` is relative to the top level; `./` makes it
        /// relative to the working directory, as git resolves it. `git log`
        /// carries the `<rev>:<path>` half now that requirement 5 removed
        /// `git show` from the read-only list.
        #[test]
        fn a_revision_path_resolves_where_git_resolves_it() {
            let repo = Repository::new("rev-path");
            assert!(runs_unasked(
                &repo.classify_in("git log HEAD:README.md", "crates")
            ));
            assert_needs_approval(&repo.classify_in("git log HEAD:../../.env", "crates"));
            assert_needs_approval(&repo.classify_in("ls ../secrets", "crates"));
        }

        #[test]
        fn a_symlink_out_of_the_repository_needs_approval() {
            let repo = Repository::new("symlink");
            let outside = std::env::temp_dir();
            std::os::unix::fs::symlink(&outside, repo.root.join("escape")).unwrap();
            assert_needs_approval(&repo.classify("ls escape"));
        }

        #[test]
        fn ls_recursive_needs_a_directory_other_than_the_root() {
            let repo = Repository::new("recursive");
            assert_needs_approval(&repo.classify("ls -R"));
            assert_needs_approval(&repo.classify("ls -R ."));
            assert!(runs_unasked(&repo.classify("ls -R crates")));
        }

        /// Requirement 4: the directory a read-only command runs in is held
        /// to the restricted patterns, as `resolve_command_directory` does.
        #[test]
        fn a_restricted_working_directory_needs_approval() {
            let repo = Repository::new("restricted-wd");
            assert_needs_approval(&repo.classify_in("ls", "secrets"));
        }

        #[test]
        fn git_globs_and_pathspec_magic_need_approval() {
            let repo = Repository::new("pathspec");
            for command in [
                "git diff -- '*.env'",
                "git log -- ':(glob)**'",
                "git diff -- :!README.md",
            ] {
                assert_needs_approval(&repo.classify(command));
            }
        }

        /// Requirement 6 renamed `shell_words` to `shell_line`, which also
        /// splits on `&&` and `|`; a line without them is one command.
        #[test]
        fn the_tokenizer_splits_quotes_without_expanding() {
            assert_eq!(
                shell_line("git log --format='%h $x' \"a b\" ''"),
                Some(ShellLine {
                    commands: vec![vec![
                        "git".to_string(),
                        "log".to_string(),
                        "--format=%h $x".to_string(),
                        "a b".to_string(),
                        String::new(),
                    ]],
                    operators: Vec::new(),
                    expansion: None,
                })
            );
            assert_eq!(
                shell_line("git diff HEAD~1").unwrap().expansion,
                None,
                "`~` inside a word is not tilde expansion"
            );
            assert!(shell_line("ls 'unclosed").is_none());
            assert!(shell_line("ls a\\ b").is_none());
            assert!(shell_line("ls a{b,c}").unwrap().expansion.is_some());
            assert!(shell_line("ls #x").unwrap().expansion.is_some());
        }

        /// The prefix match admitted anything that started with an admitted
        /// program; the boundary does not.
        #[test]
        fn is_low_risk_read_only_judges_the_whole_command() {
            assert!(is_low_risk_read_only("git status"));
            assert!(!is_low_risk_read_only("git status --ignored"));
            assert!(!is_low_risk_read_only("git statusx"));
            assert!(!is_low_risk_read_only("pwd -P"));
            assert!(!is_low_risk_read_only("ls --color=always"));
            assert!(!is_low_risk_read_only("git status | head"));
        }

        // Spec 64 §6, "Pipelines" (requirements 6–7).

        #[test]
        fn a_cd_into_the_repository_and_a_chain_of_admitted_commands_run_unasked() {
            let repo = Repository::new("pipeline-cd");
            fs::create_dir_all(repo.root.join(".claude/worktrees/x")).unwrap();
            let classification = repo.classify(
                "cd .claude/worktrees/x && git status --short && git log --oneline -n 5 | head -n 3",
            );
            assert!(
                runs_unasked(&classification),
                "{:?}",
                classification.reasons
            );
            assert!(classification.is_read_only_without_approval());
            assert!(!classification.may_use_network);
        }

        #[test]
        fn an_admitted_command_piped_through_filters_runs_unasked() {
            let repo = Repository::new("pipeline-filters");
            for command in [
                "git log --oneline | grep -i fix | wc -l",
                "git log --oneline | head -n 3",
                "git log --oneline | tail -n3",
                "git log --name-only --format= | sort | uniq -c | sort -rn | head -n 10",
                "git status --porcelain | cut -d ' ' -f 2",
                "git log --format='%h,%s' | cut -d, -f1-2",
                "git log --oneline | grep -vE 'wip|fixup'",
                "git log --oneline | grep -F -c '[x]'",
                "ls crates | wc -lw",
                "ls&&pwd|wc -c",
                // Requirement 5 removed `git diff` from the read-only list;
                // `git log --stat` carries this pipeline segment now.
                "git status -sb && git log --stat | tail -n 1",
                // Quoted `|` is a word, not an operator.
                "git log --format='%h|%s' -n 3",
            ] {
                let classification = repo.classify(command);
                assert!(
                    runs_unasked(&classification),
                    "{command}: {:?}",
                    classification.reasons
                );
                assert!(classification.is_read_only_without_approval(), "{command}");
            }
        }

        #[test]
        fn pipelines_outside_the_admitted_shape_need_approval() {
            let repo = Repository::new("pipeline-refused");
            for command in [
                "git log --oneline | grep fix secrets.txt",
                "ls | xargs cat",
                "git status; rm x",
                "git status || true",
                "git log > out.txt",
                "cd /tmp && ls",
                // Every other operator and form.
                "git status & rm x",
                "git status |& head -n 1",
                "git status && rm x",
                "git status && npm test",
                "git status | sh",
                "git status && cd crates && ls",
                "cd crates | ls",
                "cd crates",
                "cd crates engine && ls",
                "cd - && ls",
                "cd && ls",
                "&& ls",
                "ls &&",
                "ls | | head -n 1",
                "git log < x",
                "git log $(id) | head -n 1",
                "git log | grep x *",
                "cd ~ && ls",
                // Filters with options or arguments outside requirement 7.
                "git log | head",
                "git log | head -c 10",
                "git log | tail -f",
                "git log | head -n 3 x",
                "git log | wc -m",
                "git log | sort -o out.txt",
                "git log | uniq -c out.txt",
                "git log | cut -c 1-3",
                "git log | cut -d ab -f 1",
                "git log | grep -r x",
                "git log | grep -e x",
                "git log | grep",
                "git log | grep --count x",
                // The command before `|` is still held to requirements 1–3.
                "git log --output=x | head -n 1",
                "git show HEAD:.env | head -n 1",
            ] {
                assert_needs_approval(&repo.classify(command));
            }
        }

        #[test]
        fn a_cd_is_held_to_the_working_directory_rules() {
            let repo = Repository::new("pipeline-cd-rules");
            let restricted = repo.classify("cd secrets && ls");
            assert_needs_approval(&restricted);
            assert!(
                restricted
                    .reasons
                    .contains(&format!("{RESTRICTED_PATH_REASON}: secrets")),
                "{:?}",
                restricted.reasons
            );
            let outside = repo.classify("cd .. && ls");
            assert_needs_approval(&outside);
            assert!(
                outside
                    .reasons
                    .iter()
                    .any(|reason| reason.contains("outside the selected repository")),
                "{:?}",
                outside.reasons
            );
            // A directory that does not exist could be found through `CDPATH`.
            assert_needs_approval(&repo.classify("cd missing && ls"));
            let outside_directory = std::env::temp_dir();
            std::os::unix::fs::symlink(&outside_directory, repo.root.join("escape")).unwrap();
            assert_needs_approval(&repo.classify("cd escape && ls"));
            // From a sub-root, `cd` resolves against the working directory.
            assert!(runs_unasked(&repo.classify_in("cd engine && ls", "crates")));
            assert_needs_approval(&repo.classify_in("cd ../secrets && ls", "crates"));
        }

        #[test]
        fn paths_after_a_cd_resolve_in_the_cd_directory() {
            let repo = Repository::new("pipeline-cd-paths");
            assert!(runs_unasked(&repo.classify("cd crates && ls engine")));
            // The path half of `<rev>:<path>` is still from the top level;
            // requirement 5 moved the example from `git show` to `git log`.
            assert!(runs_unasked(
                &repo.classify("cd crates && git log HEAD:README.md | head -n 5")
            ));
            let restricted = repo.classify("cd crates && ls ../secrets");
            assert_needs_approval(&restricted);
            assert!(
                restricted
                    .reasons
                    .contains(&format!("{RESTRICTED_PATH_REASON}: secrets")),
                "{:?}",
                restricted.reasons
            );
            assert_needs_approval(&repo.classify("cd crates && ls ../.."));
            assert_needs_approval(&repo.classify("cd crates && ls -R .."));
        }

        /// §5.5: a pipeline is Low like a single admitted command, so it
        /// asks exactly when every command does.
        #[test]
        fn a_pipeline_asks_when_every_command_must() {
            let repo = Repository::new("pipeline-all");
            let classification = CommandPolicy::new(Config {
                require_approval_for_all_commands: true,
                ..Config::default()
            })
            .classify("git log --oneline | head -n 3", &repo.root);
            assert_eq!(classification.risk, CommandRisk::Low);
            assert!(classification.requires_approval);
        }

        #[test]
        fn the_tokenizer_splits_on_and_and_pipe_only() {
            let line = shell_line("cd x&&git log 'a|b' | head -n 1").unwrap();
            assert_eq!(
                line.commands,
                vec![
                    vec!["cd".to_string(), "x".to_string()],
                    vec!["git".to_string(), "log".to_string(), "a|b".to_string()],
                    vec!["head".to_string(), "-n".to_string(), "1".to_string()],
                ]
            );
            assert_eq!(line.operators, vec![Operator::And, Operator::Pipe]);
            for refused in [
                "a || b", "a |& b", "a & b", "a &| b", "a; b", "a > b", "a\nb",
            ] {
                assert!(shell_line(refused).is_none(), "{refused}");
            }
        }

        #[test]
        fn is_low_risk_read_only_judges_the_whole_pipeline() {
            assert!(is_low_risk_read_only("git status | head -n 1"));
            assert!(is_low_risk_read_only("cd crates && ls"));
            assert!(!is_low_risk_read_only("git status | head -n 1 && rm x"));
        }
    }

    #[test]
    fn does_not_flag_paths_inside_root() {
        let root = Path::new("/Users/example/project");
        assert!(!references_path_outside_root("cat src/main.rs", root, root));
        assert!(!references_path_outside_root(
            "cat /Users/example/project/src/main.rs",
            root,
            root
        ));
        assert!(!references_path_outside_root(
            "git status --short",
            root,
            root
        ));
    }
}
