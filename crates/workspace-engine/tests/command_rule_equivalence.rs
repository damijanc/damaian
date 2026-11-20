//! Spec 111 Task 3 (context.md D6): the old command keys migrate into rules
//! with the same effect. A corpus of commands — every command in the
//! command policy's own tests, the destructive patterns, the entries the
//! legacy-key scenario configures, and a recorded set from real sessions —
//! goes through the old policy and through the migrated rules, and every
//! decision must agree except the differences listed in
//! [`KNOWN_DIFFERENCES`], each with its reason.
//!
//! What is compared is the part of the decision the old keys and the
//! built-in destructive patterns made, because that is what rules replace:
//! the old policy blocked a command (blocklist or destructive pattern),
//! allowed it (allowlist), or left it to the rest of the classifier. The
//! rules deny, allow, leave it to the defaults, or ask when the command
//! cannot be analysed; that last one agrees only when the old policy asked
//! too. The rest of the classifier is shared, so it is not compared here.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use workspace_engine::command_policy::{ApprovalCause, CommandPolicy};
use workspace_engine::command_rules::{
    CommandRule, RuleContext, RuleEffect, RuleSource, RuleVerdict, evaluate_command,
};
use workspace_engine::config::{Config, LegacyCommandRules, RepositoryConfigReport};
use workspace_engine::repository_id_for_root;

const HOME: &str = "/Users/me";

/// Where the recorded commands ran, as the fixture writes it; replaced with
/// the test repository's root.
const RECORDED_ROOT: &str = "/Users/me/project";

/// Every command the command policy's tests classify (`command_policy.rs`
/// `mod tests`), as written there.
const POLICY_TEST_COMMANDS: &[&str] = &[
    "npm test",
    "git push",
    "  cargo test  ",
    "rm -rf /",
    "cat a.txt | grep b",
    "npm test && npm run lint",
    "echo hi > out.txt",
    "echo $(whoami)",
    "npm test\ncat /etc/passwd",
    "npm test\rcat /etc/passwd",
    "cat ../secrets/id_rsa",
    "ls ../../other-project",
    "cat /etc/passwd",
    "cat ~/secrets.txt",
    "git diff --output=README.md",
    "git log --output=x",
    "ls $HOME/.ssh",
    "git diff --no-index $HOME/x README.md",
    "ls ~",
    "ls *",
    "ls \"$HOME\"",
    "ls =ls",
    "git log --pretty=~/x",
    "git log --format=`id`",
    "ls secrets/",
    "ls secrets",
    "git log -- secrets/token",
    "git -c core.pager=x log",
    "git --git-dir=x log",
    "git --work-tree=/tmp status",
    "git --exec-path=/tmp log",
    "git diff --no-index a b",
    "git diff --ext-diff",
    "git show --textconv HEAD",
    "git log --since=yesterday",
    "git status --ignored",
    "git -C /tmp status",
    "git diff",
    "git show HEAD",
    "git show --stat",
    "git diff --cached -- README.md",
    "git log -p",
    "git log --patch -n 3",
    "git log -u",
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
    "ls /etc",
    "ls ../",
    "git log /etc/passwd",
    "git log HEAD:README.md",
    "git log HEAD:../../.env",
    "ls ../secrets",
    "ls escape",
    "ls -R",
    "ls -R .",
    "git diff -- '*.env'",
    "git log -- ':(glob)**'",
    "git diff -- :!README.md",
    "git log --format='%h $x' \"a b\" ''",
    "git diff HEAD~1",
    "ls 'unclosed",
    "ls a\\ b",
    "ls a{b,c}",
    "ls #x",
    "git status",
    "git statusx",
    "pwd -P",
    "ls --color=always",
    "git status | head",
    "git status | head -x",
    "cd .claude/worktrees/x && git status --short && git log --oneline -n 5 | head -n 3",
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
    "git status -sb && git log --stat | tail -n 1",
    "git log --format='%h|%s' -n 3",
    "git log --oneline | grep fix secrets.txt",
    "ls | xargs cat",
    "git status; rm x",
    "git status || true",
    "git log > out.txt",
    "cd /tmp && ls",
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
    "git log --output=x | head -n 1",
    "git show HEAD:.env | head -n 1",
    "cd secrets && ls",
    "cd .. && ls",
    "cd missing && ls",
    "cd escape && ls",
    "cd engine && ls",
    "cd ../secrets && ls",
    "cd crates && ls engine",
    "cd crates && git log HEAD:README.md | head -n 5",
    "cd crates && ls ../secrets",
    "cd crates && ls ../..",
    "cd crates && ls -R ..",
    "cd x&&git log 'a|b' | head -n 1",
    "a || b",
    "a |& b",
    "a & b",
    "a &| b",
    "a > b",
    "a >> /dev/null",
    "a < b",
    "a 2>x",
    "a &> /dev/null",
    "a 2>&3",
    "a '2'>/dev/null",
    "a\rb",
    "a 2>/dev/null; b >/dev/null 2>&1\nc 2> /dev/null;",
    "echo '=== CHANGELOG Unreleased ==='; sed -n '1,80p' CHANGELOG.md",
    "git log --oneline -20 2>/dev/null; echo \"---\"; git branch -a; echo \"---\"; git remote -v",
    "git worktree list --porcelain | head -30",
    "git rev-list --left-right --count main...HEAD",
    "git for-each-ref --sort=-committerdate '--format=%(refname:short)' refs/heads",
    "git rev-parse --abbrev-ref HEAD; git merge-base --is-ancestor main HEAD",
    "ls src | head",
    "ls src | tail -80",
    "pwd\nls -la\n",
    "echo done;",
    "cat notes.md",
    "cat -n notes.md src/lib.rs",
    "head -n 5 notes.md",
    "head -20 notes.md",
    "tail -n3 notes.md",
    "wc -l notes.md",
    "sed -n '1,40p' notes.md",
    "sed -n '/^## /p;$p' notes.md",
    "sed -nE '3p' notes.md",
    "grep -n Status notes.md",
    "grep -c -e Status notes.md src/lib.rs",
    "grep -A 2 -B2 -m 1 TODO notes.md",
    "grep -nE '^## (Task|Status)' notes.md | head -20",
    "cd src && cat lib.rs",
    "cat .env",
    "sed -n 1p secrets/key.txt",
    "head -n 5 ../outside.txt",
    "cat /etc/hosts",
    "grep x notes.md .env",
    "grep -r x .",
    "grep -R x src",
    "grep --include=*.md x notes.md",
    "grep -f patterns notes.md",
    "grep x",
    "cat",
    "sed -n 'w out' notes.md",
    "sed -n '1p;w out' notes.md",
    "sed -n '1e date' notes.md",
    "sed '1p' notes.md",
    "sed -i s/a/b/ notes.md",
    "tail -f notes.md",
    "tail -n +2 notes.md",
    "cat notes.md > copy.md",
    "echo $HOME",
    "echo hi > f",
    "echo \"$(id)\"",
    "cd src; ls",
    "cd src && ls; ls ..",
    "git branch -D old",
    "git branch new-branch",
    "git remote add x y",
    "git worktree add ../x",
    "git rev-parse --git-path x --output y",
    "git -C src show HEAD:lib.rs",
    "git -C src -C . status",
    "git -C src status --short",
    "git -C sub log --oneline -n 3",
    "git -C sub log -- secrets",
    "git -C secrets status",
    "git status | head -n 1",
    "cd crates && ls",
    "git status | head -n 1 && rm x",
    "cat src/main.rs",
];

/// The destructive patterns (spec 10) and the spellings spec 111 is about
/// (proposal §1, context.md "Gaps" and D8). `{root}` is the repository root.
const DESTRUCTIVE_COMMANDS: &[&str] = &[
    "rm -rf /",
    "rm -rf /tmp",
    "rm -rf /tmp/x",
    "rm -fr /",
    "rm -r -f /",
    "rm --recursive --force /",
    "rm -rf /tmp/..",
    "rm -rf ..",
    "rm -rf ~",
    "rm -rf .",
    "rm -rf ./",
    "rm -rf *",
    "rm -rf \".\"",
    "rm -rf '.'",
    "rm -rf $HOME",
    "rm -r /",
    "rm -rf target",
    "rm -rf {root}/target",
    "/bin/rm -rf /",
    "sudo rm -rf /",
    "X=1 rm -rf /",
    "echo $(rm -rf /)",
    "cargo fmt --check && cargo clippy || rm -rf /",
    "git reset --hard",
    "git reset --hard HEAD~1",
    "echo git reset --hard",
    "git clean -fd",
    "git clean -fdx",
    "git clean -d -f",
    "git clean -d --force",
    "git clean -f",
    "git clean -d --dry-run",
    "dd if=/dev/zero of=/dev/disk2 bs=1m",
    "dd if=image.iso of=/dev/rdisk4",
    "dd if=in.bin of=out.bin",
    "dd if=/dev/zero of=/dev/null count=1",
    "mkfs /dev/sda1",
    "sudo mkfs.ext4 /dev/sda1",
    "/sbin/mkfs /dev/sda1",
    "shutdown",
    "shutdown -h now",
    "reboot",
    "/sbin/reboot",
];

/// The legacy-key scenario's user config: entries from the existing tests
/// (`foundation.rs`, `plan_turn.rs`, `permission_profiles.rs`, …) and the
/// shapes the migration has to handle.
const USER_ALLOWLIST: &[&str] = &[
    "cargo test",
    "npm test",
    "./check",
    "ls src",
    "make",
    "docker ps",
    "cargo check --tests",
    "grep -rn nothing-matches src",
    "rm -rf target",
    "git status --short",
    "npm install",
    // The text patterns missed it, so it ran; the built-in default denies
    // it, and the migrated user `allow` overrides that, as before.
    "rm -r /tmp/cache",
    // Never allowed anything in the old policy, or not sayable as a rule:
    // not migrated.
    "echo $HOME",
    "ls *.rs",
    "rm -rf /tmp/cache",
    "git reset --hard",
    "git push --dry-run",
    "curl https://example.com",
    "grep 'a;b' notes.md",
];
const USER_BLOCKLIST: &[&str] = &["docker", "git push", "rm -rf /", "npm publish"];
/// Always allow grants for this repository, one of them for a sub-root.
const ALWAYS_ALLOW: &[&str] = &[
    "make test",
    "git log --oneline -n 5",
    "cd crates/engine && cargo test",
];
const REPOSITORY_BLOCKLIST: &[&str] = &["curl", "cargo publish"];

/// Commands that exercise the legacy keys: each entry, other spellings of
/// it, longer commands it is a prefix of, and the same commands where the
/// entry does not apply. Run at the root and in `crates/engine`.
const LEGACY_KEY_COMMANDS: &[&str] = &[
    "cargo test",
    "cargo  test",
    "cargo test --release",
    "cargo check --tests",
    "npm test",
    "npm install",
    "npm publish",
    "npm publish --dry-run",
    "./check",
    "ls src",
    "make",
    "make test",
    "make install",
    "docker ps",
    "docker run x",
    "dockerx",
    "grep -rn nothing-matches src",
    "grep -nr nothing-matches src",
    "rm -rf target",
    "rm -fr target",
    "git status --short",
    "git status -s",
    "git log --oneline -n 5",
    "git log -n 5 --oneline",
    "git push",
    "git push origin main",
    "git push --dry-run",
    "git pushx",
    "sudo git push",
    "echo hi && git push",
    "git push $REMOTE",
    "curl https://example.com",
    "curl -s https://example.com",
    "cargo publish",
    "cargo publish --dry-run",
    "echo $HOME",
    "ls *.rs",
    "rm -rf /tmp/cache",
    "grep 'a;b' notes.md",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Old {
    /// The blocklist or a destructive pattern blocked it.
    Blocked,
    /// The allowlist let it run.
    Allowed,
    /// Neither: the rest of the classifier decided, asking or not.
    Undecided { asks: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Difference {
    scenario: &'static str,
    /// Relative to the repository root, `""` for the root.
    directory: &'static str,
    command: String,
    old: Old,
    new: RuleVerdict,
}

/// Where a difference shows: the scenario and the directory, relative to
/// the repository root.
type Place = (&'static str, &'static str);
const ROOT: Place = ("defaults", "");
const LEGACY_ROOT: Place = ("legacy keys", "");
const LEGACY_SUB_ROOT: Place = ("legacy keys", "crates/engine");
const EVERYWHERE: &[Place] = &[ROOT, LEGACY_ROOT, LEGACY_SUB_ROOT];
const LEGACY: &[Place] = &[LEGACY_ROOT, LEGACY_SUB_ROOT];

/// One documented reason a decision changes, the commands it changes, and
/// where.
struct Known {
    why: &'static str,
    old: Old,
    new: RuleVerdict,
    places: &'static [Place],
    commands: &'static [&'static str],
}

const ASKED: Old = Old::Undecided { asks: true };

/// The documented differences: the fixes spec 111 makes on purpose
/// (proposal §1, context.md D8 and D9), and what the migration (D6) cannot
/// keep. Nothing here lets a command run that the old policy asked for,
/// except another spelling of an allowlisted command.
const KNOWN_DIFFERENCES: &[Known] = &[
    // --- Fixes: the built-in defaults over parsed commands -----------------
    Known {
        why: "options in any spelling, and any recursive `rm` of `/` or a parent of the \
              repository, are the built-in block (proposal §1, D8)",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: EVERYWHERE,
        commands: &[
            "rm -fr /",
            "rm -r -f /",
            "rm --recursive --force /",
            "rm -r /",
        ],
    },
    Known {
        why: "`..` of the root is a parent of the repository, a built-in block in any \
              spelling (D8); from a sub-root it is inside the repository",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: &[ROOT, LEGACY_ROOT],
        commands: &["rm -rf .."],
    },
    Known {
        why: "recursive `rm` outside the repository, with or without `-f`, is a built-in \
              block (D8); the allowlisted grant applies at the root only, as it did",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: &[LEGACY_SUB_ROOT],
        commands: &["rm -r /tmp/cache"],
    },
    Known {
        why: "a program named by its path is that program (Task 1)",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: EVERYWHERE,
        commands: &["/bin/rm -rf /", "/sbin/mkfs /dev/sda1", "/sbin/reboot"],
    },
    Known {
        why: "every part of a chain is checked, not only its start (D3)",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: EVERYWHERE,
        commands: &["cargo fmt --check && cargo clippy || rm -rf /"],
    },
    Known {
        why: "wrappers are unwrapped, so the defaults see the command they run (D9)",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: EVERYWHERE,
        commands: &["sudo rm -rf /"],
    },
    Known {
        why: "`git clean` with `-f` and `-d` in any spelling (D8)",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: EVERYWHERE,
        commands: &["git clean -d --force", "git clean -d -f"],
    },
    Known {
        why: "`mkfs` is matched as the program, not as text after a space (D8)",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: EVERYWHERE,
        commands: &["mkfs /dev/sda1"],
    },
    Known {
        why: "`shutdown` and `reboot` with any arguments, not only the bare word (D8)",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: EVERYWHERE,
        commands: &["shutdown -h now"],
    },
    Known {
        why: "`dd` is blocked only when it writes to a device, not for every `dd if=` (D8)",
        old: Old::Blocked,
        new: RuleVerdict::Default,
        places: EVERYWHERE,
        commands: &[
            "dd if=in.bin of=out.bin",
            "dd if=/dev/zero of=/dev/null count=1",
        ],
    },
    Known {
        why: "text inside another program's arguments is not the command (D8)",
        old: Old::Blocked,
        new: RuleVerdict::Default,
        places: EVERYWHERE,
        commands: &["echo git reset --hard"],
    },
    Known {
        why: "an absolute path strictly inside the repository is not a built-in block; the \
              relative spelling never was (D8)",
        old: Old::Blocked,
        new: RuleVerdict::Default,
        places: &[ROOT, LEGACY_SUB_ROOT],
        commands: &["rm -rf {root}/target"],
    },
    Known {
        why: "`.` below the root is strictly inside the repository, so not a built-in block \
              there (D8)",
        old: Old::Blocked,
        new: RuleVerdict::Default,
        places: &[LEGACY_SUB_ROOT],
        commands: &["rm -rf .", "rm -rf ./", "rm -rf \".\"", "rm -rf '.'"],
    },
    Known {
        why: "a glob cannot be analysed, so it asks instead of matching the text pattern (D8)",
        old: Old::Blocked,
        new: RuleVerdict::Ask,
        places: EVERYWHERE,
        commands: &["rm -rf *"],
    },
    // --- The migration of the old keys (D6) ---------------------------------
    Known {
        why: "another spelling of an allowlisted command is the same command: options as a \
              set, words without extra spaces, paths resolved",
        old: ASKED,
        new: RuleVerdict::Allow,
        places: LEGACY,
        commands: &["cargo  test"],
    },
    Known {
        why: "another spelling of an allowlisted command is the same command (options as a set)",
        old: ASKED,
        new: RuleVerdict::Allow,
        places: &[LEGACY_ROOT],
        commands: &["rm -fr target"],
    },
    Known {
        why: "the allowlisted `rm -rf target` with its path written out; the old text \
              pattern blocked the spelling, and the built-in default does not block it (D8)",
        old: Old::Blocked,
        new: RuleVerdict::Allow,
        places: &[LEGACY_ROOT],
        commands: &["rm -rf {root}/target"],
    },
    Known {
        why: "a blocklist prefix ends at a word, so `deny docker *` does not cover another \
              program `dockerx` (the old prefix matched text); these now ask as unknown programs",
        old: Old::Blocked,
        new: RuleVerdict::Default,
        places: LEGACY,
        commands: &["dockerx", "git pushx"],
    },
    Known {
        why: "a blocklist entry now denies the command wherever it appears: in any part of a \
              chain, and behind a wrapper (D3, D9)",
        old: ASKED,
        new: RuleVerdict::Deny,
        places: LEGACY,
        commands: &["echo hi && git push", "sudo git push"],
    },
    Known {
        why: "open for Task 4: a command that cannot be analysed asks and no rule applies to \
              it (D8), so a blocklisted command written with `$…` asks instead of being blocked",
        old: Old::Blocked,
        new: RuleVerdict::Ask,
        places: LEGACY,
        commands: &["git push $REMOTE"],
    },
    Known {
        why: "an allowlist entry no rule can say exactly is not migrated (`$HOME` cannot be \
              analysed; `*` would be a wildcard), so it asks; the deprecation note names it",
        old: Old::Allowed,
        new: RuleVerdict::Ask,
        places: &[LEGACY_ROOT],
        commands: &["echo $HOME", "ls *.rs"],
    },
];

struct Repository {
    root: PathBuf,
    data_dir: PathBuf,
}

impl Repository {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "damaian-rule-equivalence-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("project");
        for directory in [
            "crates/engine",
            "src",
            "secrets",
            "sub/secrets",
            ".claude/worktrees/x",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for (file, contents) in [
            ("README.md", "readme"),
            ("notes.md", "a\nb\n"),
            ("CHANGELOG.md", "# Changelog\n"),
            ("src/lib.rs", ""),
            ("secrets/token", "x"),
        ] {
            fs::write(root.join(file), contents).unwrap();
        }
        let data_dir = base.join("data");
        fs::create_dir_all(data_dir.join("config")).unwrap();
        Self { root, data_dir }
    }

    fn base(&self) -> PathBuf {
        self.root.parent().unwrap().to_path_buf()
    }

    /// Config with no legacy key set: only the built-in defaults decide.
    fn defaults(&self) -> (Config, RepositoryConfigReport) {
        self.load("", "")
    }

    /// Every legacy key in use, from user config, the Always allow grants
    /// and the repository's own config.
    fn legacy_keys(&self) -> (Config, RepositoryConfigReport) {
        let user = format!(
            "command_allowlist={}\ncommand_blocklist={}\ncommand_allowlist.{}={}\n",
            USER_ALLOWLIST.join("|"),
            USER_BLOCKLIST.join("|"),
            repository_id_for_root(&self.root),
            ALWAYS_ALLOW.join("|"),
        );
        let repository = format!(
            "command_blocklist={}\ncommand_allowlist=rm -rf /\n",
            REPOSITORY_BLOCKLIST.join("|")
        );
        self.load(&user, &repository)
    }

    fn load(&self, user: &str, repository: &str) -> (Config, RepositoryConfigReport) {
        let user_path = self.data_dir.join("config/user.conf");
        fs::write(&user_path, user).unwrap();
        let repository_path = Config::repository_config_path(&self.root);
        fs::create_dir_all(repository_path.parent().unwrap()).unwrap();
        fs::write(&repository_path, repository).unwrap();
        let base = Config {
            data_dir: self.data_dir.clone(),
            ..Config::default()
        };
        Config::load_scoped(
            base,
            Some(&user_path),
            Some(&repository_path),
            None,
            Some(&self.root),
        )
        .unwrap()
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.base());
    }
}

fn recorded_commands(root: &Path) -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/command_corpus/recorded_sessions.json");
    let commands: Vec<String> = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(
        commands.len() > 200,
        "the recorded set is the real-session half of the corpus"
    );
    commands
        .into_iter()
        .map(|command| command.replace(RECORDED_ROOT, &root.to_string_lossy()))
        .collect()
}

fn old_decision(policy: &CommandPolicy, command: &str, root: &Path, directory: &Path) -> Old {
    let classification = policy.classify_at(command, root, directory);
    let blocked_by_old_keys = classification.blocked
        && matches!(
            classification.causes.first(),
            Some(ApprovalCause::Blocklisted { .. } | ApprovalCause::Destructive { .. })
        );
    if blocked_by_old_keys {
        Old::Blocked
    } else if classification.expected_effects == "Configured safe command" {
        // The allowlist branch of `classify_text`, and only it, says this.
        Old::Allowed
    } else {
        Old::Undecided {
            asks: classification.requires_approval,
        }
    }
}

fn agrees(old: Old, new: RuleVerdict) -> bool {
    match (old, new) {
        (Old::Blocked, RuleVerdict::Deny)
        | (Old::Allowed, RuleVerdict::Allow)
        | (Old::Undecided { .. }, RuleVerdict::Default) => true,
        // Unanalysable: it asks under rules, so it agrees when it asked.
        (Old::Undecided { asks }, RuleVerdict::Ask) => asks,
        _ => false,
    }
}

fn differences(
    scenario: &'static str,
    repository: &Repository,
    (config, report): (Config, RepositoryConfigReport),
    commands: &[String],
    directories: &[&'static str],
) -> Vec<Difference> {
    let home = Path::new(HOME);
    let legacy = config.legacy_command_rules(&repository.root, Some(home), Some(&report));
    let rules: Vec<CommandRule> = legacy
        .rules
        .iter()
        .cloned()
        .chain(config.command_rules.iter().cloned())
        .collect();
    let policy = CommandPolicy::new_with_report(config, &report);
    let mut found = Vec::new();
    for &relative in directories {
        let directory = repository.root.join(relative);
        for command in commands {
            let old = old_decision(&policy, command, &repository.root, &directory);
            let new = evaluate_command(
                command,
                &rules,
                &RuleContext {
                    repository_root: &repository.root,
                    working_directory: &directory,
                    home: Some(home),
                },
            )
            .verdict();
            if !agrees(old, new) {
                found.push(Difference {
                    scenario,
                    directory: relative,
                    command: command.clone(),
                    old,
                    new,
                });
            }
        }
    }
    found
}

fn with_root(commands: &[&str], root: &Path) -> Vec<String> {
    commands
        .iter()
        .map(|command| command.replace("{root}", &root.to_string_lossy()))
        .collect()
}

/// D6's acceptance test: every old decision unchanged, except the listed
/// differences, and every listed difference still real.
#[test]
fn the_old_keys_and_the_migrated_rules_decide_the_same() {
    let repository = Repository::new("corpus");
    let root = repository.root.clone();
    let mut corpus = with_root(POLICY_TEST_COMMANDS, &root);
    corpus.extend(with_root(DESTRUCTIVE_COMMANDS, &root));
    corpus.extend(recorded_commands(&root));
    let mut legacy_commands = corpus.clone();
    legacy_commands.extend(with_root(LEGACY_KEY_COMMANDS, &root));
    for entry in USER_ALLOWLIST
        .iter()
        .chain(USER_BLOCKLIST)
        .chain(ALWAYS_ALLOW)
        .chain(REPOSITORY_BLOCKLIST)
    {
        legacy_commands.push((*entry).to_string());
    }

    let mut found = differences(
        "defaults",
        &repository,
        repository.defaults(),
        &corpus,
        &[""],
    );
    found.extend(differences(
        "legacy keys",
        &repository,
        repository.legacy_keys(),
        &legacy_commands,
        &["", "crates/engine"],
    ));
    let found: BTreeSet<Difference> = found.into_iter().collect();
    let root_text = root.to_string_lossy().into_owned();
    let root_text = root_text.as_str();
    let expected: BTreeSet<Difference> = KNOWN_DIFFERENCES
        .iter()
        .flat_map(|known| {
            known.places.iter().flat_map(move |&(scenario, directory)| {
                known.commands.iter().map(move |command| Difference {
                    scenario,
                    directory,
                    command: command.replace("{root}", root_text),
                    old: known.old,
                    new: known.new,
                })
            })
        })
        .collect();
    for known in KNOWN_DIFFERENCES {
        assert!(!known.why.is_empty() && !known.commands.is_empty());
    }
    let unexpected: Vec<_> = found.difference(&expected).collect();
    let missing: Vec<_> = expected.difference(&found).collect();
    assert!(
        unexpected.is_empty() && missing.is_empty(),
        "decisions that changed without a listed reason:\n{}\nlisted differences that no longer \
         occur:\n{}",
        format_differences(&unexpected),
        format_differences(&missing),
    );
}

fn format_differences(differences: &[&Difference]) -> String {
    differences
        .iter()
        .map(|difference| {
            format!(
                "    ({:?}, {:?}, {:?}, {:?}, RuleVerdict::{:?}),",
                difference.scenario,
                difference.directory,
                difference.command,
                difference.old,
                difference.new
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Each rule keeps the source of the scope that put its entry in force, so
/// the trust limits of D2 apply to it; the entries no rule can stand in for
/// are named, not dropped silently.
#[test]
fn migrated_rules_keep_their_source_and_directory() {
    let repository = Repository::new("sources");
    let (config, report) = repository.legacy_keys();
    let legacy =
        config.legacy_command_rules(&repository.root, Some(Path::new(HOME)), Some(&report));
    let root = repository.root.to_string_lossy().into_owned();
    let find = |effect: RuleEffect, pattern: &str| {
        legacy
            .rules
            .iter()
            .find(|rule| rule.effect == effect && rule.pattern == pattern)
            .unwrap_or_else(|| {
                panic!(
                    "no `{} {pattern}` rule in {:#?}",
                    effect.as_str(),
                    legacy.rules
                )
            })
    };

    let allow = find(RuleEffect::Allow, "cargo test");
    assert_eq!(allow.source, RuleSource::User);
    assert_eq!(allow.directory.as_deref(), Some(root.as_str()));
    // The old policy let it run, so it is migrated although the built-in
    // default would deny it: the user `allow` overrides, as the allowlist did.
    find(RuleEffect::Allow, "rm -r /tmp/cache");
    let always = find(RuleEffect::Allow, "make test");
    assert_eq!(always.source, RuleSource::User);
    assert_eq!(always.directory.as_deref(), Some(root.as_str()));
    let sub_root = legacy
        .rules
        .iter()
        .find(|rule| {
            rule.pattern == "cargo test"
                && rule.directory.as_deref() == Some(&format!("{root}/crates/engine"))
        })
        .expect("the sub-root grant is a rule for that directory");
    assert_eq!(sub_root.source, RuleSource::User);

    assert_eq!(
        find(RuleEffect::Deny, "git push *").source,
        RuleSource::User
    );
    assert_eq!(
        find(RuleEffect::Deny, "curl *").source,
        RuleSource::Repository
    );
    assert_eq!(
        find(RuleEffect::Deny, "cargo publish *").source,
        RuleSource::Repository
    );
    // The repository's own `command_allowlist` was never applied (spec 34),
    // so it migrates into nothing.
    assert!(
        !legacy
            .rules
            .iter()
            .any(|rule| rule.effect == RuleEffect::Allow && rule.pattern == "rm -rf /")
    );
    // Allows first, then denies: within one source the deny wins, as the
    // blocklist came before the allowlist.
    let first_deny = legacy
        .rules
        .iter()
        .position(|rule| rule.effect == RuleEffect::Deny)
        .unwrap();
    assert!(
        legacy.rules[first_deny..]
            .iter()
            .all(|rule| rule.effect == RuleEffect::Deny)
    );

    let unmigrated: Vec<(&str, &str)> = legacy
        .unmigrated
        .iter()
        .map(|entry| (entry.key, entry.entry.as_str()))
        .collect();
    assert_eq!(
        unmigrated,
        vec![
            ("command_allowlist", "docker ps"),
            ("command_allowlist", "echo $HOME"),
            ("command_allowlist", "ls *.rs"),
            ("command_allowlist", "rm -rf /tmp/cache"),
            ("command_allowlist", "git reset --hard"),
            ("command_allowlist", "git push --dry-run"),
            ("command_allowlist", "curl https://example.com"),
            ("command_allowlist", "grep 'a;b' notes.md"),
        ]
    );
    for (entry, reason) in [
        ("docker ps", "`command_blocklist` entry `docker`"),
        ("echo $HOME", "shell expansion"),
        ("ls *.rs", "wildcard"),
        ("rm -rf /tmp/cache", "`command_blocklist` entry `rm -rf /`"),
        ("git reset --hard", "Damaian blocks it (`git reset --hard`)"),
        ("git push --dry-run", "`command_blocklist` entry `git push`"),
        (
            "curl https://example.com",
            "`command_blocklist` entry `curl`",
        ),
        ("grep 'a;b' notes.md", "shell control"),
    ] {
        let found = legacy
            .unmigrated
            .iter()
            .find(|unmigrated| unmigrated.entry == entry)
            .unwrap();
        assert!(found.reason.contains(reason), "`{entry}`: {}", found.reason);
    }
}

/// The deprecation note (D6) names each old key in use and each entry that
/// did not become a rule; with no old key in use it says nothing.
#[test]
fn the_old_keys_carry_a_deprecation_note() {
    let repository = Repository::new("notes");
    let (config, report) = repository.defaults();
    let none = config.legacy_command_rules(&repository.root, Some(Path::new(HOME)), Some(&report));
    assert_eq!(none, LegacyCommandRules::default());
    assert!(none.deprecation_notes().is_empty());

    let (config, report) = repository.legacy_keys();
    let legacy =
        config.legacy_command_rules(&repository.root, Some(Path::new(HOME)), Some(&report));
    assert_eq!(legacy.allowlist_entries, USER_ALLOWLIST.len());
    assert_eq!(legacy.always_allow_entries, ALWAYS_ALLOW.len());
    assert_eq!(
        legacy.blocklist_entries,
        USER_BLOCKLIST.len() + REPOSITORY_BLOCKLIST.len()
    );
    let notes = legacy.deprecation_notes();
    for key in [
        "`command_allowlist` is deprecated",
        "`command_allowlist.<repository_id>` is deprecated",
        "`command_blocklist` is deprecated",
        "`command_allowlist` entry `ls *.rs` is not read as a rule",
    ] {
        assert!(
            notes.iter().any(|note| note.contains(key)),
            "{key}: {notes:#?}"
        );
    }
    assert_eq!(notes.len(), 3 + legacy.unmigrated.len());
}

/// Without a report the source of an entry is unknown; it is attributed to
/// the user, whose rules are final either way, and nothing else changes.
#[test]
fn without_a_report_every_entry_is_the_users() {
    let repository = Repository::new("no-report");
    let config = Config {
        command_allowlist: vec!["cargo test".to_string()],
        command_blocklist: vec!["curl".to_string()],
        ..Config::default()
    };
    let legacy = config.legacy_command_rules(&repository.root, Some(Path::new(HOME)), None);
    assert!(
        legacy
            .rules
            .iter()
            .all(|rule| rule.source == RuleSource::User)
    );
    assert_eq!(
        legacy
            .rules
            .iter()
            .map(CommandRule::text)
            .collect::<Vec<_>>(),
        vec![
            format!("allow cargo test in {}", repository.root.display()),
            "deny curl *".to_string(),
        ]
    );
}
