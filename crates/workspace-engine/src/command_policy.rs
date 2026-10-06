use crate::config::{CommandAccess, Config};
use crate::error::Result;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
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
        let mut classification = self.classify_pattern(command, location.as_deref());
        if !classification.blocked && location.is_none() {
            escalate(
                &mut classification,
                "Working directory is outside the selected repository",
            );
        }
        if !classification.blocked
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
    /// `Some("")` at the root, `None` outside the repository.
    fn classify_pattern(&self, command: &str, location: Option<&str>) -> CommandClassification {
        let normalized = command.trim().to_string();

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
            return CommandClassification {
                command: normalized,
                risk: CommandRisk::High,
                blocked: false,
                requires_approval: true,
                reasons: vec![
                    "Command contains shell control syntax and needs explicit review".to_string(),
                ],
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

        if is_low_risk_read_only(&normalized) {
            return CommandClassification {
                command: normalized,
                risk: CommandRisk::Low,
                blocked: false,
                requires_approval: self.config.require_approval_for_all_commands,
                reasons: vec!["Read-only command".to_string()],
                expected_effects: "Reads workspace or Git metadata".to_string(),
                may_use_network: false,
            };
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

    pub fn detect_project_commands(
        &self,
        root_path: impl AsRef<Path>,
    ) -> Result<Vec<ProjectCommand>> {
        let root = root_path.as_ref();
        let mut commands = Vec::new();
        let package_path = root.join("package.json");
        if let Ok(package_json) = fs::read_to_string(package_path) {
            for name in ["test", "lint", "typecheck", "build", "format"] {
                if package_json.contains(&format!("\"{name}\"")) {
                    let command = format!("npm run {name}");
                    commands.push(ProjectCommand {
                        name: name.to_string(),
                        risk: self.classify(&command, root).risk,
                        command,
                    });
                }
            }
            if package_json.contains("\"test\"") {
                commands.push(ProjectCommand {
                    name: "test-shortcut".to_string(),
                    command: "npm test".to_string(),
                    risk: self.classify("npm test", root).risk,
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
            if root.join(file_name).exists() {
                commands.push(ProjectCommand {
                    name: file_name.to_string(),
                    command: command.to_string(),
                    risk: self.classify(command, root).risk,
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

pub(crate) fn is_low_risk_read_only(command: &str) -> bool {
    command == "pwd"
        || command == "ls"
        || command.starts_with("ls ")
        || command == "git status"
        || command.starts_with("git status ")
        || command == "git diff"
        || command.starts_with("git diff ")
        || command == "git log"
        || command.starts_with("git log ")
        || command == "git show"
        || command.starts_with("git show ")
}

fn is_validation_command(command: &str) -> bool {
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
