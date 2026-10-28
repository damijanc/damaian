//! Branch naming, and later branch delivery (spec 36).
//!
//! Naming is the only part of this spec that does not change a ref. It is kept
//! here rather than in `git_service.rs` because it is a Damaian convention
//! built on Git's validation, not another Git read. The rest of the module —
//! the prepared record and the mutations — arrives in later tasks.

use crate::error::{ClientError, Result};
use crate::git_service::GitService;
use crate::secret_scanner::SecretScanner;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A branch slug stops here so the composed name stays comfortably under Git's
/// 255-byte ref-component limit even with a long configured prefix.
pub const BRANCH_SLUG_MAX_CHARS: usize = 50;

/// A prefix is naming, not a document; anything longer is a configuration
/// mistake rather than a convention.
const BRANCH_PREFIX_MAX_CHARS: usize = 200;

/// How many `-2`, `-3`, … alternatives are searched before giving up. A
/// collision this deep is almost certainly a naming loop, not a branch.
const MAX_COLLISION_SUFFIX: u32 = 99;

/// A suggested branch name and what the user needs to know before accepting it.
/// The name is never silently changed (spec 36 §5.2): a collision leaves `name`
/// as it was and offers `alternative`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSuggestion {
    pub name: String,
    /// Whether Git accepts `name`.
    pub valid: bool,
    /// Git's refusal text, when `valid` is false.
    pub refusal: Option<String>,
    /// Whether `name` already exists locally.
    pub exists: bool,
    /// The first free `<name>-N`, when `exists` is true.
    pub alternative: Option<String>,
}

/// Lowercase ASCII words joined by `-`, at most [`BRANCH_SLUG_MAX_CHARS`]
/// characters, and `change` when nothing is left (spec 36 §5.2). Every
/// non-alphanumeric run collapses to one `-`; non-ASCII characters are
/// separators rather than content, matching `worktree::worker_slug`.
pub fn slugify_branch_title(text: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(character.to_ascii_lowercase());
        } else {
            separator = true;
        }
    }
    if slug.len() > BRANCH_SLUG_MAX_CHARS {
        slug.truncate(BRANCH_SLUG_MAX_CHARS);
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "change".to_string()
    } else {
        slug
    }
}

/// Whether `prefix` can be put in front of a slug without changing what the
/// name means. This runs before `git check-ref-format --branch`, because
/// `--branch` expands `@{-N}` and similar syntax: a prefix containing `@{`
/// could be interpreted as a previous branch instead of validated as text
/// (spec 36 `context.md` §3.14).
///
/// A trailing `/` is allowed and expected (`damaian/`); an empty prefix is
/// allowed and simply means no prefix.
pub fn validate_branch_prefix(prefix: &str) -> Result<()> {
    if prefix.is_empty() {
        return Ok(());
    }
    let invalid = prefix.chars().count() > BRANCH_PREFIX_MAX_CHARS
        || prefix.starts_with('-')
        || prefix.starts_with('/')
        || prefix.ends_with('.')
        || prefix.contains("..")
        || prefix.contains("@{")
        || prefix.contains("//")
        || prefix.chars().any(|character| {
            character.is_control()
                || matches!(character, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
        || prefix.trim_end_matches('/').split('/').any(|part| {
            part.is_empty()
                || part.starts_with('.')
                || part.ends_with('.')
                || part.ends_with(".lock")
        });
    if invalid {
        return Err(ClientError::InvalidInput(format!(
            "`{prefix}` is not a branch-name prefix Git would accept"
        )));
    }
    Ok(())
}

/// Suggest a branch name from the task title or the user's request.
///
/// `source` is free text and can contain a credential, so it is redacted before
/// it is slugged and before anything is returned (spec 36 `context.md` §3.15,
/// decision D10). The composed name is validated by Git and checked against the
/// local branches; a collision is reported with a free alternative rather than
/// silently suffixing the name.
pub fn suggest_branch_name(
    git: &GitService,
    scanner: &SecretScanner,
    root: impl AsRef<Path>,
    prefix: &str,
    source: &str,
) -> Result<BranchSuggestion> {
    let root = root.as_ref();
    validate_branch_prefix(prefix)?;
    let redacted = scanner.redact(source);
    let name = format!("{prefix}{}", slugify_branch_title(&redacted.text));
    let check = git.check_branch_name(root, &name)?;
    if !check.valid {
        return Ok(BranchSuggestion {
            name,
            valid: false,
            refusal: check.reason,
            exists: false,
            alternative: None,
        });
    }
    let branches = git.local_branches(root)?;
    if !branches.contains(&name) {
        return Ok(BranchSuggestion {
            name,
            valid: true,
            refusal: None,
            exists: false,
            alternative: None,
        });
    }
    let alternative = (2..=MAX_COLLISION_SUFFIX)
        .map(|suffix| format!("{name}-{suffix}"))
        .find(|candidate| !branches.contains(candidate));
    Ok(BranchSuggestion {
        name,
        valid: true,
        refusal: None,
        exists: true,
        alternative,
    })
}
