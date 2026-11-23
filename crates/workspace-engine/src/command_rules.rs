//! Command rules (spec 111 D1, D2, D5): an ordered list of `allow`, `ask` and
//! `deny` rules over parsed commands, evaluated part by part under the
//! user's authority.
//!
//! A rule is written `command_rule = <effect> <pattern> [in <directory>]`.
//! The pattern is parsed with the same parser as the command
//! ([`crate::command_parse`]), so option spellings and paths compare
//! normalised: `deny rm -rf /tmp` also covers `rm --force -r /tmp/`. In a
//! pattern `*` matches within one word or path segment, `**` any number of
//! path segments (a trailing `**` at least one), and a trailing ` *` word any
//! further arguments, options included. A rule's relative paths resolve where
//! the command runs; `in <directory>` (absolute or `~`) limits where that is.
//!
//! Evaluation per part, last match wins within each source's authority:
//! Damaian's built-in `deny` defaults first, then repository rules, profile
//! rules, the user's rules, admin rules, and session rules. Repository and
//! profile rules only move a decision toward `ask`/`deny`; the user's (and
//! admin's) rules are final whichever way they point; a session rule only
//! allows, only the exact command it was approved for, and never lifts a
//! `deny`. A chain is denied if any part is, asks if any part asks, and is
//! allowed only when every part is.
//!
//! Nothing here changes a decision yet: until spec 111 Task 4 the command
//! policy (`command_policy.rs`) decides, and this path sits beside it.

use crate::command_parse::{
    Argument, Chain, ChainOperator, CommandOption, CommandParse, CommandPart, ParseContext,
    PartShape, ShellParameters, displayed, parse_command,
};
use crate::command_policy::normalize_lexically;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleEffect {
    Allow,
    Ask,
    Deny,
}

impl RuleEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }

    fn verdict(self) -> RuleVerdict {
        match self {
            Self::Allow => RuleVerdict::Allow,
            Self::Ask => RuleVerdict::Ask,
            Self::Deny => RuleVerdict::Deny,
        }
    }
}

/// Where a rule came from, which decides how far it may move a decision
/// (D2). Damaian's built-in defaults are not rules of this kind; they are
/// [`BuiltInDefault`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleSource {
    /// Untrusted: arrives with a clone. `ask` and `deny` only.
    Repository,
    /// The selected permission profile. `ask` and `deny` only.
    Profile,
    User,
    /// The administrator's config, with the user's authority, applied after
    /// the user's own rules as admin config is applied after user config.
    Admin,
    /// "Approve for this session" (spec 109 D5): `allow` of one exact
    /// command in one directory, kept in the session log, not in config.
    Session,
}

impl RuleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Profile => "profile",
            Self::User => "user",
            Self::Admin => "admin",
            Self::Session => "session",
        }
    }

    /// Evaluation order (D1): later sources are consulted later, so their
    /// matches win within the limits of [`evaluate_part`].
    const ORDER: [Self; 5] = [
        Self::Repository,
        Self::Profile,
        Self::User,
        Self::Admin,
        Self::Session,
    ];
}

/// One command rule. `pattern` and `directory` are kept as written, so the
/// rule reads back as the user wrote it, and are parsed against each command
/// they are compared with, because a relative path in a pattern means "where
/// the command runs".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRule {
    pub effect: RuleEffect,
    pub pattern: String,
    /// `None`: any directory. For a session rule, the exact directory the
    /// command was approved in.
    pub directory: Option<String>,
    pub source: RuleSource,
    /// The line of its config file, when it came from one, so a refusal or a
    /// card can point at it.
    pub line: Option<usize>,
}

impl CommandRule {
    /// Parses the value of a `command_rule` line: `<effect> <pattern> [in
    /// <directory>]`. Whether the source may hold this effect is the
    /// config merge's decision, not the parser's, so a repository `allow`
    /// parses here and is refused where the scope is known.
    pub fn parse(text: &str, source: RuleSource) -> Result<Self, String> {
        if source == RuleSource::Session {
            return Err("a session rule comes from approving a command, not from text".to_string());
        }
        let text = text.trim();
        let (effect, rest) = text
            .split_once(char::is_whitespace)
            .ok_or_else(|| format!("`{text}` needs an effect and a command: `allow <command>`"))?;
        let effect = match effect {
            "allow" => RuleEffect::Allow,
            "ask" => RuleEffect::Ask,
            "deny" => RuleEffect::Deny,
            other => {
                return Err(format!(
                    "`{other}` is not an effect; a rule starts with `allow`, `ask` or `deny`"
                ));
            }
        };
        let (pattern, directory) = split_directory(rest.trim());
        Self::build(effect, pattern, directory.map(str::to_string), source)
    }

    /// A rule from its parts, checked the way [`Self::parse`] checks a line:
    /// the pattern must read as one command (or one wrapper group). Used
    /// where the rule is built rather than written, as the migration of the
    /// old command keys does (D6), so a pattern that would read back as a
    /// different rule is refused rather than silently changed.
    pub fn new(
        effect: RuleEffect,
        pattern: &str,
        directory: Option<String>,
        source: RuleSource,
    ) -> Result<Self, String> {
        if split_directory(pattern.trim()).1.is_some() {
            return Err(
                "the command ends in `in <directory>`, which a rule reads as its directory"
                    .to_string(),
            );
        }
        Self::build(effect, pattern, directory, source)
    }

    fn build(
        effect: RuleEffect,
        pattern: &str,
        directory: Option<String>,
        source: RuleSource,
    ) -> Result<Self, String> {
        if source == RuleSource::Session {
            return Err("a session rule comes from approving a command, not from text".to_string());
        }
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Err("the rule names no command".to_string());
        }
        // Validated against a stand-in location: whether the pattern parses
        // as one command does not depend on where it runs.
        compile_pattern(
            pattern,
            &ParseContext {
                working_directory: Path::new("/"),
                home: Some(Path::new("/")),
                shell_parameters: ShellParameters::ALL,
            },
        )?;
        Ok(Self {
            effect,
            pattern: pattern.to_string(),
            directory,
            source,
            line: None,
        })
    }

    /// The rule "Approve for this session" writes (spec 109 D5, proposal
    /// requirement 9): `allow` exactly `command` in exactly
    /// `working_directory`. No wildcard is read in it, so a quoted `'*'` in
    /// the approved command stays a literal. Each part of an approved chain
    /// is covered, alone or in another chain, in the directory it ran in.
    /// `shell_parameters` is the user's `command_shell_parameters`, so a
    /// command the rules cannot analyse is never approved for the session.
    pub fn session(
        command: &str,
        working_directory: &Path,
        home: Option<&Path>,
        shell_parameters: ShellParameters,
    ) -> Result<Self, String> {
        let directory = normalize_lexically(working_directory);
        let parsed = parse_command(
            command,
            &ParseContext {
                working_directory: &directory,
                home,
                shell_parameters,
            },
        );
        if let CommandParse::Unanalysable { reason } = parsed {
            return Err(format!(
                "the command cannot be approved for the session: it {reason}"
            ));
        }
        Ok(Self {
            effect: RuleEffect::Allow,
            pattern: command.trim().to_string(),
            directory: Some(directory.to_string_lossy().into_owned()),
            source: RuleSource::Session,
            line: None,
        })
    }

    /// The rule as a `command_rule` value: `allow rm -rf /tmp/** in ~/x`.
    pub fn text(&self) -> String {
        match &self.directory {
            Some(directory) => format!("{} {} in {directory}", self.effect.as_str(), self.pattern),
            None => format!("{} {}", self.effect.as_str(), self.pattern),
        }
    }

    /// Whether this rule covers `group[position]`. A group is a part and
    /// the wrappers around it (`sudo nice rm …` is three parts joined by
    /// [`ChainOperator::Wraps`]); an unwrapped part is a group of one. A
    /// one-part pattern covers any single part, wrapped or not, so
    /// `deny rm -rf *` sees `sudo rm -rf x` and `allow sudo *` covers only
    /// the `sudo` part. A wrapped pattern (`allow sudo make install`)
    /// covers every part of a group that matches it part for part.
    pub fn matches(&self, group: &[CommandPart], position: usize, home: Option<&Path>) -> bool {
        let part = &group[position];
        if self.source == RuleSource::Session {
            return self.covers_exactly(group, home);
        }
        if let Some(directory) = &self.directory
            && !directory_matches(directory, &part.working_directory, home)
        {
            return false;
        }
        let Ok(compiled) = compile_pattern(
            &self.pattern,
            &ParseContext {
                working_directory: &part.working_directory,
                home,
                shell_parameters: ShellParameters::ALL,
            },
        ) else {
            return false;
        };
        match compiled.parts.as_slice() {
            [single] => part_matches(single, compiled.rest, part),
            parts => {
                parts.len() == group.len()
                    && parts
                        .iter()
                        .zip(group)
                        .enumerate()
                        .all(|(index, (pattern, part))| {
                            part_matches(pattern, compiled.rest && index + 1 == parts.len(), part)
                        })
            }
        }
    }

    /// A session rule covers each group of the command it was approved for,
    /// compared whole: wrappers, program, options, operands and the
    /// directory it runs in.
    fn covers_exactly(&self, group: &[CommandPart], home: Option<&Path>) -> bool {
        let Some(directory) = &self.directory else {
            return false;
        };
        match parse_command(
            &self.pattern,
            &ParseContext {
                working_directory: Path::new(directory),
                home,
                // The approved command parsed when the rule was made; a
                // wider set reads an analysable command the same way.
                shell_parameters: ShellParameters::ALL,
            },
        ) {
            CommandParse::Chain(chain) => wrap_groups(&chain)
                .into_iter()
                .any(|range| &chain.parts[range] == group),
            CommandParse::Unanalysable { .. } => false,
        }
    }
}

/// Splits a trailing `in <directory>` off a rule. Only a directory that is
/// absolute or starts at `~` counts, so `allow grep in notes.txt` is a
/// command, not a directory. A stated limit: a directory with spaces cannot
/// be written.
fn split_directory(rest: &str) -> (&str, Option<&str>) {
    let Some((head, directory)) = rest.rsplit_once(char::is_whitespace) else {
        return (rest, None);
    };
    let is_directory =
        directory.starts_with('/') || directory == "~" || directory.starts_with("~/");
    let head = head.trim_end();
    match head.rsplit_once(char::is_whitespace) {
        Some((command, "in")) if is_directory => (command.trim_end(), Some(directory)),
        None if head == "in" && is_directory => ("", Some(directory)),
        _ => (rest, None),
    }
}

// --- Patterns ---------------------------------------------------------------

/// Stands in for a pattern's `*` while the pattern goes through the command
/// parser, which would otherwise refuse it as a glob the shell expands. A
/// private-use character, so no command a person writes contains it.
const WILDCARD: char = '\u{E000}';

struct CompiledPattern {
    /// One part, or a wrapper group.
    parts: Vec<CommandPart>,
    /// The pattern ended in a ` *` word: any further arguments of its last
    /// part.
    rest: bool,
}

fn compile_pattern(pattern: &str, context: &ParseContext) -> Result<CompiledPattern, String> {
    if pattern.contains(WILDCARD) {
        return Err("the pattern contains a reserved character".to_string());
    }
    let (body, rest) = match pattern.strip_suffix('*') {
        Some(body) if body.ends_with(char::is_whitespace) => (body.trim_end(), true),
        _ => (pattern, false),
    };
    let marked = mark_wildcards(body);
    match parse_command(&marked, context) {
        CommandParse::Unanalysable { reason } => {
            Err(format!("the pattern {}", reason.replace(WILDCARD, "*")))
        }
        CommandParse::Chain(chain)
            if chain
                .operators
                .iter()
                .all(|operator| *operator == ChainOperator::Wraps) =>
        {
            Ok(CompiledPattern {
                parts: chain.parts,
                rest,
            })
        }
        CommandParse::Chain(_) => Err(
            "the pattern names more than one command; a chain is decided part by part, so \
             write one rule for each part"
                .to_string(),
        ),
    }
}

/// Every `*` of a pattern is a wildcard, except the one in the shell
/// parameter `$*` (or `${*}`), which is the parameter (spec 111 D11).
fn mark_wildcards(pattern: &str) -> String {
    let mut marked = String::with_capacity(pattern.len());
    let mut previous: [Option<char>; 2] = [None, None];
    for character in pattern.chars() {
        let parameter =
            character == '*' && (previous[1] == Some('$') || previous == [Some('$'), Some('{')]);
        if character == '*' && !parameter {
            marked.push(WILDCARD);
        } else {
            marked.push(character);
        }
        previous = [previous[1], Some(character)];
    }
    marked
}

fn part_matches(pattern: &CommandPart, rest: bool, command: &CommandPart) -> bool {
    if !word_matches(&pattern.program, &command.program) {
        return false;
    }
    match (&pattern.shape, &command.shape) {
        (
            PartShape::Known {
                subcommand: pattern_subcommand,
                global_options: pattern_globals,
                options: pattern_options,
                operands: pattern_operands,
            },
            PartShape::Known {
                subcommand,
                global_options,
                options,
                operands,
            },
        ) => {
            let subcommand_matches = match (pattern_subcommand, subcommand) {
                (None, None) => true,
                (Some(pattern), Some(subcommand)) => word_matches(pattern, subcommand),
                // `allow git *` covers every subcommand.
                (None, Some(_)) => {
                    rest && pattern_operands.is_empty() && pattern_options.is_empty()
                }
                (Some(_), None) => false,
            };
            subcommand_matches
                && options_match(pattern_globals, global_options, rest)
                && options_match(pattern_options, options, rest)
                && arguments_match(pattern_operands, operands, rest)
        }
        (
            PartShape::Unknown {
                arguments: pattern_arguments,
            },
            PartShape::Unknown { arguments },
        ) => arguments_match(pattern_arguments, arguments, rest),
        _ => false,
    }
}

/// Options compare as sets. Without a trailing ` *` the sets are equal;
/// with one, the command may carry options the pattern does not name.
fn options_match(
    pattern: &BTreeSet<CommandOption>,
    command: &BTreeSet<CommandOption>,
    rest: bool,
) -> bool {
    let covered = |option: &CommandOption, by: &BTreeSet<CommandOption>, pattern_side: bool| {
        by.iter().any(|other| {
            if pattern_side {
                option_matches(option, other)
            } else {
                option_matches(other, option)
            }
        })
    };
    pattern.iter().all(|option| covered(option, command, true))
        && (rest || command.iter().all(|option| covered(option, pattern, false)))
}

fn option_matches(pattern: &CommandOption, command: &CommandOption) -> bool {
    word_matches(&pattern.name, &command.name)
        && match (&pattern.value, &command.value) {
            (None, None) => true,
            (Some(pattern), Some(value)) => argument_matches(pattern, value),
            _ => false,
        }
}

fn arguments_match(pattern: &[Argument], command: &[Argument], rest: bool) -> bool {
    let lengths_fit = if rest {
        pattern.len() <= command.len()
    } else {
        pattern.len() == command.len()
    };
    lengths_fit
        && pattern
            .iter()
            .zip(command)
            .all(|(pattern, argument)| argument_matches(pattern, argument))
}

/// A path pattern matches only a path, and a word pattern only a word: an
/// unknown program's `x` and `./x` are different arguments (D3), and erring
/// here only ever makes a rule cover less.
fn argument_matches(pattern: &Argument, argument: &Argument) -> bool {
    match (pattern, argument) {
        (Argument::Path(pattern), Argument::Path(path)) => path_matches(pattern, path),
        (Argument::Word(pattern), Argument::Word(word)) => word_matches(pattern, word),
        _ => false,
    }
}

fn directory_matches(pattern: &str, directory: &Path, home: Option<&Path>) -> bool {
    let expanded = if pattern == "~" || pattern.starts_with("~/") {
        let Some(home) = home else {
            return false;
        };
        home.join(pattern[1..].trim_start_matches('/'))
    } else {
        PathBuf::from(pattern)
    };
    let marked = PathBuf::from(
        expanded
            .to_string_lossy()
            .replace('*', &WILDCARD.to_string()),
    );
    path_matches(
        &normalize_lexically(&marked),
        &normalize_lexically(directory),
    )
}

fn segments(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(segment) => Some(segment.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// Both paths are absolute and lexically normalised, so `/tmp/..` has
/// already become `/` and cannot slip under a `/tmp/**` pattern.
fn path_matches(pattern: &Path, path: &Path) -> bool {
    segments_match(&segments(pattern), &segments(path))
}

fn is_any_depth(segment: &str) -> bool {
    segment.chars().count() >= 2 && segment.chars().all(|character| character == WILDCARD)
}

fn segments_match(pattern: &[String], path: &[String]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        // A trailing `**` is everything below, not the directory itself, the
        // way `.gitignore` reads `dir/**`.
        Some((first, [])) if is_any_depth(first) => !path.is_empty(),
        Some((first, rest)) if is_any_depth(first) => {
            (0..=path.len()).any(|skip| segments_match(rest, &path[skip..]))
        }
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(head, tail)| word_matches(first, head) && segments_match(rest, tail)),
    }
}

/// `pattern` with [`WILDCARD`] matching any run of characters, including
/// none.
fn word_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == WILDCARD {
            backtrack = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star, matched)) = backtrack {
            p = star + 1;
            t = matched + 1;
            backtrack = Some((star, matched + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&character| character == WILDCARD)
}

// --- Damaian's built-in defaults (D5) ---------------------------------------

/// The destructive commands Damaian denies when no rule of the user's says
/// otherwise, as checks over parsed commands rather than text. Settings
/// shows them as "Damaian's defaults"; they are not editable, and a user
/// rule overrides them (D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltInDefault {
    /// Recursive `rm` of `/`, the home directory, the repository root, a
    /// parent of it, or anything outside the repository (`/tmp` included).
    RecursiveRemove,
    GitResetHard,
    /// `git clean` with both `-f` and `-d`.
    GitCleanForce,
    /// `dd` writing to a device under `/dev`.
    DdToDevice,
    MakeFilesystem,
    Shutdown,
    Reboot,
}

impl BuiltInDefault {
    pub const ALL: [Self; 7] = [
        Self::RecursiveRemove,
        Self::GitResetHard,
        Self::GitCleanForce,
        Self::DdToDevice,
        Self::MakeFilesystem,
        Self::Shutdown,
        Self::Reboot,
    ];

    /// The name an audit record carries (D7): never the command itself.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecursiveRemove => "recursive_remove",
            Self::GitResetHard => "git_reset_hard",
            Self::GitCleanForce => "git_clean_force",
            Self::DdToDevice => "dd_to_device",
            Self::MakeFilesystem => "make_filesystem",
            Self::Shutdown => "shutdown",
            Self::Reboot => "reboot",
        }
    }

    /// What Settings lists under "Damaian's defaults".
    pub fn description(self) -> &'static str {
        match self {
            Self::RecursiveRemove => {
                "recursive `rm` of `/`, your home directory, the repository root, a directory \
                 above it, or anything outside the repository"
            }
            Self::GitResetHard => "`git reset --hard`",
            Self::GitCleanForce => "`git clean` with `-f` and `-d`",
            Self::DdToDevice => "`dd` writing to a device (`of=/dev/…`)",
            Self::MakeFilesystem => "`mkfs` and its variants",
            Self::Shutdown => "`shutdown`",
            Self::Reboot => "`reboot`",
        }
    }
}

/// A built-in default that denies one part, with what it saw: "recursive
/// `rm` of `/`".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltInDenial {
    pub default: BuiltInDefault,
    pub detail: String,
}

/// Devices `dd` may write to without the default denying it: they discard
/// or print what they are given.
const HARMLESS_DEVICES: [&str; 4] = ["/dev/null", "/dev/stdout", "/dev/stderr", "/dev/tty"];

fn built_in_denial(part: &CommandPart, repository_root: &Path) -> Option<BuiltInDenial> {
    let denial = |default, detail: String| Some(BuiltInDenial { default, detail });
    let name = Path::new(&part.program)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match (&part.shape, part.program.as_str()) {
        (
            PartShape::Known {
                options, operands, ..
            },
            "rm",
        ) if options.iter().any(|option| option.name == "--recursive") => {
            let root = normalize_lexically(repository_root);
            operands.iter().find_map(|operand| match operand {
                // At or above the root, or outside it: not strictly inside.
                Argument::Path(path) if !path.starts_with(&root) || path == &root => denial(
                    BuiltInDefault::RecursiveRemove,
                    format!("recursive `rm` of `{}`", displayed(&path.to_string_lossy())),
                ),
                _ => None,
            })
        }
        (
            PartShape::Known {
                subcommand: Some(subcommand),
                options,
                ..
            },
            "git",
        ) => {
            let has = |name: &str| options.iter().any(|option| option.name == name);
            match subcommand.as_str() {
                "reset" if has("--hard") => denial(
                    BuiltInDefault::GitResetHard,
                    "`git reset --hard`".to_string(),
                ),
                "clean" if has("--force") && has("-d") => {
                    denial(BuiltInDefault::GitCleanForce, "`git clean -fd`".to_string())
                }
                _ => None,
            }
        }
        (PartShape::Known { options, .. }, "dd") => {
            options
                .iter()
                .find_map(|option| match (&option.name[..], &option.value) {
                    ("of", Some(Argument::Path(path)))
                        if path.starts_with("/dev")
                            && !HARMLESS_DEVICES
                                .iter()
                                .any(|device| path == Path::new(device)) =>
                    {
                        denial(
                            BuiltInDefault::DdToDevice,
                            format!("`dd` writing to `{}`", displayed(&path.to_string_lossy())),
                        )
                    }
                    _ => None,
                })
        }
        _ if name.starts_with("mkfs") => {
            denial(BuiltInDefault::MakeFilesystem, format!("`{name}`"))
        }
        _ if name == "shutdown" => denial(BuiltInDefault::Shutdown, "`shutdown`".to_string()),
        _ if name == "reboot" => denial(BuiltInDefault::Reboot, "`reboot`".to_string()),
        _ => None,
    }
}

// --- Evaluation ---------------------------------------------------------------

/// Where a command runs, for the parser and the built-in defaults.
#[derive(Debug, Clone, Copy)]
pub struct RuleContext<'a> {
    pub repository_root: &'a Path,
    pub working_directory: &'a Path,
    pub home: Option<&'a Path>,
    /// The user's `command_shell_parameters` (spec 111 D11).
    pub shell_parameters: ShellParameters,
}

/// What the rules decide about a part or a chain, in order of strictness so
/// a chain takes the strictest of its parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleVerdict {
    Allow,
    /// No rule matched and no built-in default denies it: Damaian's other
    /// built-in defaults decide (read-only admission, spec 64; the approval
    /// causes, spec 62).
    Default,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decider {
    BuiltIn(BuiltInDenial),
    Rule(CommandRule),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartDecision {
    pub part: CommandPart,
    pub verdict: RuleVerdict,
    /// The rule that decided, or the built-in default; `None` when the
    /// verdict is [`RuleVerdict::Default`].
    pub decided_by: Option<Decider>,
    /// The built-in default that would deny this part, whether or not a rule
    /// of the user's overrode it, so the card can warn (D7).
    pub built_in: Option<BuiltInDenial>,
}

impl PartDecision {
    /// A rule of the user's lifted a built-in `deny` (D7: warn and audit as
    /// `command_rule_override`).
    pub fn overrides_built_in(&self) -> bool {
        self.built_in.is_some() && self.verdict != RuleVerdict::Deny
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleEvaluation {
    /// The command cannot be analysed (proposal requirement 4): it asks, and
    /// no rule covers it.
    Unanalysable {
        reason: String,
    },
    Parts(Vec<PartDecision>),
}

impl RuleEvaluation {
    /// Any `deny` denies, else any `ask` asks, else any part left to the
    /// defaults leaves the chain to them; only all-`allow` allows.
    pub fn verdict(&self) -> RuleVerdict {
        match self {
            Self::Unanalysable { .. } => RuleVerdict::Ask,
            Self::Parts(parts) => parts
                .iter()
                .map(|part| part.verdict)
                .max()
                .unwrap_or(RuleVerdict::Default),
        }
    }
}

/// Evaluates `command` against `rules` (from every source, in each source's
/// own order) and the built-in defaults.
pub fn evaluate_command(
    command: &str,
    rules: &[CommandRule],
    context: &RuleContext,
) -> RuleEvaluation {
    let parsed = parse_command(
        command,
        &ParseContext {
            working_directory: context.working_directory,
            home: context.home,
            shell_parameters: context.shell_parameters,
        },
    );
    match parsed {
        CommandParse::Unanalysable { reason } => RuleEvaluation::Unanalysable { reason },
        CommandParse::Chain(chain) => RuleEvaluation::Parts(
            wrap_groups(&chain)
                .into_iter()
                .flat_map(|range| {
                    let group = &chain.parts[range];
                    (0..group.len()).map(move |position| (group, position))
                })
                .map(|(group, position)| evaluate_part(group, position, rules, context))
                .collect(),
        ),
    }
}

/// The ranges of `chain.parts` that a wrapper ties together.
fn wrap_groups(chain: &Chain) -> Vec<std::ops::Range<usize>> {
    let mut groups = Vec::new();
    let mut start = 0;
    for (index, operator) in chain.operators.iter().enumerate() {
        if *operator != ChainOperator::Wraps {
            groups.push(start..index + 1);
            start = index + 1;
        }
    }
    if !chain.parts.is_empty() {
        groups.push(start..chain.parts.len());
    }
    groups
}

/// One part, under D2's limits: a repository or profile rule only tightens
/// (an `allow` from either is refused at load, and ignored here should one
/// arrive); a user or admin rule is final either way; a session rule only
/// allows, and never lifts a `deny` — a built-in default's or any rule's.
fn evaluate_part(
    group: &[CommandPart],
    position: usize,
    rules: &[CommandRule],
    context: &RuleContext,
) -> PartDecision {
    let part = group[position].clone();
    let built_in = built_in_denial(&part, context.repository_root);
    let mut verdict = if built_in.is_some() {
        RuleVerdict::Deny
    } else {
        RuleVerdict::Default
    };
    let mut decided_by = built_in.clone().map(Decider::BuiltIn);
    for source in RuleSource::ORDER {
        for rule in rules.iter().filter(|rule| rule.source == source) {
            if !rule.matches(group, position, context.home) {
                continue;
            }
            let effect = rule.effect.verdict();
            let takes = match rule.source {
                RuleSource::Repository | RuleSource::Profile => {
                    rule.effect != RuleEffect::Allow && effect >= verdict
                }
                RuleSource::User | RuleSource::Admin => true,
                RuleSource::Session => {
                    rule.effect == RuleEffect::Allow && verdict != RuleVerdict::Deny
                }
            };
            if takes {
                verdict = effect;
                decided_by = Some(Decider::Rule(rule.clone()));
            }
        }
    }
    PartDecision {
        part,
        verdict,
        decided_by,
        built_in,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BuiltInDefault, CommandRule, Decider, RuleContext, RuleEffect, RuleEvaluation, RuleSource,
        RuleVerdict, evaluate_command, word_matches,
    };
    use crate::command_parse::{ShellParameter, ShellParameters};
    use std::path::Path;

    const ROOT: &str = "/Users/me/project";
    const HOME: &str = "/Users/me";

    fn context() -> RuleContext<'static> {
        RuleContext {
            repository_root: Path::new(ROOT),
            working_directory: Path::new(ROOT),
            home: Some(Path::new(HOME)),
            shell_parameters: ShellParameters::default(),
        }
    }

    fn rule(source: RuleSource, text: &str) -> CommandRule {
        CommandRule::parse(text, source).unwrap_or_else(|error| panic!("`{text}`: {error}"))
    }

    fn user(text: &str) -> CommandRule {
        rule(RuleSource::User, text)
    }

    fn evaluate(command: &str, rules: &[CommandRule]) -> RuleEvaluation {
        evaluate_command(command, rules, &context())
    }

    fn verdict(command: &str, rules: &[CommandRule]) -> RuleVerdict {
        evaluate(command, rules).verdict()
    }

    fn built_in(command: &str) -> Option<BuiltInDefault> {
        match evaluate(command, &[]) {
            RuleEvaluation::Parts(parts) => parts
                .iter()
                .find_map(|part| part.built_in.as_ref().map(|denial| denial.default)),
            RuleEvaluation::Unanalysable { reason } => panic!("`{command}`: {reason}"),
        }
    }

    #[test]
    fn a_rule_line_parses_into_effect_pattern_and_directory() {
        let parsed = user("allow  rm -rf /tmp/**  in ~/work");
        assert_eq!(parsed.effect, RuleEffect::Allow);
        assert_eq!(parsed.pattern, "rm -rf /tmp/**");
        assert_eq!(parsed.directory.as_deref(), Some("~/work"));
        assert_eq!(parsed.text(), "allow rm -rf /tmp/** in ~/work");
        assert_eq!(
            user("ask grep in notes.txt").pattern,
            "grep in notes.txt",
            "`in` before something that is not a directory is part of the command"
        );
        for (text, expected) in [
            ("permit rm -rf /", "not an effect"),
            ("allow", "needs an effect and a command"),
            ("allow in /tmp", "names no command"),
            ("allow cargo fmt && cargo clippy", "more than one command"),
            ("deny echo $(rm -rf /)", "shell expansion"),
            ("deny rm -rf $HOME", "shell expansion"),
        ] {
            let error = CommandRule::parse(text, RuleSource::User).unwrap_err();
            assert!(error.contains(expected), "`{text}`: {error}");
        }
        assert!(CommandRule::parse("allow ls", RuleSource::Session).is_err());
    }

    /// Proposal §4, first criterion.
    #[test]
    fn rm_rf_tmp_and_rm_rf_root_are_denied_by_default_and_each_needs_its_own_allow() {
        for command in [
            "rm -rf /",
            "rm -rf /tmp",
            "rm -fr /",
            "rm -r -f /",
            "rm --recursive --force /",
            "rm -rf /tmp/..",
            "rm -rf ..",
            "rm -rf ~",
            "rm -rf .",
            "rm -rf /tmp/x",
            "/bin/rm -rf /",
        ] {
            assert_eq!(verdict(command, &[]), RuleVerdict::Deny, "`{command}`");
            assert_eq!(built_in(command), Some(BuiltInDefault::RecursiveRemove));
        }
        assert_eq!(
            verdict("rm -rf target", &[]),
            RuleVerdict::Default,
            "inside the repository the other defaults decide"
        );

        let allow_tmp = [user("allow rm -rf /tmp")];
        assert_eq!(verdict("rm -rf /tmp", &allow_tmp), RuleVerdict::Allow);
        assert_eq!(verdict("rm -fr /tmp/", &allow_tmp), RuleVerdict::Allow);
        for command in [
            "rm -rf /",
            "rm -rf /tmp/x",
            "rm -rf /tmpfiles",
            "rm -rf /tmp/..",
        ] {
            assert_eq!(
                verdict(command, &allow_tmp),
                RuleVerdict::Deny,
                "`allow rm -rf /tmp` does not cover `{command}`"
            );
        }

        let allow_below_tmp = [user("allow rm -rf /tmp/**")];
        assert_eq!(
            verdict("rm -rf /tmp/x/y", &allow_below_tmp),
            RuleVerdict::Allow
        );
        for command in [
            "rm -rf /tmp/..",
            "rm -rf /tmp",
            "rm -rf /",
            "rm -rf /tmpfiles/x",
        ] {
            assert_eq!(
                verdict(command, &allow_below_tmp),
                RuleVerdict::Deny,
                "`allow rm -rf /tmp/**` does not cover `{command}`"
            );
        }

        let allow_root = [user("allow rm -rf /")];
        for command in [
            "rm -rf /",
            "rm -fr /",
            "rm -r -f /",
            "rm --recursive --force /",
            "rm -rf /tmp/..",
        ] {
            assert_eq!(
                verdict(command, &allow_root),
                RuleVerdict::Allow,
                "`{command}`"
            );
        }
        assert_eq!(
            verdict("rm -rf /tmp", &allow_root),
            RuleVerdict::Deny,
            "`allow rm -rf /` does not cover `/tmp`"
        );
    }

    #[test]
    fn a_user_allow_overrides_a_built_in_deny_and_says_so() {
        let RuleEvaluation::Parts(parts) = evaluate(
            "git reset --hard HEAD~1",
            &[user("allow git reset --hard *")],
        ) else {
            panic!("should parse");
        };
        let part = &parts[0];
        assert_eq!(part.verdict, RuleVerdict::Allow);
        assert!(part.overrides_built_in());
        assert_eq!(
            part.built_in.as_ref().map(|denial| denial.default),
            Some(BuiltInDefault::GitResetHard)
        );
        assert!(matches!(
            &part.decided_by,
            Some(Decider::Rule(rule)) if rule.source == RuleSource::User
        ));

        let admin = rule(RuleSource::Admin, "ask shutdown *");
        assert_eq!(verdict("shutdown -h now", &[admin]), RuleVerdict::Ask);
    }

    #[test]
    fn a_user_deny_overrides_every_built_in_permission() {
        assert_eq!(verdict("ls", &[]), RuleVerdict::Default);
        assert_eq!(verdict("ls -la", &[user("deny ls *")]), RuleVerdict::Deny);
        // Last match wins within the user's own rules.
        let rules = [user("deny cargo *"), user("allow cargo test *")];
        assert_eq!(verdict("cargo test --lib", &rules), RuleVerdict::Allow);
        assert_eq!(verdict("cargo publish", &rules), RuleVerdict::Deny);
    }

    #[test]
    fn a_repository_or_profile_rule_never_widens() {
        for source in [RuleSource::Repository, RuleSource::Profile] {
            // Refused at load (config tests); ignored here should one arrive.
            let widening = rule(source, "allow rm -rf /");
            assert_eq!(verdict("rm -rf /", &[widening]), RuleVerdict::Deny);
            let widening = rule(source, "allow cargo publish");
            assert_eq!(verdict("cargo publish", &[widening]), RuleVerdict::Default);
            // An `ask` cannot soften a built-in `deny`, or a user `deny` …
            assert_eq!(
                verdict("git clean -fd", &[rule(source, "ask git clean *")]),
                RuleVerdict::Deny
            );
            assert_eq!(
                verdict(
                    "cargo publish",
                    &[user("deny cargo publish"), rule(source, "ask cargo *")]
                ),
                RuleVerdict::Deny
            );
            // … but it can tighten.
            assert_eq!(verdict("ls", &[rule(source, "ask ls")]), RuleVerdict::Ask);
            assert_eq!(verdict("ls", &[rule(source, "deny ls")]), RuleVerdict::Deny);
            assert_eq!(
                verdict("ls", &[rule(source, "deny ls"), rule(source, "ask ls")]),
                RuleVerdict::Deny,
                "a later `ask` of the same source does not soften its `deny`"
            );
        }
        // D1: user rules are consulted after the repository's.
        let repository = rule(RuleSource::Repository, "deny cargo *");
        assert_eq!(
            verdict(
                "cargo test",
                &[repository.clone(), user("allow cargo test")]
            ),
            RuleVerdict::Allow
        );
        assert_eq!(verdict("cargo test", &[repository]), RuleVerdict::Deny);
    }

    #[test]
    fn a_session_rule_allows_its_exact_command_and_never_lifts_a_deny() {
        let session = |command: &str| {
            CommandRule::session(
                command,
                Path::new(ROOT),
                Some(Path::new(HOME)),
                ShellParameters::default(),
            )
            .unwrap()
        };
        let cargo = [session("cargo clippy --all-targets")];
        assert_eq!(
            verdict("cargo clippy --all-targets", &cargo),
            RuleVerdict::Allow
        );
        assert_eq!(
            verdict("cargo clippy", &cargo),
            RuleVerdict::Default,
            "exact only"
        );
        assert_eq!(
            evaluate_command(
                "cargo clippy --all-targets",
                &cargo,
                &RuleContext {
                    working_directory: Path::new("/Users/me/project/sub"),
                    ..context()
                }
            )
            .verdict(),
            RuleVerdict::Default,
            "only in the directory it was approved in"
        );
        // A quoted star in the approved command is a literal, not a wildcard.
        let grep = [session("grep -n '*' notes.txt")];
        assert_eq!(verdict("grep -n '*' notes.txt", &grep), RuleVerdict::Allow);
        assert_eq!(verdict("grep -n x notes.txt", &grep), RuleVerdict::Default);

        // Never over a user `deny` …
        let rules = [
            user("deny cargo clippy *"),
            session("cargo clippy --all-targets"),
        ];
        assert_eq!(
            verdict("cargo clippy --all-targets", &rules),
            RuleVerdict::Deny
        );
        // … nor a built-in one.
        assert_eq!(
            verdict("rm -rf /tmp", &[session("rm -rf /tmp")]),
            RuleVerdict::Deny
        );
        // An approved chain covers each of its parts.
        let chain = [session("cargo fmt --check && cargo clippy")];
        assert_eq!(
            verdict("cargo fmt --check && cargo clippy", &chain),
            RuleVerdict::Allow
        );
        assert_eq!(verdict("cargo clippy", &chain), RuleVerdict::Allow);
        assert!(
            CommandRule::session(
                "echo $(rm -rf /)",
                Path::new(ROOT),
                None,
                ShellParameters::default()
            )
            .is_err()
        );
    }

    #[test]
    fn any_deny_in_a_chain_denies_and_every_part_must_be_allowed() {
        let rules = [
            user("allow cargo fmt --check"),
            user("allow cargo clippy *"),
        ];
        assert_eq!(
            verdict("cargo fmt --check && cargo clippy --all-targets", &rules),
            RuleVerdict::Allow
        );
        assert_eq!(
            verdict("cargo fmt --check && cargo test", &rules),
            RuleVerdict::Default,
            "a part no rule allows leaves the chain to the defaults"
        );
        assert_eq!(
            verdict("cargo fmt --check && cargo clippy || rm -rf /", &rules),
            RuleVerdict::Deny
        );
        let mut asking = rules.to_vec();
        asking.push(user("ask cargo test"));
        assert_eq!(
            verdict("cargo fmt --check; cargo test", &asking),
            RuleVerdict::Ask
        );
        // Unanalysable asks, and no `allow` covers it.
        let everything = [user("allow echo *"), user("allow rm *")];
        assert_eq!(verdict("echo $(rm -rf /)", &everything), RuleVerdict::Ask);
        assert!(matches!(
            evaluate("echo $(rm -rf /)", &everything),
            RuleEvaluation::Unanalysable { .. }
        ));
    }

    #[test]
    fn patterns_match_parsed_commands() {
        // Options in any spelling and order.
        let rules = [user("deny git push --force *")];
        for command in ["git push -f origin main", "git push origin main --force"] {
            assert_eq!(verdict(command, &rules), RuleVerdict::Deny, "`{command}`");
        }
        assert_eq!(
            verdict("git push origin main", &rules),
            RuleVerdict::Default
        );
        // Without a trailing ` *`, nothing may be added.
        let exact = [user("allow cargo test")];
        assert_eq!(verdict("cargo test", &exact), RuleVerdict::Allow);
        assert_eq!(verdict("cargo test --lib", &exact), RuleVerdict::Default);
        assert_eq!(
            verdict("cargo test", &[user("allow cargo test *")]),
            RuleVerdict::Allow,
            "a trailing ` *` also matches nothing further"
        );
        // `*` within a segment, `**` across segments.
        let one = [user("allow rm -r /tmp/*/cache")];
        assert_eq!(verdict("rm -r /tmp/a/cache", &one), RuleVerdict::Allow);
        assert_eq!(verdict("rm -r /tmp/a/b/cache", &one), RuleVerdict::Deny);
        let deep = [user("allow rm -r /tmp/**/cache")];
        assert_eq!(verdict("rm -r /tmp/a/b/cache", &deep), RuleVerdict::Allow);
        assert_eq!(verdict("rm -r /tmp/cache", &deep), RuleVerdict::Allow);
        // A relative path in a pattern resolves where the command runs.
        let target = [user("deny rm -r target")];
        assert_eq!(verdict("rm -r ./target", &target), RuleVerdict::Deny);
        assert_eq!(
            verdict("rm -r target/../target", &target),
            RuleVerdict::Deny
        );
        // A directory limits where a rule applies.
        let here = [user("allow make deploy in ~/project")];
        assert_eq!(verdict("make deploy", &here), RuleVerdict::Allow);
        assert_eq!(
            evaluate_command(
                "make deploy",
                &here,
                &RuleContext {
                    working_directory: Path::new("/Users/me/other"),
                    ..context()
                }
            )
            .verdict(),
            RuleVerdict::Default
        );
        let below = [user("allow make in /Users/me/**")];
        assert_eq!(verdict("make", &below), RuleVerdict::Allow);
        // `allow git *` covers every subcommand.
        assert_eq!(
            verdict("git stash drop", &[user("allow git *")]),
            RuleVerdict::Allow
        );
        assert_eq!(
            verdict("git reset --hard", &[user("allow git *")]),
            RuleVerdict::Allow,
            "the user's own wildcard is the user's decision"
        );
    }

    #[test]
    fn the_built_in_defaults_read_parsed_commands() {
        for (command, expected) in [
            ("git reset --hard", Some(BuiltInDefault::GitResetHard)),
            (
                "git reset -q --hard origin/main",
                Some(BuiltInDefault::GitResetHard),
            ),
            (
                "git -C ../other reset --hard",
                Some(BuiltInDefault::GitResetHard),
            ),
            ("git reset --soft HEAD~1", None),
            ("git clean -fd", Some(BuiltInDefault::GitCleanForce)),
            ("git clean -d --force", Some(BuiltInDefault::GitCleanForce)),
            ("git clean -fdx", Some(BuiltInDefault::GitCleanForce)),
            ("git clean -f", None),
            ("git clean -d --dry-run", None),
            (
                "dd if=/dev/zero of=/dev/disk2 bs=1m",
                Some(BuiltInDefault::DdToDevice),
            ),
            (
                "dd if=image.iso of=/dev/rdisk4",
                Some(BuiltInDefault::DdToDevice),
            ),
            ("dd if=/dev/zero of=/dev/null count=1", None),
            ("dd if=in.bin of=out.bin", None),
            ("mkfs.ext4 /dev/sda1", Some(BuiltInDefault::MakeFilesystem)),
            ("/sbin/mkfs /dev/sda1", Some(BuiltInDefault::MakeFilesystem)),
            ("shutdown", Some(BuiltInDefault::Shutdown)),
            ("shutdown -h now", Some(BuiltInDefault::Shutdown)),
            ("/sbin/reboot", Some(BuiltInDefault::Reboot)),
            ("rm -f /tmp/x", None),
            ("rm -r build", None),
            ("echo git reset --hard", None),
        ] {
            assert_eq!(built_in(command), expected, "`{command}`");
        }
    }

    #[test]
    fn word_wildcards_match_any_run_of_characters() {
        let w = |pattern: &str| pattern.replace('*', "\u{E000}");
        assert!(word_matches(&w("*.rs"), "main.rs"));
        assert!(word_matches(&w("a*b*c"), "aXXbYc"));
        assert!(word_matches(&w("*"), ""));
        assert!(!word_matches(&w("*.rs"), "main.rsx"));
        assert!(!word_matches("abc", "abx"));
    }

    /// Context D9 (the owner, 2026-11-20): wrappers are unwrapped, so rules
    /// and defaults see the command behind them, and the wrapper and the
    /// command must both be allowed.
    #[test]
    fn rules_see_through_wrappers_and_both_parts_must_be_allowed() {
        for command in [
            "sudo rm -rf /",
            "sudo -n nice -n 5 rm -fr /tmp",
            "time -p rm -rf ~",
        ] {
            assert_eq!(verdict(command, &[]), RuleVerdict::Deny, "`{command}`");
            assert_eq!(built_in(command), Some(BuiltInDefault::RecursiveRemove));
        }
        let sudo = [user("allow sudo *")];
        assert_eq!(
            verdict("sudo rm -rf /", &sudo),
            RuleVerdict::Deny,
            "`allow sudo *` does not cover the wrapped command"
        );
        assert_eq!(
            verdict("sudo make install", &sudo),
            RuleVerdict::Default,
            "the wrapped command is left to its own rules and defaults"
        );
        let both = [user("allow sudo *"), user("allow make install")];
        assert_eq!(verdict("sudo make install", &both), RuleVerdict::Allow);
        assert_eq!(
            verdict("sudo make install", &[user("allow sudo make install")]),
            RuleVerdict::Allow,
            "a wrapped pattern covers its whole group"
        );
        assert_eq!(
            verdict("sudo make clean", &[user("allow sudo make install")]),
            RuleVerdict::Default
        );
        assert_eq!(
            verdict("make install", &[user("allow sudo make install")]),
            RuleVerdict::Default,
            "a wrapped pattern does not cover the command without its wrapper"
        );
        // A user `deny` of the inner command sees through the wrapper.
        assert_eq!(
            verdict(
                "sudo -E make install",
                &[user("allow sudo *"), user("deny make *")]
            ),
            RuleVerdict::Deny
        );
        // The user may still allow the wrapped destructive command, both parts.
        assert_eq!(
            verdict("sudo rm -rf /tmp", &[user("allow sudo rm -rf /tmp")]),
            RuleVerdict::Allow
        );
        // `env`'s assignments belong to the `env` part.
        let env = [user("allow env RUST_LOG=debug"), user("allow cargo run")];
        assert_eq!(
            verdict("env RUST_LOG=debug cargo run", &env),
            RuleVerdict::Allow
        );
        assert_eq!(
            verdict("env RUST_LOG=trace cargo run", &env),
            RuleVerdict::Default
        );
        // `xargs` stays unanalysable: it asks, no rule covers it, and a rule
        // naming it is a config error.
        assert!(matches!(
            evaluate("xargs rm -rf", &[user("allow rm *")]),
            RuleEvaluation::Unanalysable { .. }
        ));
        assert!(CommandRule::parse("allow xargs *", RuleSource::User).is_err());
        // A session approval covers the wrapped group exactly.
        let session = [CommandRule::session(
            "sudo make install",
            Path::new(ROOT),
            Some(Path::new(HOME)),
            ShellParameters::default(),
        )
        .unwrap()];
        assert_eq!(verdict("sudo make install", &session), RuleVerdict::Allow);
        assert_eq!(verdict("make install", &session), RuleVerdict::Default);
        assert_eq!(verdict("sudo make clean", &session), RuleVerdict::Default);
    }

    fn session_with(
        command: &str,
        shell_parameters: ShellParameters,
    ) -> Result<CommandRule, String> {
        CommandRule::session(
            command,
            Path::new(ROOT),
            Some(Path::new(HOME)),
            shell_parameters,
        )
    }

    /// Spec 111 D11: a parameter is matched as written.
    #[test]
    fn a_shell_parameter_is_matched_as_written() {
        // `/tmp/$$` is outside the repository, so the built-in default
        // denies it; a user `allow` naming `/tmp/*` overrides that, one
        // naming `/tmp/build` does not cover it.
        assert_eq!(verdict("rm -rf /tmp/$$", &[]), RuleVerdict::Deny);
        let star = [user("allow rm -rf /tmp/*")];
        assert_eq!(verdict("rm -rf /tmp/$$", &star), RuleVerdict::Allow);
        assert_eq!(verdict("rm -rf /tmp/run-$$", &star), RuleVerdict::Allow);
        let build = [user("allow rm -rf /tmp/build")];
        assert_eq!(verdict("rm -rf /tmp/$$", &build), RuleVerdict::Deny);
        // A rule may name the parameter itself, `$*` included.
        let exact = [user("allow rm -rf /tmp/$$")];
        assert_eq!(verdict("rm -rf /tmp/$$", &exact), RuleVerdict::Allow);
        assert_eq!(verdict("rm -rf /tmp/$#", &exact), RuleVerdict::Deny);
        let echo = [user("deny echo $*")];
        assert_eq!(verdict("echo $*", &echo), RuleVerdict::Deny);
        assert_eq!(verdict("echo \"$@\"", &echo), RuleVerdict::Default);
        assert_eq!(verdict("echo x", &echo), RuleVerdict::Default);
        let any = [user("allow echo *")];
        assert_eq!(verdict("echo \"exit $?\"", &any), RuleVerdict::Allow);
        // Empty-capable ones inside a path stay unanalysable: no rule.
        assert!(matches!(
            evaluate("rm -rf /tmp/$1", &star),
            RuleEvaluation::Unanalysable { .. }
        ));
        assert!(matches!(
            evaluate("rm -rf /tmp/$!", &star),
            RuleEvaluation::Unanalysable { .. }
        ));
        // The card's detail shows the parameter without its mark.
        let RuleEvaluation::Parts(parts) = evaluate("rm -rf /tmp/$$", &[]) else {
            panic!("`rm -rf /tmp/$$` should parse");
        };
        let detail = &parts[0].built_in.as_ref().expect("built-in denial").detail;
        assert_eq!(detail, "recursive `rm` of `/tmp/$$`");
    }

    #[test]
    fn a_quoted_dollar_never_matches_the_same_rule_as_the_parameter() {
        let literal = [user("allow rm -rf /tmp/'$$'")];
        assert_eq!(verdict("rm -rf /tmp/'$$'", &literal), RuleVerdict::Allow);
        assert_eq!(verdict("rm -rf /tmp/$$", &literal), RuleVerdict::Deny);
        let real = [user("allow rm -rf /tmp/$$")];
        assert_eq!(verdict("rm -rf /tmp/$$", &real), RuleVerdict::Allow);
        assert_eq!(verdict("rm -rf /tmp/'$$'", &real), RuleVerdict::Deny);
        // Nor the same session approval.
        let approved = [session_with("echo '$1'", ShellParameters::default()).unwrap()];
        assert_eq!(verdict("echo '$1'", &approved), RuleVerdict::Allow);
        assert_eq!(verdict("echo $1", &approved), RuleVerdict::Default);
        let approved = [session_with("echo $1", ShellParameters::default()).unwrap()];
        assert_eq!(verdict("echo $1", &approved), RuleVerdict::Allow);
        assert_eq!(verdict("echo '$1'", &approved), RuleVerdict::Default);
    }

    #[test]
    fn opt_in_parameters_are_analysed_only_when_the_user_turned_them_on() {
        let rules = [user("allow echo $0")];
        assert!(matches!(
            evaluate("echo $0", &rules),
            RuleEvaluation::Unanalysable { .. }
        ));
        let mut enabled = context();
        enabled.shell_parameters = ShellParameters::from_list(&[ShellParameter::Zero]);
        assert_eq!(
            evaluate_command("echo $0", &rules, &enabled).verdict(),
            RuleVerdict::Allow
        );
        assert!(matches!(
            evaluate_command("echo $_", &rules, &enabled),
            RuleEvaluation::Unanalysable { .. }
        ));
        // A session approval needs the setting the command is judged under.
        assert!(session_with("echo $IFS", ShellParameters::default()).is_err());
        assert!(session_with("echo $IFS", ShellParameters::ALL).is_ok());
    }
}
