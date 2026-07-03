use crate::file_access::FileAccessController;
use crate::indexer::RepositoryIndex;
use crate::secret_scanner::SecretScanner;
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

        for path in explicit_paths {
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
