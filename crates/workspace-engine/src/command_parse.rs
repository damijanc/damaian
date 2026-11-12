//! Commands as the command rules see them (spec 111 D3, D4): a chain split
//! into parts, each part's options normalised for the programs Damaian knows,
//! and its path arguments resolved against the directory it runs in.
//!
//! The words come from the same no-expansion tokenizer as the read-only
//! boundary (spec 64, `command_policy::chain_line`). Anything the shell would
//! rewrite after the check — `$VAR`, `$(…)`, backticks, an unquoted glob, a
//! here-doc — makes the whole command [`CommandParse::Unanalysable`], which
//! the rules treat as `ask` and no `allow` covers (proposal requirement 4).
//!
//! Stated limits: symlinks are not followed (D4); GNU's abbreviated long
//! options (`--rec`) are compared as written; an option the table does not
//! know is kept as written, and in a group of short flags everything from the
//! first unknown letter on is kept as one word, because it may be that
//! option's value.

use crate::command_policy::{Operator, chain_line, normalize_lexically};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where a command runs, for resolving its paths.
#[derive(Debug, Clone, Copy)]
pub struct ParseContext<'a> {
    pub working_directory: &'a Path,
    /// The user's home directory, for `~`. Without one, a `~` makes the
    /// command unanalysable.
    pub home: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandParse {
    Chain(Chain),
    /// The shell would run something other than the words seen here, or the
    /// text uses syntax the tokenizer does not model.
    Unanalysable {
        reason: String,
    },
}

impl CommandParse {
    pub fn chain(&self) -> Option<&Chain> {
        match self {
            CommandParse::Chain(chain) => Some(chain),
            CommandParse::Unanalysable { .. } => None,
        }
    }
}

/// The parts of a command line; `operators[i]` joins `parts[i]` and
/// `parts[i + 1]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    pub parts: Vec<CommandPart>,
    pub operators: Vec<ChainOperator>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainOperator {
    And,
    Or,
    /// `;` or a newline.
    Sequence,
    Pipe,
}

/// One simple command of a chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPart {
    /// The program as written, except that a known program named by its
    /// system path (`/bin/rm`) is recorded by its name (`rm`).
    pub program: String,
    /// The directory this part runs in: the command's, or the one a
    /// preceding `cd <dir> &&` moved to.
    pub working_directory: PathBuf,
    pub shape: PartShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartShape {
    /// A program in the table: options compare as a set, whatever their
    /// spelling or order; operands stay in order.
    Known {
        /// git's subcommand, when git's table knows it.
        subcommand: Option<String>,
        /// Options before the subcommand (git's `-C`, `-c`, …).
        global_options: BTreeSet<CommandOption>,
        options: BTreeSet<CommandOption>,
        operands: Vec<Argument>,
    },
    /// Any other program: its arguments word for word, in order, with
    /// path-like ones (`/…`, `./…`, `../…`, `.`, `..`, `~`) resolved.
    Unknown { arguments: Vec<Argument> },
}

/// A normalised option: `-r`, `-R` and `--recursive` of `rm` are all
/// `--recursive`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommandOption {
    pub name: String,
    pub value: Option<Argument>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Argument {
    /// Absolute and lexically normalised.
    Path(PathBuf),
    Word(String),
}

/// Parses `command` as it would run in `context.working_directory`.
pub fn parse_command(command: &str, context: &ParseContext) -> CommandParse {
    let unanalysable = |reason: &str| CommandParse::Unanalysable {
        reason: reason.to_string(),
    };
    let Some(line) = chain_line(command.trim()) else {
        return unanalysable(
            "uses shell syntax the parser does not model (a backslash, an unclosed quote, a \
             here-doc, `&`, or a redirect other than to /dev/null)",
        );
    };
    if let Some(found) = line.unanalysable {
        return CommandParse::Unanalysable {
            reason: format!("uses shell expansion ({found})"),
        };
    }
    if !line.home_tilde.is_empty() && context.home.is_none() {
        return unanalysable("uses `~` and the home directory is not known");
    }
    let operators: Vec<ChainOperator> = line
        .operators
        .iter()
        .map(|operator| match operator {
            Operator::And => ChainOperator::And,
            Operator::Or => ChainOperator::Or,
            Operator::Sequence => ChainOperator::Sequence,
            Operator::Pipe => ChainOperator::Pipe,
        })
        .collect();
    let mut working_directory = normalize_lexically(context.working_directory);
    let mut parts = Vec::with_capacity(line.commands.len());
    for (index, raw_words) in line.commands.iter().enumerate() {
        if raw_words.is_empty() {
            return unanalysable("has an operator with nothing on one side");
        }
        let words: Vec<Word> = raw_words
            .iter()
            .enumerate()
            .map(|(position, text)| Word {
                text,
                home: line.home_tilde.contains(&(index, position)),
            })
            .collect();
        if words[0].text.contains('=') && !words[0].text.starts_with('=') {
            return unanalysable("sets an environment variable before the program");
        }
        let resolver = Resolver {
            base: working_directory.clone(),
            home: context.home,
        };
        let part = parse_part(&words, &resolver);
        if part.program == "cd" {
            let next = operators.get(index);
            match (&part.shape, next) {
                (_, None) => {}
                (PartShape::Unknown { arguments }, Some(ChainOperator::And)) => {
                    match arguments.as_slice() {
                        [Argument::Path(directory)] => working_directory = directory.clone(),
                        [Argument::Word(directory)] if !directory.starts_with('-') => {
                            working_directory =
                                normalize_lexically(&working_directory.join(directory));
                        }
                        _ => {
                            return unanalysable(
                                "uses `cd` without a single directory, so later paths are unknown",
                            );
                        }
                    }
                }
                _ => {
                    return unanalysable(
                        "uses `cd` other than as `cd <dir> &&`, so later paths are unknown",
                    );
                }
            }
        }
        parts.push(part);
    }
    CommandParse::Chain(Chain { parts, operators })
}

struct Word<'a> {
    text: &'a str,
    /// Begins with an unquoted home tilde.
    home: bool,
}

struct Resolver<'a> {
    base: PathBuf,
    home: Option<&'a Path>,
}

impl Resolver<'_> {
    fn path(&self, word: &Word) -> Argument {
        if word.home
            && let Some(home) = self.home
        {
            let rest = word.text[1..].trim_start_matches('/');
            return Argument::Path(normalize_lexically(&home.join(rest)));
        }
        Argument::Path(normalize_lexically(&self.base.join(word.text)))
    }

    /// For unknown programs (D4): resolve only what looks like a path.
    fn path_if_path_like(&self, word: &Word) -> Argument {
        let text = word.text;
        let path_like = word.home
            || text.starts_with('/')
            || text.starts_with("./")
            || text.starts_with("../")
            || text == "."
            || text == "..";
        if path_like {
            self.path(word)
        } else {
            Argument::Word(text.to_string())
        }
    }

    fn value(&self, kind: Value, word: &Word) -> Argument {
        match kind {
            Value::Path => self.path(word),
            Value::Word | Value::None => Argument::Word(word.text.to_string()),
        }
    }

    /// An option's value written inside its word (`-ofile`, `--output=file`).
    /// A tilde there is not expanded by the shell, so it is literal.
    fn inline_value(&self, kind: Value, text: &str) -> Argument {
        self.value(kind, &Word { text, home: false })
    }
}

/// Directories whose programs are the system's, so `/bin/rm` is `rm`.
const SYSTEM_DIRECTORIES: [&str; 4] = ["/bin/", "/usr/bin/", "/sbin/", "/usr/sbin/"];

fn parse_part(words: &[Word], resolver: &Resolver) -> CommandPart {
    let written = words[0].text;
    let name = SYSTEM_DIRECTORIES
        .iter()
        .find_map(|directory| written.strip_prefix(directory))
        .filter(|name| !name.contains('/'))
        .unwrap_or(written);
    let arguments = &words[1..];
    let Some(grammar) = grammar(name) else {
        return CommandPart {
            program: written.to_string(),
            working_directory: resolver.base.clone(),
            shape: PartShape::Unknown {
                arguments: arguments
                    .iter()
                    .map(|word| resolver.path_if_path_like(word))
                    .collect(),
            },
        };
    };
    let shape = match grammar.operands {
        Operands::Git => parse_git(arguments, resolver),
        Operands::Find => parse_find(grammar, arguments, resolver),
        Operands::Dd => parse_dd(arguments, resolver),
        _ => {
            let (options, operands) = parse_options(grammar, arguments, resolver);
            PartShape::Known {
                subcommand: None,
                global_options: BTreeSet::new(),
                options,
                operands,
            }
        }
    };
    CommandPart {
        program: name.to_string(),
        working_directory: resolver.base.clone(),
        shape,
    }
}

/// Options anywhere among the operands, as GNU tools accept them; `--` ends
/// them. Reading BSD's `rm / -rf` this way treats `-rf` as options rather
/// than a file name, which errs toward the stricter reading.
fn parse_options(
    grammar: &Grammar,
    words: &[Word],
    resolver: &Resolver,
) -> (BTreeSet<CommandOption>, Vec<Argument>) {
    let mut options = BTreeSet::new();
    let mut raw_operands: Vec<(&Word, bool)> = Vec::new();
    let mut index = 0;
    let mut end_of_options = false;
    while index < words.len() {
        let word = &words[index];
        let text = word.text;
        index += 1;
        if end_of_options || !text.starts_with('-') || text == "-" {
            raw_operands.push((word, end_of_options));
            continue;
        }
        if text == "--" {
            end_of_options = true;
            continue;
        }
        if grammar.dash_mode_operand && is_symbolic_mode(text) {
            raw_operands.push((word, false));
            continue;
        }
        if let Some(flag) = grammar.flag(text) {
            let value = match flag.value {
                Value::None => None,
                kind => words.get(index).map(|next| {
                    index += 1;
                    resolver.value(kind, next)
                }),
            };
            options.insert(CommandOption {
                name: flag.canonical.to_string(),
                value,
            });
            continue;
        }
        if let Some((name, inline)) = text.split_once('=')
            && name.starts_with("--")
            && let Some(flag) = grammar.flag(name)
            && flag.value != Value::None
        {
            options.insert(CommandOption {
                name: flag.canonical.to_string(),
                value: Some(resolver.inline_value(flag.value, inline)),
            });
            continue;
        }
        if text.starts_with("--") {
            options.insert(CommandOption {
                name: text.to_string(),
                value: None,
            });
            continue;
        }
        // A group of short flags: `-rf`, `-ofile`, `-n5`.
        let letters = &text[1..];
        for (offset, letter) in letters.char_indices() {
            let spelling = format!("-{letter}");
            let rest = &letters[offset + letter.len_utf8()..];
            match grammar.flag(&spelling) {
                Some(flag) if flag.value == Value::None => {
                    options.insert(CommandOption {
                        name: flag.canonical.to_string(),
                        value: None,
                    });
                }
                Some(flag) => {
                    let value = if rest.is_empty() {
                        words.get(index).map(|next| {
                            index += 1;
                            resolver.value(flag.value, next)
                        })
                    } else {
                        Some(resolver.inline_value(flag.value, rest))
                    };
                    options.insert(CommandOption {
                        name: flag.canonical.to_string(),
                        value,
                    });
                    break;
                }
                None => {
                    options.insert(CommandOption {
                        name: format!("-{}", &letters[offset..]),
                        value: None,
                    });
                    break;
                }
            }
        }
    }
    let operands = raw_operands
        .into_iter()
        .enumerate()
        .map(
            |(position, (word, after_separator))| match grammar.operands {
                Operands::Paths => resolver.path(word),
                Operands::FirstWordThenPaths if position == 0 => {
                    Argument::Word(word.text.to_string())
                }
                Operands::FirstWordThenPaths => resolver.path(word),
                Operands::PathsAfterSeparator if after_separator => resolver.path(word),
                _ => Argument::Word(word.text.to_string()),
            },
        )
        .collect();
    (options, operands)
}

/// `chmod -w file`: a symbolic mode that starts with `-` is the mode, not an
/// option.
fn is_symbolic_mode(text: &str) -> bool {
    text.len() > 1 && text[1..].chars().all(|letter| "rwxXst".contains(letter))
}

/// git's global options, then a subcommand from the table, whose options
/// compare as a set; an unknown subcommand's arguments compare word for word.
/// `-C <dir>` moves where later paths resolve, as it moves git.
fn parse_git(words: &[Word], resolver: &Resolver) -> PartShape {
    let mut global_options = BTreeSet::new();
    let mut base = resolver.base.clone();
    let mut index = 0;
    while let Some(word) = words.get(index) {
        let text = word.text;
        if !text.starts_with('-') {
            break;
        }
        index += 1;
        let local = Resolver {
            base: base.clone(),
            home: resolver.home,
        };
        let (flag, inline) = match text.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (GIT_GLOBAL.flag(name), Some(value)),
            _ => (GIT_GLOBAL.flag(text), None),
        };
        let Some(flag) = flag else {
            global_options.insert(CommandOption {
                name: text.to_string(),
                value: None,
            });
            continue;
        };
        let value = match (flag.value, inline) {
            (Value::None, _) => None,
            (kind, Some(inline)) => Some(local.inline_value(kind, inline)),
            (kind, None) => words.get(index).map(|next| {
                index += 1;
                local.value(kind, next)
            }),
        };
        if flag.canonical == "-C"
            && let Some(Argument::Path(directory)) = &value
        {
            base = directory.clone();
        }
        global_options.insert(CommandOption {
            name: flag.canonical.to_string(),
            value,
        });
    }
    let resolver = Resolver {
        base,
        home: resolver.home,
    };
    let Some(subcommand) = words.get(index) else {
        return PartShape::Known {
            subcommand: None,
            global_options,
            options: BTreeSet::new(),
            operands: Vec::new(),
        };
    };
    let rest = &words[index + 1..];
    let (options, operands) = match git_subcommand(subcommand.text) {
        Some(grammar) => parse_options(grammar, rest, &resolver),
        None => (
            BTreeSet::new(),
            rest.iter()
                .map(|word| resolver.path_if_path_like(word))
                .collect(),
        ),
    };
    PartShape::Known {
        subcommand: Some(subcommand.text.to_string()),
        global_options,
        options,
        operands,
    }
}

/// `find [options] [paths] [expression]`: the leading options form the set,
/// the starting points are paths, and the expression stays in order.
fn parse_find(grammar: &Grammar, words: &[Word], resolver: &Resolver) -> PartShape {
    let mut options = BTreeSet::new();
    let mut index = 0;
    while let Some(flag) = words.get(index).and_then(|word| grammar.flag(word.text)) {
        index += 1;
        let value = match flag.value {
            Value::None => None,
            kind => words.get(index).map(|next| {
                index += 1;
                resolver.value(kind, next)
            }),
        };
        options.insert(CommandOption {
            name: flag.canonical.to_string(),
            value,
        });
    }
    let mut operands = Vec::new();
    while let Some(word) = words.get(index) {
        if word.text.starts_with('-') || word.text == "!" {
            break;
        }
        operands.push(resolver.path(word));
        index += 1;
    }
    operands.extend(
        words[index..]
            .iter()
            .map(|word| Argument::Word(word.text.to_string())),
    );
    PartShape::Known {
        subcommand: None,
        global_options: BTreeSet::new(),
        options,
        operands,
    }
}

/// `dd if=… of=… bs=…`: operands are `key=value` in any order, so they form
/// the option set; `if` and `of` are paths.
fn parse_dd(words: &[Word], resolver: &Resolver) -> PartShape {
    let mut options = BTreeSet::new();
    let mut operands = Vec::new();
    for word in words {
        match word.text.split_once('=') {
            Some((key, value)) => {
                let kind = if matches!(key, "if" | "of") {
                    Value::Path
                } else {
                    Value::Word
                };
                options.insert(CommandOption {
                    name: key.to_string(),
                    value: Some(resolver.inline_value(kind, value)),
                });
            }
            None => operands.push(Argument::Word(word.text.to_string())),
        }
    }
    PartShape::Known {
        subcommand: None,
        global_options: BTreeSet::new(),
        options,
        operands,
    }
}

// --- The table of known programs (D3) -------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Value {
    None,
    Word,
    Path,
}

struct Flag {
    /// The name every spelling normalises to.
    canonical: &'static str,
    /// Every spelling: `-r`, `-R`, `--recursive`.
    spellings: &'static [&'static str],
    value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operands {
    /// Every operand is a path (`rm`, `cp`, `mv`, …).
    Paths,
    /// A mode or owner, then paths (`chmod`, `chown`, `chgrp`).
    FirstWordThenPaths,
    /// Words, except after `--` (git's revisions and pathspecs).
    PathsAfterSeparator,
    /// Words (`curl`'s and `wget`'s URLs, git's branch names).
    Words,
    Git,
    Find,
    Dd,
}

struct Grammar {
    flags: &'static [Flag],
    operands: Operands,
    /// `chmod -w`: a symbolic mode may start with `-`.
    dash_mode_operand: bool,
}

impl Grammar {
    fn flag(&self, spelling: &str) -> Option<&'static Flag> {
        self.flags
            .iter()
            .find(|flag| flag.spellings.contains(&spelling))
    }
}

const fn flag(canonical: &'static str, spellings: &'static [&'static str]) -> Flag {
    Flag {
        canonical,
        spellings,
        value: Value::None,
    }
}

const fn word(canonical: &'static str, spellings: &'static [&'static str]) -> Flag {
    Flag {
        canonical,
        spellings,
        value: Value::Word,
    }
}

const fn path(canonical: &'static str, spellings: &'static [&'static str]) -> Flag {
    Flag {
        canonical,
        spellings,
        value: Value::Path,
    }
}

const fn grammar_of(flags: &'static [Flag], operands: Operands) -> Grammar {
    Grammar {
        flags,
        operands,
        dash_mode_operand: false,
    }
}

const RM: Grammar = grammar_of(
    &[
        flag("--recursive", &["-r", "-R", "--recursive"]),
        flag("--force", &["-f", "--force"]),
        flag("--dir", &["-d", "--dir"]),
        flag("--interactive", &["-i", "--interactive"]),
        flag("-I", &["-I"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag("--one-file-system", &["-x", "--one-file-system"]),
        flag("--no-preserve-root", &["--no-preserve-root"]),
        flag("--preserve-root", &["--preserve-root"]),
        flag("-P", &["-P"]),
        flag("-W", &["-W"]),
    ],
    Operands::Paths,
);

const RMDIR: Grammar = grammar_of(
    &[
        flag("--parents", &["-p", "--parents"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag(
            "--ignore-fail-on-non-empty",
            &["--ignore-fail-on-non-empty"],
        ),
    ],
    Operands::Paths,
);

const MV: Grammar = grammar_of(
    &[
        flag("--force", &["-f", "--force"]),
        flag("--interactive", &["-i", "--interactive"]),
        flag("--no-clobber", &["-n", "--no-clobber"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag("--update", &["-u", "--update"]),
        flag("--backup", &["-b"]),
        flag("--no-target-directory", &["-T", "--no-target-directory"]),
        flag("--strip-trailing-slashes", &["--strip-trailing-slashes"]),
        flag("-h", &["-h"]),
        path("--target-directory", &["-t", "--target-directory"]),
    ],
    Operands::Paths,
);

const CP: Grammar = grammar_of(
    &[
        flag("--recursive", &["-r", "-R", "--recursive"]),
        flag("--force", &["-f", "--force"]),
        flag("--interactive", &["-i", "--interactive"]),
        flag("--no-clobber", &["-n", "--no-clobber"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag("--archive", &["-a", "--archive"]),
        flag("--update", &["-u", "--update"]),
        flag("--link", &["-l", "--link"]),
        flag("--symbolic-link", &["-s", "--symbolic-link"]),
        flag("--dereference", &["-L", "--dereference"]),
        flag("--no-dereference", &["-P", "--no-dereference"]),
        flag("--no-target-directory", &["-T", "--no-target-directory"]),
        flag("-p", &["-p"]),
        flag("-H", &["-H"]),
        flag("-X", &["-X"]),
        flag("-c", &["-c"]),
        path("--target-directory", &["-t", "--target-directory"]),
    ],
    Operands::Paths,
);

const LN: Grammar = grammar_of(
    &[
        flag("--symbolic", &["-s", "--symbolic"]),
        flag("--force", &["-f", "--force"]),
        flag("--no-dereference", &["-n", "--no-dereference"]),
        flag("--interactive", &["-i", "--interactive"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag("--relative", &["-r", "--relative"]),
        flag("--no-target-directory", &["-T", "--no-target-directory"]),
        flag("-h", &["-h"]),
        flag("-F", &["-F"]),
        path("--target-directory", &["-t", "--target-directory"]),
    ],
    Operands::Paths,
);

const MKDIR: Grammar = grammar_of(
    &[
        flag("--parents", &["-p", "--parents"]),
        flag("--verbose", &["-v", "--verbose"]),
        word("--mode", &["-m", "--mode"]),
    ],
    Operands::Paths,
);

const TOUCH: Grammar = grammar_of(
    &[
        flag("-a", &["-a"]),
        flag("-m", &["-m"]),
        flag("--no-create", &["-c", "--no-create"]),
        flag("--no-dereference", &["-h", "--no-dereference"]),
        path("--reference", &["-r", "--reference"]),
        word("-t", &["-t"]),
        word("--date", &["-d", "--date"]),
        word("-A", &["-A"]),
    ],
    Operands::Paths,
);

const OWNERSHIP_FLAGS: &[Flag] = &[
    flag("--recursive", &["-R", "--recursive"]),
    flag("--silent", &["-f", "--silent", "--quiet"]),
    flag("--verbose", &["-v", "--verbose"]),
    flag("--changes", &["-c", "--changes"]),
    flag("--no-dereference", &["-h", "--no-dereference"]),
    flag("-H", &["-H"]),
    flag("-L", &["-L"]),
    flag("-P", &["-P"]),
    flag("-n", &["-n"]),
    path("--reference", &["--reference"]),
    word("--from", &["--from"]),
];

const CHMOD: Grammar = Grammar {
    flags: OWNERSHIP_FLAGS,
    operands: Operands::FirstWordThenPaths,
    dash_mode_operand: true,
};

const CHOWN: Grammar = grammar_of(OWNERSHIP_FLAGS, Operands::FirstWordThenPaths);

const FIND: Grammar = grammar_of(
    &[
        flag("-H", &["-H"]),
        flag("-L", &["-L"]),
        flag("-P", &["-P"]),
        flag("-E", &["-E"]),
        flag("-X", &["-X"]),
        flag("-d", &["-d"]),
        flag("-s", &["-s"]),
        flag("-x", &["-x"]),
        path("-f", &["-f"]),
    ],
    Operands::Find,
);

const DD: Grammar = grammar_of(&[], Operands::Dd);

const CURL: Grammar = grammar_of(
    &[
        flag("--silent", &["-s", "--silent"]),
        flag("--show-error", &["-S", "--show-error"]),
        flag("--location", &["-L", "--location"]),
        flag("--fail", &["-f", "--fail"]),
        flag("--insecure", &["-k", "--insecure"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag("--include", &["-i", "--include"]),
        flag("--head", &["-I", "--head"]),
        flag("--get", &["-G", "--get"]),
        flag("--remote-name", &["-O", "--remote-name"]),
        flag("--remote-name-all", &["--remote-name-all"]),
        flag("--progress-bar", &["-#", "--progress-bar"]),
        flag("--no-buffer", &["-N", "--no-buffer"]),
        flag("--compressed", &["--compressed"]),
        flag("--fail-with-body", &["--fail-with-body"]),
        path("--output", &["-o", "--output"]),
        path("--upload-file", &["-T", "--upload-file"]),
        path("--cookie-jar", &["-c", "--cookie-jar"]),
        path("--dump-header", &["-D", "--dump-header"]),
        path("--config", &["-K", "--config"]),
        path("--cacert", &["--cacert"]),
        word("--request", &["-X", "--request"]),
        word("--data", &["-d", "--data"]),
        word("--data-binary", &["--data-binary"]),
        word("--data-raw", &["--data-raw"]),
        word("--data-urlencode", &["--data-urlencode"]),
        word("--form", &["-F", "--form"]),
        word("--header", &["-H", "--header"]),
        word("--user", &["-u", "--user"]),
        word("--user-agent", &["-A", "--user-agent"]),
        word("--referer", &["-e", "--referer"]),
        word("--cookie", &["-b", "--cookie"]),
        word("--url", &["--url"]),
        word("--write-out", &["-w", "--write-out"]),
        word("--max-time", &["-m", "--max-time"]),
        word("--connect-timeout", &["--connect-timeout"]),
        word("--proxy", &["-x", "--proxy"]),
        word("--range", &["-r", "--range"]),
        word("--continue-at", &["-C", "--continue-at"]),
        word("--cert", &["-E", "--cert"]),
        word("--retry", &["--retry"]),
    ],
    Operands::Words,
);

/// The two-letter short form of `--no-directories` is left out: the spelling
/// checker reads it as a typo, and unlisted it still compares as written.
const WGET: Grammar = grammar_of(
    &[
        flag("--quiet", &["-q", "--quiet"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag("--no-verbose", &["-nv", "--no-verbose"]),
        flag("--continue", &["-c", "--continue"]),
        flag("--recursive", &["-r", "--recursive"]),
        flag("--timestamping", &["-N", "--timestamping"]),
        flag("--no-clobber", &["-nc", "--no-clobber"]),
        flag("--no-directories", &["--no-directories"]),
        flag("--no-host-directories", &["-nH", "--no-host-directories"]),
        flag("--no-parent", &["-np", "--no-parent"]),
        flag("--convert-links", &["-k", "--convert-links"]),
        flag("--mirror", &["-m", "--mirror"]),
        flag("--page-requisites", &["-p", "--page-requisites"]),
        flag("--force-directories", &["-x", "--force-directories"]),
        flag("--background", &["-b", "--background"]),
        path("--output-document", &["-O", "--output-document"]),
        path("--output-file", &["-o", "--output-file"]),
        path("--directory-prefix", &["-P", "--directory-prefix"]),
        path("--input-file", &["-i", "--input-file"]),
        word("--user-agent", &["-U", "--user-agent"]),
        word("--header", &["--header"]),
        word("--tries", &["-t", "--tries"]),
        word("--timeout", &["-T", "--timeout"]),
        word("--level", &["-l", "--level"]),
        word("--execute", &["-e", "--execute"]),
        word("--post-data", &["--post-data"]),
        word("--method", &["--method"]),
    ],
    Operands::Words,
);

const GIT: Grammar = grammar_of(&[], Operands::Git);

const GIT_GLOBAL: Grammar = grammar_of(
    &[
        path("-C", &["-C"]),
        word("-c", &["-c"]),
        word("--config-env", &["--config-env"]),
        path("--git-dir", &["--git-dir"]),
        path("--work-tree", &["--work-tree"]),
        word("--namespace", &["--namespace"]),
        flag("--no-pager", &["-P", "--no-pager"]),
        flag("--paginate", &["-p", "--paginate"]),
        flag("--bare", &["--bare"]),
        flag("--no-replace-objects", &["--no-replace-objects"]),
        flag("--literal-pathspecs", &["--literal-pathspecs"]),
        flag("--no-optional-locks", &["--no-optional-locks"]),
    ],
    Operands::Words,
);

const GIT_RESET: Grammar = grammar_of(
    &[
        flag("--hard", &["--hard"]),
        flag("--soft", &["--soft"]),
        flag("--mixed", &["--mixed"]),
        flag("--keep", &["--keep"]),
        flag("--merge", &["--merge"]),
        flag("--quiet", &["-q", "--quiet"]),
        flag("--patch", &["-p", "--patch"]),
        flag("-N", &["-N"]),
    ],
    Operands::PathsAfterSeparator,
);

const GIT_CLEAN: Grammar = grammar_of(
    &[
        flag("--force", &["-f", "--force"]),
        flag("-d", &["-d"]),
        flag("-x", &["-x"]),
        flag("-X", &["-X"]),
        flag("--dry-run", &["-n", "--dry-run"]),
        flag("--quiet", &["-q", "--quiet"]),
        flag("--interactive", &["-i", "--interactive"]),
        word("--exclude", &["-e", "--exclude"]),
    ],
    Operands::Paths,
);

const GIT_RM: Grammar = grammar_of(
    &[
        flag("-r", &["-r"]),
        flag("--force", &["-f", "--force"]),
        flag("--dry-run", &["-n", "--dry-run"]),
        flag("--quiet", &["-q", "--quiet"]),
        flag("--cached", &["--cached"]),
        flag("--ignore-unmatch", &["--ignore-unmatch"]),
    ],
    Operands::Paths,
);

const GIT_ADD: Grammar = grammar_of(
    &[
        flag("--all", &["-A", "--all"]),
        flag("--update", &["-u", "--update"]),
        flag("--force", &["-f", "--force"]),
        flag("--dry-run", &["-n", "--dry-run"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag("--patch", &["-p", "--patch"]),
        flag("--intent-to-add", &["-N", "--intent-to-add"]),
        flag("--interactive", &["-i", "--interactive"]),
    ],
    Operands::Paths,
);

const GIT_RESTORE: Grammar = grammar_of(
    &[
        flag("--staged", &["-S", "--staged"]),
        flag("--worktree", &["-W", "--worktree"]),
        flag("--patch", &["-p", "--patch"]),
        flag("--quiet", &["-q", "--quiet"]),
        flag("--ours", &["--ours"]),
        flag("--theirs", &["--theirs"]),
        word("--source", &["-s", "--source"]),
    ],
    Operands::Paths,
);

const GIT_CHECKOUT: Grammar = grammar_of(
    &[
        flag("--force", &["-f", "--force"]),
        flag("--quiet", &["-q", "--quiet"]),
        flag("--detach", &["--detach"]),
        flag("--patch", &["-p", "--patch"]),
        flag("--ours", &["--ours"]),
        flag("--theirs", &["--theirs"]),
        flag("--track", &["-t", "--track"]),
        word("-b", &["-b"]),
        word("-B", &["-B"]),
    ],
    Operands::PathsAfterSeparator,
);

const GIT_SWITCH: Grammar = grammar_of(
    &[
        flag("--detach", &["-d", "--detach"]),
        flag("--force", &["-f", "--force", "--discard-changes"]),
        flag("--quiet", &["-q", "--quiet"]),
        word("--create", &["-c", "--create"]),
        word("--force-create", &["-C", "--force-create"]),
    ],
    Operands::Words,
);

const GIT_BRANCH: Grammar = grammar_of(
    &[
        flag("--delete", &["-d", "--delete"]),
        flag("-D", &["-D"]),
        flag("--force", &["-f", "--force"]),
        flag("--move", &["-m", "--move"]),
        flag("-M", &["-M"]),
        flag("--copy", &["-c", "--copy"]),
        flag("-C", &["-C"]),
        flag("--all", &["-a", "--all"]),
        flag("--remotes", &["-r", "--remotes"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag("--list", &["-l", "--list"]),
        flag("--show-current", &["--show-current"]),
        word("--set-upstream-to", &["-u", "--set-upstream-to"]),
    ],
    Operands::Words,
);

const GIT_PUSH: Grammar = grammar_of(
    &[
        flag("--force", &["-f", "--force"]),
        flag("--force-with-lease", &["--force-with-lease"]),
        flag("--set-upstream", &["-u", "--set-upstream"]),
        flag("--delete", &["-d", "--delete"]),
        flag("--tags", &["--tags"]),
        flag("--follow-tags", &["--follow-tags"]),
        flag("--all", &["--all"]),
        flag("--mirror", &["--mirror"]),
        flag("--prune", &["--prune"]),
        flag("--atomic", &["--atomic"]),
        flag("--no-verify", &["--no-verify"]),
        flag("--dry-run", &["-n", "--dry-run"]),
        flag("--quiet", &["-q", "--quiet"]),
        flag("--verbose", &["-v", "--verbose"]),
    ],
    Operands::Words,
);

const GIT_COMMIT: Grammar = grammar_of(
    &[
        flag("--all", &["-a", "--all"]),
        flag("--amend", &["--amend"]),
        flag("--no-verify", &["-n", "--no-verify"]),
        flag("--quiet", &["-q", "--quiet"]),
        flag("--verbose", &["-v", "--verbose"]),
        flag("--allow-empty", &["--allow-empty"]),
        flag("--no-edit", &["--no-edit"]),
        flag("--signoff", &["-s", "--signoff"]),
        word("--message", &["-m", "--message"]),
        path("--file", &["-F", "--file"]),
    ],
    Operands::Words,
);

fn grammar(program: &str) -> Option<&'static Grammar> {
    Some(match program {
        "rm" => &RM,
        "rmdir" => &RMDIR,
        "mv" => &MV,
        "cp" => &CP,
        "ln" => &LN,
        "mkdir" => &MKDIR,
        "touch" => &TOUCH,
        "chmod" => &CHMOD,
        "chown" | "chgrp" => &CHOWN,
        "find" => &FIND,
        "dd" => &DD,
        "curl" => &CURL,
        "wget" => &WGET,
        "git" => &GIT,
        _ => return None,
    })
}

fn git_subcommand(subcommand: &str) -> Option<&'static Grammar> {
    Some(match subcommand {
        "reset" => &GIT_RESET,
        "clean" => &GIT_CLEAN,
        "rm" => &GIT_RM,
        "add" => &GIT_ADD,
        "restore" => &GIT_RESTORE,
        "checkout" => &GIT_CHECKOUT,
        "switch" => &GIT_SWITCH,
        "branch" => &GIT_BRANCH,
        "push" => &GIT_PUSH,
        "commit" => &GIT_COMMIT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Argument, ChainOperator, CommandOption, CommandParse, CommandPart, ParseContext, PartShape,
        parse_command,
    };
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    const WORKING_DIRECTORY: &str = "/Users/me/project";
    const HOME: &str = "/Users/me";

    fn parse(command: &str) -> CommandParse {
        parse_command(
            command,
            &ParseContext {
                working_directory: Path::new(WORKING_DIRECTORY),
                home: Some(Path::new(HOME)),
            },
        )
    }

    fn parts(command: &str) -> Vec<CommandPart> {
        match parse(command) {
            CommandParse::Chain(chain) => chain.parts,
            CommandParse::Unanalysable { reason } => {
                panic!("`{command}` should parse, but: {reason}")
            }
        }
    }

    fn only_part(command: &str) -> CommandPart {
        let mut parts = parts(command);
        assert_eq!(parts.len(), 1, "`{command}`");
        parts.remove(0)
    }

    fn operands(part: &CommandPart) -> &[Argument] {
        match &part.shape {
            PartShape::Known { operands, .. } => operands,
            PartShape::Unknown { arguments } => arguments,
        }
    }

    fn options(part: &CommandPart) -> Vec<&str> {
        match &part.shape {
            PartShape::Known { options, .. } => {
                options.iter().map(|option| option.name.as_str()).collect()
            }
            PartShape::Unknown { .. } => panic!("{} is not a known program", part.program),
        }
    }

    fn path(text: &str) -> Argument {
        Argument::Path(PathBuf::from(text))
    }

    fn word(text: &str) -> Argument {
        Argument::Word(text.to_string())
    }

    fn assert_unanalysable(command: &str) {
        assert!(
            matches!(parse(command), CommandParse::Unanalysable { .. }),
            "`{command}` should be unanalysable, got {:?}",
            parse(command)
        );
    }

    #[test]
    fn rm_options_in_any_spelling_parse_equal() {
        let expected = only_part("rm -rf /");
        assert_eq!(options(&expected), ["--force", "--recursive"]);
        assert_eq!(operands(&expected), [path("/")]);
        for spelling in [
            "rm -fr /",
            "rm -r -f /",
            "rm -R -f /",
            "rm --recursive --force /",
            "rm --force -r /",
            "rm -f / -r",
            "/bin/rm -rf /",
            "rm -rf / 2>/dev/null",
        ] {
            assert_eq!(only_part(spelling), expected, "`{spelling}`");
        }
        assert_ne!(
            only_part("rm -r /"),
            expected,
            "`-f` is part of the command"
        );
        assert_ne!(only_part("rm -rf /tmp"), expected, "the path is part of it");
    }

    #[test]
    fn paths_resolve_lexically_against_the_working_directory() {
        let root = only_part("rm -rf /");
        for spelling in [
            "rm -rf /tmp/..",
            "rm -rf /tmp/../",
            "rm -rf //",
            "rm -rf ../../..",
            "rm -rf /..",
        ] {
            assert_eq!(only_part(spelling), root, "`{spelling}`");
        }
        assert_eq!(operands(&only_part("rm -rf /tmp/")), [path("/tmp")]);
        assert_eq!(operands(&only_part("rm -rf .")), [path(WORKING_DIRECTORY)]);
        assert_eq!(operands(&only_part("rm -rf ..")), [path(HOME)]);
        assert_eq!(
            operands(&only_part("rm -rf ./target/../src")),
            [path("/Users/me/project/src")]
        );
        assert_eq!(
            operands(&only_part("rm -rf target")),
            [path("/Users/me/project/target")],
            "a known program's operands are paths even without `./`"
        );
        assert_eq!(operands(&only_part("rm -rf ~")), [path(HOME)]);
        assert_eq!(
            operands(&only_part("rm -rf ~/x/../y")),
            [path("/Users/me/y")]
        );
        assert_eq!(
            operands(&only_part("rm -rf '~/x'")),
            [path("/Users/me/project/~/x")],
            "a quoted `~` is a directory named `~`"
        );
        assert_eq!(
            operands(&only_part("rm -rf -- -x")),
            [path("/Users/me/project/-x")],
            "after `--` a dash word is a file"
        );
    }

    #[test]
    fn expansion_substitution_and_globs_are_unanalysable() {
        for command in [
            "echo $(rm -rf /)",
            "rm -rf $HOME",
            "rm -rf \"$HOME\"",
            "rm -rf ${HOME}",
            "rm -rf `pwd`",
            "rm -rf *",
            "rm -rf /tmp/*",
            "rm -rf /tmp/?",
            "rm -rf {a,b}",
            "cat <<EOF",
            "rm -rf a\\ b",
            "rm -rf ~other",
            "X=1 rm -rf /",
            "(rm -rf /)",
            "rm -rf / &",
            "rm -rf / > out.txt",
            "rm 'unclosed",
            "a && && b",
        ] {
            assert_unanalysable(command);
        }
        // Quoted, the same characters are literal and the command parses.
        assert_eq!(
            operands(&only_part("rm -rf '*' '$HOME'")),
            [path("/Users/me/project/*"), path("/Users/me/project/$HOME")]
        );
    }

    #[test]
    fn a_tilde_without_a_home_directory_is_unanalysable() {
        let parsed = parse_command(
            "rm -rf ~/x",
            &ParseContext {
                working_directory: Path::new(WORKING_DIRECTORY),
                home: None,
            },
        );
        assert!(matches!(parsed, CommandParse::Unanalysable { .. }));
    }

    #[test]
    fn chains_split_into_parts_and_quotes_are_respected() {
        let CommandParse::Chain(chain) =
            parse("cargo fmt --check && cargo clippy || echo 'a && b; c | d' ; ls | wc -l")
        else {
            panic!("should parse");
        };
        assert_eq!(
            chain.operators,
            [
                ChainOperator::And,
                ChainOperator::Or,
                ChainOperator::Sequence,
                ChainOperator::Pipe
            ]
        );
        let programs: Vec<&str> = chain
            .parts
            .iter()
            .map(|part| part.program.as_str())
            .collect();
        assert_eq!(programs, ["cargo", "cargo", "echo", "ls", "wc"]);
        assert_eq!(
            operands(&chain.parts[2]),
            [word("a && b; c | d")],
            "operators inside quotes are words"
        );
        assert_eq!(
            operands(&chain.parts[0]),
            [word("fmt"), word("--check")],
            "unknown programs compare word for word"
        );
    }

    #[test]
    fn unknown_programs_compare_word_for_word_with_path_like_words_resolved() {
        assert_ne!(
            only_part("cargo test -q --lib"),
            only_part("cargo test --lib -q")
        );
        assert_eq!(
            operands(&only_part("tool ./a ../b /c/../d ~/e f --out=./g")),
            [
                path("/Users/me/project/a"),
                path("/Users/me/b"),
                path("/d"),
                path("/Users/me/e"),
                word("f"),
                word("--out=./g"),
            ]
        );
    }

    #[test]
    fn a_cd_and_moves_where_later_parts_resolve() {
        let parts = parts("cd ../other && rm -rf build");
        assert_eq!(parts[1].working_directory, Path::new("/Users/me/other"));
        assert_eq!(operands(&parts[1]), [path("/Users/me/other/build")]);
        assert_eq!(operands(&only_part("cd /tmp")), [path("/tmp")]);
        assert_unanalysable("cd /tmp; rm -rf ..");
        assert_unanalysable("cd /tmp || rm -rf ..");
        assert_unanalysable("cd && rm -rf x");
    }

    #[test]
    fn options_with_values_take_them_in_any_spelling() {
        let output = |part: &CommandPart| match &part.shape {
            PartShape::Known { options, .. } => options.clone(),
            PartShape::Unknown { .. } => panic!("curl is known"),
        };
        let expected: BTreeSet<CommandOption> = [
            CommandOption {
                name: "--output".to_string(),
                value: Some(path("/Users/me/project/out.txt")),
            },
            CommandOption {
                name: "--silent".to_string(),
                value: None,
            },
        ]
        .into();
        for spelling in [
            "curl -so out.txt https://example.com",
            "curl -s -o out.txt https://example.com",
            "curl -s -o./out.txt https://example.com",
            "curl --output=out.txt --silent https://example.com",
            "curl https://example.com --silent --output ./out.txt",
        ] {
            let part = only_part(spelling);
            assert_eq!(output(&part), expected, "`{spelling}`");
            assert_eq!(operands(&part), [word("https://example.com")]);
        }
        assert_eq!(
            options(&only_part("wget -nv -O x https://example.com")),
            ["--no-verbose", "--output-document"],
            "wget's two-letter short options are one option"
        );
    }

    #[test]
    fn git_normalises_its_global_options_and_known_subcommands() {
        let expected = only_part("git clean -fd");
        assert_eq!(options(&expected), ["--force", "-d"]);
        assert_eq!(only_part("git clean -d --force"), expected);
        assert_eq!(only_part("git clean -d -f"), expected);
        assert_eq!(
            only_part("git reset -q --hard"),
            only_part("git reset --hard --quiet")
        );
        let PartShape::Known {
            subcommand,
            global_options,
            operands,
            ..
        } = only_part("git -C ../other clean -f -d build").shape
        else {
            panic!("git is known");
        };
        assert_eq!(subcommand.as_deref(), Some("clean"));
        assert!(global_options.contains(&CommandOption {
            name: "-C".to_string(),
            value: Some(path("/Users/me/other")),
        }));
        assert_eq!(
            operands,
            [path("/Users/me/other/build")],
            "`-C` moves where git resolves paths"
        );
        let push = parse("git push -f origin main");
        assert_eq!(
            push.chain().map(|chain| options(&chain.parts[0])),
            Some(vec!["--force"])
        );
        assert_eq!(
            only_part("git push --force origin main"),
            only_part("git push -f origin main")
        );
        assert_ne!(
            only_part("git stash drop -q"),
            only_part("git stash -q drop"),
            "an unknown subcommand compares word for word"
        );
    }

    #[test]
    fn chmod_dd_and_find_follow_their_own_grammars() {
        let chmod = only_part("chmod -R 755 ./bin");
        assert_eq!(options(&chmod), ["--recursive"]);
        assert_eq!(
            operands(&chmod),
            [word("755"), path("/Users/me/project/bin")]
        );
        assert_eq!(
            operands(&only_part("chmod -w file")),
            [word("-w"), path("/Users/me/project/file")],
            "a symbolic mode may start with `-`"
        );
        assert_eq!(
            only_part("dd if=/dev/zero of=/dev/disk2 bs=1m"),
            only_part("dd bs=1m of=/dev/disk2 if=/dev/zero")
        );
        let find = only_part("find -L .. -name '*.tmp' -delete");
        assert_eq!(options(&find), ["-L"]);
        assert_eq!(
            operands(&find),
            [path(HOME), word("-name"), word("*.tmp"), word("-delete")]
        );
    }

    #[test]
    fn an_unknown_short_flag_keeps_the_rest_of_its_group() {
        assert_eq!(options(&only_part("rm -rZf x")), ["--recursive", "-Zf"]);
    }
}
