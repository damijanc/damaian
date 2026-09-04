//! The data directory's schema marker, per
//! `docs/specs/15_install_and_update_verification.md` §5.2.
//!
//! The data directory holds `config/`, `sessions/`, `audit/`, `patches/`,
//! `rollback/`, and `models/`, all read by convention. Nothing recorded which
//! layout wrote them, so a version boundary was undetectable: an older build
//! meeting data a newer one had reorganised would write straight over it. This
//! module makes the boundary explicit and refuses to cross it in the direction
//! that loses data.
//!
//! The refusal path matters more than the migration path. There is no migration
//! yet — version 1 is the current on-disk layout — only somewhere for the next
//! format change to go, with a test harness already pointed at it.

use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

/// The layout this build understands. Bump it in the same change that alters
/// a persisted format, and add the migration for the boundary it creates.
pub const CURRENT_DATA_SCHEMA_VERSION: u32 = 1;

/// The marker file, in the same flat `key=value` format as `config/user.conf`
/// so it needs no new parser and stays readable and hand-editable.
const MARKER_FILE: &str = "schema.conf";
const VERSION_KEY: &str = "schema_version";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSchemaOutcome {
    /// No data directory, or an empty one. Normal first run.
    Initialized,
    /// Existing content with no marker: every install that predates the
    /// marker. Adopted as version 1, which must not be disruptive.
    Adopted,
    /// The marker already reads as this build's version.
    Current,
    /// The marker was older; its migrations ran and it now reads as this
    /// build's version.
    Upgraded { from: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSchemaError {
    /// The directory was written by a layout this build does not understand,
    /// or its marker cannot be read as a version. Nothing was changed.
    Unsupported {
        data_dir: PathBuf,
        found: String,
        supported: u32,
    },
    Io(String),
}

impl Display for DataSchemaError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported {
                data_dir,
                found,
                supported,
            } => write!(
                formatter,
                "The data directory at {} declares data schema version \"{found}\", \
                 and this version of Damaian supports version {supported}. \
                 Nothing in it was changed. Run a version of Damaian that supports \
                 it, or point DAMAIAN_DATA_DIR at a different directory.",
                data_dir.display()
            ),
            Self::Io(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for DataSchemaError {}

impl From<std::io::Error> for DataSchemaError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// Resolves the data directory's schema version, and either brings it to this
/// build's version or refuses to touch it.
///
/// Call this once at startup, before anything reads or writes the directory,
/// and report an error rather than continuing: an empty projects list reads as
/// data loss and prompts exactly the destructive recovery attempt this exists
/// to prevent.
pub fn ensure_data_dir_schema(
    data_dir: &Path,
) -> std::result::Result<DataSchemaOutcome, DataSchemaError> {
    let marker = data_dir.join(MARKER_FILE);
    if !marker.exists() {
        // Nothing has been read or written yet, so this is the one branch that
        // may create the directory.
        let adopted = has_existing_content(data_dir);
        fs::create_dir_all(data_dir)?;
        write_marker(&marker, CURRENT_DATA_SCHEMA_VERSION)?;
        return Ok(if adopted {
            DataSchemaOutcome::Adopted
        } else {
            DataSchemaOutcome::Initialized
        });
    }

    let content = fs::read_to_string(&marker)?;
    let found = parse_version(&content);
    let unsupported = |found: String| DataSchemaError::Unsupported {
        data_dir: data_dir.to_path_buf(),
        found,
        supported: CURRENT_DATA_SCHEMA_VERSION,
    };
    // A marker that cannot be read as a known version is refused rather than
    // treated as version 0 or as missing: overwriting it would destroy the only
    // record of what wrote the directory.
    let Some(version) = found else {
        return Err(unsupported(recorded_value(&content)));
    };
    if version == 0 || version > CURRENT_DATA_SCHEMA_VERSION {
        return Err(unsupported(version.to_string()));
    }
    if version == CURRENT_DATA_SCHEMA_VERSION {
        return Ok(DataSchemaOutcome::Current);
    }

    migrate(data_dir, version)?;
    write_marker(&marker, CURRENT_DATA_SCHEMA_VERSION)?;
    Ok(DataSchemaOutcome::Upgraded { from: version })
}

/// Brings a directory written by `from` up to [`CURRENT_DATA_SCHEMA_VERSION`].
/// No format has changed yet, so there is no boundary to cross; the next one
/// adds its arm here.
fn migrate(_data_dir: &Path, _from: u32) -> std::result::Result<(), DataSchemaError> {
    Ok(())
}

fn write_marker(marker: &Path, version: u32) -> std::result::Result<(), DataSchemaError> {
    fs::write(marker, format!("{VERSION_KEY}={version}\n"))?;
    Ok(())
}

/// True when the directory holds anything at all. An existing install has
/// `config/` or `sessions/` here; an empty or absent directory is a first run.
fn has_existing_content(data_dir: &Path) -> bool {
    fs::read_dir(data_dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

fn parse_version(content: &str) -> Option<u32> {
    recorded_line(content)?.parse().ok()
}

/// What the marker records, for the refusal message. Empty when the marker has
/// no `schema_version` line at all.
fn recorded_value(content: &str) -> String {
    recorded_line(content).unwrap_or_default().to_string()
}

fn recorded_line(content: &str) -> Option<&str> {
    content.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        line.strip_prefix(VERSION_KEY)?
            .trim_start()
            .strip_prefix('=')
            .map(str::trim)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_version_from_the_marker_format() {
        assert_eq!(parse_version("schema_version=1\n"), Some(1));
        assert_eq!(
            parse_version("# written by 0.31.0\nschema_version = 7"),
            Some(7)
        );
    }

    #[test]
    fn does_not_parse_a_commented_out_or_absent_version() {
        assert_eq!(parse_version("#schema_version=1\n"), None);
        assert_eq!(parse_version("other_key=1\n"), None);
        assert_eq!(parse_version("schema_version=banana"), None);
    }

    #[test]
    fn reports_what_an_unparsable_marker_recorded() {
        assert_eq!(recorded_value("schema_version=banana\n"), "banana");
        assert_eq!(recorded_value("nothing useful\n"), "");
    }
}
