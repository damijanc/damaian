use crate::file_access::FileAccessController;
use crate::indexer::RepositoryIndex;
use crate::secret_scanner::SecretScanner;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const PROJECT_RULES: &[&str] = &[
    "AGENTS.md",
    "README.md",
    "CONTRIBUTING.md",
    ".editorconfig",
    "package.json",
    "pyproject.toml",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextItem {
    pub kind: String,
    pub path: Option<String>,
    pub content: String,
    pub tokens: usize,
    pub redaction_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPlan {
    pub repository_id: String,
    pub task_id: String,
    pub token_estimate: usize,
    pub items: Vec<ContextItem>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ContextManager {
    file_access: FileAccessController,
    scanner: SecretScanner,
}

impl ContextManager {
    pub fn new(file_access: FileAccessController, scanner: SecretScanner) -> Self {
        Self {
            file_access,
            scanner,
        }
    }

    pub fn build_context(
        &self,
        repository_root: impl AsRef<Path>,
        repository_id: &str,
        task_id: &str,
        prompt: &str,
        index: Option<&RepositoryIndex>,
        explicit_paths: &[String],
        token_budget: usize,
    ) -> ContextPlan {
        let mut items = Vec::new();
        let mut files = Vec::new();
        let mut token_estimate = 0;

        add_text(
            &self.scanner,
            &mut items,
            &mut token_estimate,
            token_budget,
            "user_prompt",
            None,
            prompt,
        );

        let mut requested_paths: Vec<(String, bool)> = explicit_paths
            .iter()
            .map(|path| (path.clone(), true))
            .collect();
        for path in prompt_file_mentions(prompt, index) {
            if !requested_paths.iter().any(|(existing, _)| existing == &path) {
                requested_paths.push((path, false));
            }
        }

        for (path, allow_outside_root) in &requested_paths {
            self.add_file(
                repository_root.as_ref(),
                repository_id,
                task_id,
                path,
                "explicit_file",
                &mut files,
                &mut items,
                &mut token_estimate,
                token_budget,
                *allow_outside_root,
            );
        }

        for rule_path in PROJECT_RULES {
            self.add_file(
                repository_root.as_ref(),
                repository_id,
                task_id,
                rule_path,
                "project_rule",
                &mut files,
                &mut items,
                &mut token_estimate,
                token_budget,
                false,
            );
        }

        if let Some(index) = index {
            let mut results = index.keyword_search(prompt, 8);
            results.extend(index.semantic_search(prompt, 8));
            for result in results {
                self.add_file(
                    repository_root.as_ref(),
                    repository_id,
                    task_id,
                    &result.path,
                    "retrieved_file",
                    &mut files,
                    &mut items,
                    &mut token_estimate,
                    token_budget,
                    false,
                );
            }
        }

        ContextPlan {
            repository_id: repository_id.to_string(),
            task_id: task_id.to_string(),
            token_estimate,
            items,
            files,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_file(
        &self,
        repository_root: &Path,
        repository_id: &str,
        task_id: &str,
        path: &str,
        kind: &str,
        files: &mut Vec<String>,
        items: &mut Vec<ContextItem>,
        token_estimate: &mut usize,
        token_budget: usize,
        allow_outside_root: bool,
    ) {
        if files.iter().any(|existing| existing == path) {
            return;
        }
        let Ok(file) = self.file_access.read_file(
            repository_root,
            path,
            Some(task_id),
            Some(repository_id),
            false,
            allow_outside_root,
        ) else {
            return;
        };
        let added = add_text(
            &self.scanner,
            items,
            token_estimate,
            token_budget,
            kind,
            Some(file.path.clone()),
            &file.content,
        );
        if added {
            files.push(file.path);
        }
    }
}

fn add_text(
    scanner: &SecretScanner,
    items: &mut Vec<ContextItem>,
    token_estimate: &mut usize,
    token_budget: usize,
    kind: &str,
    path: Option<String>,
    content: &str,
) -> bool {
    if content.is_empty() {
        return false;
    }
    let redaction = scanner.redact(content);
    let tokens = redaction.text.len().div_ceil(4);
    if *token_estimate + tokens > token_budget {
        return false;
    }
    *token_estimate += tokens;
    items.push(ContextItem {
        kind: kind.to_string(),
        path,
        content: redaction.text,
        tokens,
        redaction_status: if redaction.findings.is_empty() {
            "clean".to_string()
        } else {
            "redacted".to_string()
        },
    });
    true
}

fn prompt_file_mentions(prompt: &str, index: Option<&RepositoryIndex>) -> Vec<String> {
    let Some(index) = index else {
        return Vec::new();
    };

    let paths = index
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let exact_paths = paths
        .iter()
        .map(|path| (path.to_lowercase(), path.clone()))
        .collect::<HashMap<_, _>>();
    let mut basename_matches: HashMap<String, Vec<String>> = HashMap::new();
    for path in &paths {
        if let Some(name) = path.rsplit('/').next() {
            basename_matches
                .entry(name.to_lowercase())
                .or_default()
                .push(path.clone());
        }
    }

    let mut mentioned = Vec::new();
    let mut seen = HashSet::new();
    for candidate in prompt_path_candidates(prompt) {
        let lower = candidate.to_lowercase();
        let resolved = if let Some(path) = exact_paths.get(&lower) {
            Some(path.clone())
        } else if !candidate.contains('/') {
            basename_matches
                .get(&lower)
                .filter(|matches| matches.len() == 1)
                .and_then(|matches| matches.first().cloned())
        } else {
            None
        };

        if let Some(path) = resolved {
            if seen.insert(path.clone()) {
                mentioned.push(path);
            }
        }
    }

    mentioned
}

fn prompt_path_candidates(prompt: &str) -> Vec<String> {
    prompt
        .split_whitespace()
        .filter_map(|part| {
            let candidate = part
                .trim_matches(|character: char| {
                    matches!(
                        character,
                        '`' | '"'
                            | '\''
                            | '('
                            | ')'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | '<'
                            | '>'
                            | ','
                            | ':'
                            | ';'
                    )
                })
                .trim_end_matches(|character: char| matches!(character, '.' | '?' | '!'))
                .replace('\\', "/");

            if candidate.is_empty()
                || candidate.starts_with('/')
                || candidate.starts_with("http://")
                || candidate.starts_with("https://")
                || candidate.contains("../")
                || candidate == ".."
                || candidate.ends_with('/')
            {
                return None;
            }

            let looks_like_path = candidate.contains('/')
                || candidate
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.contains('.'));
            looks_like_path.then_some(candidate)
        })
        .collect()
}
