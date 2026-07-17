use crate::config::Config;
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

#[derive(Debug, Clone)]
pub struct ProjectCommand {
    pub name: String,
    pub command: String,
    pub risk: CommandRisk,
}

#[derive(Debug, Clone)]
pub struct CommandPolicy {
    config: Config,
}

impl CommandPolicy {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn classify(&self, command: &str, working_directory: &Path) -> CommandClassification {
        let mut classification = self.classify_pattern(command);
        if !classification.blocked
            && references_path_outside_root(&classification.command, working_directory)
        {
            classification.requires_approval = true;
            if classification.risk == CommandRisk::Low {
                classification.risk = CommandRisk::Medium;
            }
            classification.reasons.push(
                "Command references a path outside the selected repository".to_string(),
            );
        }
        classification
    }

    fn classify_pattern(&self, command: &str) -> CommandClassification {
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

        if configured_exact_matches(&self.config.command_allowlist, &normalized) {
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

fn is_low_risk_read_only(command: &str) -> bool {
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
fn references_path_outside_root(command: &str, working_directory: &Path) -> bool {
    command
        .split_whitespace()
        .flat_map(|token| {
            let token = token.trim_matches(|character| matches!(character, '\'' | '"'));
            match token.split_once('=') {
                Some((_, value)) if !value.is_empty() => vec![token, value],
                _ => vec![token],
            }
        })
        .any(|token| token_escapes_root(token, working_directory))
}

fn token_escapes_root(token: &str, working_directory: &Path) -> bool {
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
    !normalize_lexically(&absolute).starts_with(normalize_lexically(working_directory))
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
    use super::{contains_shell_control, references_path_outside_root};
    use std::path::Path;

    #[test]
    fn detects_line_breaks_as_shell_control() {
        assert!(contains_shell_control("npm test\ncat /etc/passwd"));
        assert!(contains_shell_control("npm test\rcat /etc/passwd"));
    }

    #[test]
    fn detects_relative_traversal_outside_root() {
        let root = Path::new("/Users/example/project");
        assert!(references_path_outside_root("cat ../secrets/id_rsa", root));
        assert!(references_path_outside_root("ls ../../other-project", root));
    }

    #[test]
    fn detects_absolute_path_outside_root() {
        let root = Path::new("/Users/example/project");
        assert!(references_path_outside_root("cat /etc/passwd", root));
        assert!(references_path_outside_root("cat ~/secrets.txt", root));
    }

    #[test]
    fn does_not_flag_paths_inside_root() {
        let root = Path::new("/Users/example/project");
        assert!(!references_path_outside_root("cat src/main.rs", root));
        assert!(!references_path_outside_root(
            "cat /Users/example/project/src/main.rs",
            root
        ));
        assert!(!references_path_outside_root("git status --short", root));
    }
}
