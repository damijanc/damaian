use crate::config::{Config, DEFAULT_RESTRICTED_PATTERNS};
use crate::error::{ClientError, Result};
use crate::ignore::{IgnoreRule, is_ignored_by_rules, parse_ignore_patterns};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ResolvedPath {
    pub root: PathBuf,
    pub absolute_path: PathBuf,
    pub relative_path: String,
}

#[derive(Debug, Clone)]
pub struct PathPolicy {
    allowed_roots: Vec<PathBuf>,
    restricted_rules: Vec<IgnoreRule>,
}

impl PathPolicy {
    pub fn new(config: &Config) -> Self {
        Self {
            allowed_roots: config.allowed_roots.clone(),
            restricted_rules: parse_ignore_patterns(&config.restricted_patterns, ""),
        }
    }

    pub fn unrestricted() -> Self {
        Self {
            allowed_roots: Vec::new(),
            restricted_rules: parse_ignore_patterns(
                &DEFAULT_RESTRICTED_PATTERNS
                    .iter()
                    .map(|pattern| pattern.to_string())
                    .collect::<Vec<_>>(),
                "",
            ),
        }
    }

    pub fn canonical_root(&self, root_path: impl AsRef<Path>) -> Result<PathBuf> {
        let root = fs::canonicalize(root_path)?;
        if self.allowed_roots.is_empty() {
            return Ok(root);
        }

        for allowed_root in &self.allowed_roots {
            let allowed = fs::canonicalize(allowed_root)?;
            if is_inside(&allowed, &root) {
                return Ok(root);
            }
        }
        Err(ClientError::AccessDenied(
            "Repository root is not allowed".to_string(),
        ))
    }

    pub fn resolve_existing(
        &self,
        root_path: impl AsRef<Path>,
        requested_path: impl AsRef<Path>,
        allow_outside_root: bool,
    ) -> Result<ResolvedPath> {
        let root = self.canonical_root(root_path)?;
        let candidate = if requested_path.as_ref().is_absolute() {
            requested_path.as_ref().to_path_buf()
        } else {
            root.join(requested_path)
        };
        let absolute_path = fs::canonicalize(candidate)?;
        if !is_inside(&root, &absolute_path) {
            if !allow_outside_root {
                return Err(ClientError::AccessDenied(
                    "Path resolves outside the selected repository".to_string(),
                ));
            }
            return Ok(ResolvedPath {
                relative_path: absolute_path.to_string_lossy().replace('\\', "/"),
                root,
                absolute_path,
            });
        }
        Ok(ResolvedPath {
            relative_path: relative_path(&root, &absolute_path)?,
            root,
            absolute_path,
        })
    }

    pub fn resolve_for_write(
        &self,
        root_path: impl AsRef<Path>,
        requested_path: impl AsRef<Path>,
    ) -> Result<ResolvedPath> {
        let root = self.canonical_root(root_path)?;
        let absolute_path = if requested_path.as_ref().is_absolute() {
            requested_path.as_ref().to_path_buf()
        } else {
            root.join(requested_path)
        };

        if !is_inside(&root, &absolute_path) {
            return Err(ClientError::AccessDenied(
                "Write target is outside the selected repository".to_string(),
            ));
        }

        let absolute_path = match fs::symlink_metadata(&absolute_path) {
            Ok(_) => {
                let resolved = fs::canonicalize(&absolute_path)?;
                if !is_inside(&root, &resolved) {
                    return Err(ClientError::AccessDenied(
                        "Write target resolves outside the selected repository".to_string(),
                    ));
                }
                resolved
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let ancestor = nearest_existing_ancestor(&absolute_path, &root)?;
                if !is_inside(&root, &ancestor) {
                    return Err(ClientError::AccessDenied(
                        "Write target resolves outside the selected repository".to_string(),
                    ));
                }
                assert_existing_components_stay_inside(&root, &absolute_path)?;
                absolute_path
            }
            Err(error) => return Err(ClientError::from(error)),
        };

        Ok(ResolvedPath {
            relative_path: relative_path(&root, &absolute_path)?,
            root,
            absolute_path,
        })
    }

    pub fn is_restricted(&self, relative_path: &str, is_directory: bool) -> bool {
        is_ignored_by_rules(&self.restricted_rules, relative_path, is_directory)
    }

    pub fn assert_not_restricted(&self, relative_path: &str, allow_restricted: bool) -> Result<()> {
        if !allow_restricted && self.is_restricted(relative_path, false) {
            return Err(ClientError::AccessDenied(
                "Path is restricted by policy".to_string(),
            ));
        }
        Ok(())
    }
}

fn is_inside(root: &Path, target: &Path) -> bool {
    target.starts_with(root)
}

fn relative_path(root: &Path, target: &Path) -> Result<String> {
    let relative = target
        .strip_prefix(root)
        .map_err(|error| ClientError::AccessDenied(error.to_string()))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn nearest_existing_ancestor(target: &Path, root: &Path) -> Result<PathBuf> {
    let mut current = target.parent().unwrap_or(root).to_path_buf();
    loop {
        match fs::canonicalize(&current) {
            Ok(path) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if current == root || !current.pop() {
                    return Err(ClientError::from(error));
                }
            }
            Err(error) => return Err(ClientError::from(error)),
        }
    }
}

fn assert_existing_components_stay_inside(root: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(root)
        .map_err(|error| ClientError::AccessDenied(error.to_string()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    let resolved = fs::canonicalize(&current)?;
                    if !is_inside(root, &resolved) {
                        return Err(ClientError::AccessDenied(
                            "Write target resolves outside the selected repository".to_string(),
                        ));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(ClientError::from(error)),
        }
    }
    Ok(())
}
