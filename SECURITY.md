# Security Policy

Damaian is a developer preview. It is designed to keep repository access, file writes, command execution, secret redaction, and audit logging under local application control.

## Supported Versions

Security fixes target the current `main` branch and ship in the next release. Only the latest release is supported. Release builds are Developer ID signed and notarized; locally built DMGs are ad-hoc signed.

## Reporting a Vulnerability

If you find a security issue, avoid posting exploit details, API keys, repository contents, or private user data in a public issue.

Report it privately through GitHub: open the repository's **Security** tab and select **Report a vulnerability**, or go straight to <https://github.com/damijanc/damaian/security/advisories/new>. Only the maintainers can see the report. Please do not open a public issue for a security problem.

Useful reports include:

- affected version or commit
- operating system and architecture
- impact and expected behavior
- minimal reproduction steps that do not include secrets

## Secret Handling

Do not commit real API keys, provider tokens, certificates, private keys, or local `.damaian` data. Keep real values out of examples, logs, screenshots, and test fixtures.

Damaian supports macOS Keychain-backed model API key storage from desktop settings. Config files should store Keychain references such as `keychain:model-api-key` or environment variable names for CLI/development workflows, never raw provider keys.

Secret scanning redacts detected credentials from indexed context, the user's prompt, earlier conversation turns replayed into a request, command output, patch diffs, Git diff output, audit log fields, and model-visible command results. Session messages are stored as typed, so a prompt and the replayed turns are scanned on the way into every request rather than at storage time; a credential typed once into a message is re-sent redacted on every later turn, not in cleartext. Generated file content is still checked before apply and is blocked by default when hardcoded secrets are detected.

That block is a warning the user can overrule, not a hard stop. When a selected file is flagged, the apply reports which file tripped the check and which detection categories fired — never the matched value — and applies nothing. The user reviews the diff and may then apply anyway (`Apply Anyway` in the desktop app, `--allow-generated-secrets` in the CLI). The override is per-apply, is never inferred or remembered, and is recorded in the audit log as `generatedSecretOverride=true`.

Assignment-shaped detection (`password=`, `api_key:`, database URL credentials) exempts values that are structurally not credentials: template and variable syntax (`<your-key>`, `${DB_PASSWORD}`, `{{ vault_password }}`, `$OPENAI_KEY`), Keychain references, repeated filler (`xxxxxxxx`), environment variable names, and values naming themselves as examples. Setup documentation is the dominant source of these, and blocking or redacting them protected nothing while making generated READMEs unappliable. Structural detectors — private keys, AWS keys, JWTs, provider token prefixes, Azure keys, custom patterns — are not exempted.

## Local App Boundary

The desktop shell binds to loopback on the fixed app origin `http://127.0.0.1:4765`. Startup refuses to continue if that port is already occupied. The Tauri capability admits only that origin, as `http://127.0.0.1:4765` and `http://localhost:4765`, and only the `main` window. It grants the bootstrap command, the dialog and updater plugins, and the terminal panel's commands (`terminal_open`, `terminal_write`, `terminal_resize`, `terminal_close`), so the terminal panel is reachable from that origin and from nowhere else.

The desktop API token is never served over HTTP. It is delivered to the webview through a Tauri IPC command (`damaian_desktop_bootstrap`), which only the app's own webview process can invoke, so no local HTTP request against the shell server can retrieve it. Local `/api/*` requests require that token.

Model provider requests use `curl --config -` so the provider API key is passed through the child process stdin configuration, not as a command-line argument.

Patch rollback snapshots and session checkpoints store pre-edit file content unredacted, under `<data_dir>/rollback/` and the checkpoint store. That is deliberate: redaction covers what leaves your control (model context, command output, diffs, the audit log), and a snapshot is a local copy kept so a file can be put back exactly. Redacting it would corrupt the restore. Treat the data directory as sensitive and do not share it.

## Command Execution

Damaian classifies every command before it runs, and the classification, not the model, decides whether you are asked:

- **Runs without asking:** commands Damaian recognises as read-only, such as `pwd`, `ls`, `git status` and `git log` with listed options, and simple pipelines of them filtered through `head`, `tail`, `wc`, `sort`, `uniq`, `cut` or `grep`. They run in your login shell in the working folder, which is why the admitted set is a closed list of programs and options rather than a heuristic.
- **Asks first:** everything else, including anything that writes, installs, reaches the network or uses shell control syntax. `git diff` and `git show` ask too, because they print file contents a command line cannot bound or keep restricted paths out of; the agent's `read_git_diff` tool does both. Every Docker command asks, `docker ps` included, because the daemon is privileged.
- **Blocked:** commands matching `command_blocklist` or a destructive pattern are refused and cannot be approved from the UI.

An approval's first line names the deciding cause — which shell-control construct, network keyword, path, setting or profile block — rather than a generic sentence, so the prompt can be judged instead of clicked through. The card shows that cause beside the risk badge and keeps every recorded reason in **Why this command**; the model's own stated purpose appears separately as *The agent says: …*. The same causes are recorded on the stored proposal and in the audit log.

The read-only boundary is a tokenizer, not a prefix match. A command runs
without asking only when a shell tokenizer splits it into words with no
expansion — no `$`, no backtick, no unquoted `*`, `?`, `[`, `{` or `~` — every
program and option is from the closed list above, and every path argument
resolves inside the repository and clear of `restricted_patterns`. What is
checked is exactly what the login shell will run; anything the shell would
rewrite after classification asks for approval. A path outside the repository,
behind a symlink, or inside a restricted directory such as `secrets/` asks, and
so does a restricted working directory.

Two things the boundary deliberately does not cover:

- **Allowlisted commands.** *Allow Always* records one exact command string the
  user approved, and it keeps running without these checks. The allowlist is a
  user decision about a specific command, not a claim that the command is
  read-only; whether it should be held to the boundary is a separate question,
  not this one.
- **`ls` listing restricted names.** A path argument is checked, but `ls` does
  not police the names it lists the way `list_directory` does. `ls secrets/`
  asks, because the path is restricted; `ls` of an admitted directory still
  lists a restricted entry nested inside it by name. The `list_directory` tool
  is the answer for a listing that must hide restricted names.

Commands find programs on the `PATH` of your interactive login shell (spec 69).
Damaian starts that shell once per run, in your home folder rather than a
repository, with stdin closed and a five-second limit, and keeps only `PATH`
from it, dropping empty and relative entries so no command resolves a program
against its working directory. Aliases, functions and other variables are not
taken, so the command the boundary above judged is the command that runs. The
shell and this setting are user and admin configuration; a repository or a
profile cannot change either.

`require_approval_for_all_commands` makes every command ask. *Allow Always* records one exact command for one checkout in user config (see the trust boundary below) and is not offered for blocked commands or commands with shell control syntax. A working mode or permission profile can only narrow what runs. Output of every command is redacted before the model or the UI sees it, and each command runs under `command_timeout_secs` and is registered in the process registry. A command left running by a crash is found by the sweep at the next launch and killed by its recorded pid, only while its recorded start time still matches, and never by name.

## Hooks

A hook ([user guide](docs/USER_GUIDE.md#hooks)) is an executable you, an
administrator, or a reviewed repository configured to run at one of nine
lifecycle events. It is **user-trusted code running outside the working mode
and the permission profile**: it runs as your user, in every mode, and its own
process is not classified by the command policy. Only what it returns, and what
it asks Damaian to run, are constrained:

- **There is no widening verdict.** A hook can answer `allow`, `warn`,
  `request_approval` or `deny`, and nothing else. No verdict grants a
  permission, re-enables a mode- or profile-refused action, or turns an
  approval-gated action automatic. A hook cannot edit a patch, a command or a
  context item. A repository cannot add a widening variant to the enum.
- **A requested command is an agent action.** The `commands` field runs under
  `profile ∩ mode`, through the same classification and `permits` check as a
  turn's command, and only when policy would run it unasked. A command that
  needs approval, is blocked, or is refused by the profile or Ask mode is not
  run, and no approval card is raised. A hook therefore cannot use a command
  request to put a model-independent prompt in front of the user.
- **The environment is cleared.** A hook gets `PATH`, `HOME`, `USER`, `LANG`
  and `TMPDIR` if set, plus `DAMAIAN_HOOK_ID`, `DAMAIAN_HOOK_EVENT` and
  `DAMAIAN_REPOSITORY_ROOT`; never the model API key, an MCP token, or
  anything else Damaian inherited. It runs in the repository root. Its stdin
  is the event JSON and is then closed.
- **Repository hooks require review at a hash.** Hook configuration is a
  capability key. A repository may define a hook, but it arrives pending and
  does not run until you approve its current definition hash, computed over
  the definition fields and the content of every command or argument that
  resolves to a file inside the repository. The review is stored in your user
  config; the repository cannot record one. A later change to the definition
  or the script makes the hook `changed_since_review`, and it stops running
  rather than executing code you did not approve. A repository can switch off
  a hook it defined, and can never change, disable, retime or make optional a
  hook your config or the admin config defines. A permission profile cannot
  carry a hook.
- **Failures fail closed.** `mode` defaults to `mandatory`. A spawn failure, a
  non-zero exit, a timeout, malformed output, an unknown `hookApiVersion`, an
  oversized response, an unknown field or verdict, or a changed definition
  blocks a mandatory hook's action. An `optional` hook's failure warns and the
  action proceeds, explicitly and per hook.
- **Output is bounded and redacted.** stdout is read to 64 KiB; exceeding the
  ceiling is a failure, not a truncation. `reason`, `findings` and `context`
  pass through `SecretScanner` before display, persistence or entry into
  context, and findings use `FindingSource::Hook` so they are bounded and
  redacted by the same constructor as every other finding. stderr is kept to
  its last 8 KiB, redacted, and never sent to the model.
- **Processes are tracked and killed.** A hook runs in its own process group
  and is registered in the PID registry. A timeout kills the group, and a hook
  left running by a crash is found by the next launch's sweep, by recorded pid
  and start time, never by name.
- **Nesting is bounded.** A requested command dispatches `before_tool` and
  `after_tool` at depth + 1 with the requesting hook excluded, so a hook never
  sees its own command. Past depth 2 the nested dispatch is skipped, the
  command still runs, and a `hook_depth_exceeded` audit record and warning
  finding say so.
- **Deleting a session runs `session_end` only for its own checkout.** The
  delete request names the repository; a root that does not match the
  session's repository id is refused before any hook runs.

Every invocation and its outcome is audited as `hook_invoked`, with failure
kinds distinguished; widening attempts, depth overruns and refused requested
commands have their own event types, and reviews are recorded as
`hook_reviewed`.

## File Access

Reads and writes go through one path policy:

- A path that resolves outside the selected repository is refused, for reads and writes alike. For writes, every existing component of the target is checked, so a symlink inside the repository that points outside it cannot be written through. Additional directories are reachable only through `allowed_roots`, which a repository cannot set.
- `restricted_patterns` are refused to the agent's read tools and excluded from search, context and git reads. The defaults cover `.env` and `.env.*`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, SSH private key names (`id_rsa`, `id_dsa`, `id_ecdsa`, `id_ed25519`), and `secrets/` and `credentials/` directories at any depth. A repository may add patterns, never remove them.

## Patch Application

The model never writes files. An edit comes back as a patch preview, and nothing is written until you apply it. Each file in a patch records the hash of the content it was generated against; at apply time a file whose current hash differs is blocked rather than overwritten, so a patch approved earlier cannot silently replace changes made after the preview. Generated content is scanned for hardcoded secrets before apply, as described above.

## Audit Log

Security-relevant actions — commands run or refused, patches applied or rejected, approvals, hook invocations and reviews, rejected repository config keys, page fetches (`web_fetch`), redirects (`web_fetch_redirect`), host decisions (`web_fetch_approval_decision`), worker spawns and terminations — are appended to `<data_dir>/audit/events.jsonl` (by default under `~/Library/Application Support/DamaianClient/`). Fields are redacted before they are written, and attacker-controlled values such as a rejected repository key's value are never recorded. Files older than `audit_retention_days` are pruned. The log stays on your machine; nothing sends it anywhere.

## Data That Leaves Your Machine

Prompts and the context Damaian assembles go to the model provider you configured, and only there. Detected credentials in the prompt, in the replayed earlier turns, and in every context item are redacted first (see [Secret Handling](#secret-handling)). These cases can reach somewhere else, and each needs you:

- **Provider fallback.** When a provider refuses a turn and a second qualifying provider is configured, the desktop app offers to answer that one turn with it. The offer is refused unless you accept it, it never becomes permanent, and it covers only the turn it was offered for.
- **Browser diagnostics on a remote URL.** Inspecting a local page (`localhost`, `127.0.0.1`, `[::1]`) runs without asking. Any page outside your machine asks every time and shows the origin it will reach; the per-session grant never covers a remote URL.
- **Reading a page (`fetch_url`).** The model names one absolute `http`/`https` URL, and Damaian makes one GET to it. The request carries no `Authorization` header, no cookie and no Damaian credential; the transport has no credential field, so one cannot be passed. Redirects are never followed — a 3xx is returned to the model as text naming the target, and fetching it is a new call with its own approval. A host is read only after you allow it in the desktop app: *Allow once*, *Allow always* (which writes `web_domain_allowlist.<repository id>` to **user** config) or *Don't fetch*. The dialog names the host and shows the URL verbatim, and breaks out an over-long query or fragment. Repository and permission-profile config cannot add a host; both keys are forbidden and a repository attempt is audited by key and class, never by value. Before any dialog is shown, a URL whose text looks like a credential, or whose query/fragment contains a line from a file the agent read with `read_file` in this task, is refused outright; raw, percent-decoded and base64-decoded values are checked. Responses are capped, and only `text/*`, `application/json` and `application/xhtml+xml` are converted — anything else returns its type and size and no bytes. The body is redacted before it is cached, cached per session, and never logged. The text reaches the model as quoted material in the lowest-priority context category, never in the system prompt and never as an instruction; every later turn re-renders the same notice, `> ` quoting, injection markers and end marker, so page text cannot fabricate a prompt section. A host is resolved before the request and every answer must be a public internet address: loopback, private, link-local (including `169.254.169.254`), carrier-grade-NAT, reserved and IPv4-mapped forms are refused, and the connection is pinned to the checked address with curl `--resolve`, so a DNS answer cannot rebind between approval and request. IPv4 is accepted only as canonical dotted-quad, so `2130706433`, `0x7f000001` and `127.1` cannot display as one host and connect to another. The child runs with `--noproxy '*'` and the proxy environment variables removed, so an environment proxy cannot see or redirect a fetch. The loopback allowance used by the evaluation stub is a test constructor, not a config key: no repository, profile or user setting reaches it. A URL is therefore an outbound channel: what leaves is one GET to an approved, public host, and an approval is never the last line of defence against a credential in it.
- **A language server.** A configured `language_server.<language>` is a local program you named in your own config, started with the repository as its working directory. Damaian sends it the content of files it already has on disk, and scans everything it returns before storing or showing it. It is not a network endpoint, and a repository cannot name one (see [Repository Config Trust Boundary](#repository-config-trust-boundary)).

The provider API key is stored in the macOS Keychain under the service `DamaianClient`, or read from an environment variable you name. Config holds only a reference to it (`keychain:<account>` or the variable's name), never the key itself.

A loopback provider such as Ollama (local) may run without a key. When the reference resolves to nothing on a loopback endpoint, Damaian sends no `Authorization` header instead of refusing the request. A missing key is tolerated only for a loopback endpoint; every non-loopback endpoint still requires one.

## Git Read Hardening

Every git read — `read_git_status`, `read_git_diff`, `read_git_log` and
`list_worktrees`, in a worktree or the main checkout — runs through one hardened
runner (`git_service.rs`), because the same function serves the model and the
CLI and the gaps were the same on both sides. Four properties hold, and the CLI
and desktop shell inherit them because they call the same functions:

- **Restricted paths are excluded.** `status` and `diff` judge each changed path
  with `PathPolicy::is_restricted` — the same matcher `read_file` refuses by —
  and drop it, or any path below a restricted directory. A rename is dropped when
  either name is restricted, and a status whose only changes are restricted reads
  as clean. A restricted path's name or changed lines therefore never reach the
  model through a git read.
- **Output is bounded.** `status`, `diff` and `log` are cut to `max_read_lines`
  and `max_file_bytes`, whichever binds first, and the cut result ends in a line
  saying how much was cut. Output is redacted before it is cut, so a cut cannot
  leave a secret's prefix the scanner no longer recognises.
- **No repository-supplied code runs.** Every read passes
  `-c core.fsmonitor=false`, and `diff` and `log` pass `--no-ext-diff
  --no-textconv`. `.gitattributes` is repository content and can select a diff
  driver, a textconv driver or an fsmonitor hook by name; those flags stop all
  three. (Clean/smudge filters are not disabled: git has no flag for them, and
  turning them off by name would make git-lfs or git-crypt files read as
  modified. That gap is recorded in spec 60 §7.)
- **A read takes no lock, and arguments cannot become options.** Every read
  passes `--no-optional-locks` and `-c diff.autoRefreshIndex=false`, so a read
  cannot take `index.lock` from another agent working in the same checkout.
  `base` is validated against git's ref-name rules and passed after
  `--end-of-options`; `paths` are resolved through `PathPolicy` against the
  checkout and passed after `--`, so neither can be read as an option.

## Git Commits

Committing is the only operation that writes to the user's Git repository, and
it is treated as a capability rather than a convenience:

- **A typed, per-commit approval.** A commit request carries its own approval
  field and is audited as `commit_created`; a command approval, a plan approval
  or an answer to a question never satisfies it. There is no persistent
  "always allow" for committing.
- **Code mode and a profile that permits it.** Committing is refused in Ask,
  Plan and Review, and whenever the resolved `allow_git_commits` capability is
  false. The built-in **Read-only** profile turns it off. Repository config may
  turn it off and may never turn it on, like every other capability key.
- **The user's index is not the commit's base.** The tree is composed through a
  private `GIT_INDEX_FILE` under the data directory, verified with `ls-tree`
  before `commit-tree`, and the branch moves with an `update-ref` compare-and-swap
  against the `HEAD` that was reviewed. A failure before the ref moves leaves
  the user's index and working tree untouched. After a success, each committed
  path's entry in the real index is rewritten to the new `HEAD`, only where it
  still equals the old `HEAD`'s entry — so staged work at another path, or a
  different staged version of that path, is never overwritten. Leaving those
  entries stale would make the user's next plain `git commit` revert the commit.
- **The preview is redacted; the commit is not.** The displayed diff goes
  through `SecretScanner`. Before the commit, the scanner runs on the committed
  blob itself — hashed into a throwaway object directory and read back, so a
  clean or end-of-line filter cannot make the scanned bytes differ from the
  committed bytes. A finding is reported as categories and counts
  — never matched text — and requires a separate per-commit override that is
  audited. Files too large to scan are treated as unscanned and need the same
  override. The prepared record stores each candidate's exact blob id, so a
  file that changed after review refuses the commit.
- **Hooks are not bypassed.** `commit-tree` runs no hooks, so `pre-commit`,
  `prepare-commit-msg`, `commit-msg` and `post-commit` are executed explicitly
  in Git's order, with the private index in their environment, through the
  command path's timeout, output truncation, redaction and process registry.
  There is no `--no-verify` and no setting that adds one. A hook that rewrites
  the message must be approved again before the commit object is created.
- **Restricted paths stay out.** Candidates come from a status read that applies
  `PathPolicy`, and the commit re-checks every selected path for containment
  and restriction before composing the tree.
- **A commit is a side-effecting action.** It is bracketed by #17's markers with
  `sideEffecting: true`; a crash between them classifies as an unknown external
  outcome and recovery offers inspection, never an automatic re-commit.

## Git Branches

Branch creation, switching and deletion are Git mutations and therefore separate
capabilities from committing (spec 36):

- **One typed approval per operation.** `GitMutation` has one variant per
  operation — `CreateBranch`, `SwitchBranch`, `DeleteBranch` — and no catch-all,
  so an approval can never be reused across operations. The route requires
  `approval=branch`, the preparing `session_id`, and the exact prepared mutation;
  a commit approval, a command approval or an answer to a question never
  satisfies it. The durable record is marked `applying` before Git runs and
  consumed once, so a repeated or concurrent request is refused. There is no
  “always allow”.
- **Code mode and a profile that permits it.** Every prepare and every apply is
  refused in Ask, Plan and Review, and whenever the resolved
  `allow_git_branches` is false. The built-in **Read-only** profile turns it off.
  Repository config may turn it off and may never turn it on, like every other
  capability key.
- **No force, merge, rebase or cherry-pick is executable here.** Creation runs
  `git branch <name> <resolved-sha>`; switching runs
  `git switch --no-guess --no-overwrite-ignore`, so an ignored local file that
  the target branch tracks is never silently replaced; deletion runs
  `git branch -d`, or `-D` only after the separate confirmation that names the
  unmerged count and subjects. No command this feature constructs contains `-f`
  or `--force`. Merge and rebase preparation run only
  `git merge-tree --write-tree --name-only` and reads, which touch neither the
  index, the working tree nor `HEAD`; there is no real-merge fallback and no
  route that executes a merge, rebase or cherry-pick.
- **Names and commit subjects come from redacted text.** A suggested name is
  derived from the session title or request only after `SecretScanner`
  redaction, and Git — `git check-ref-format --branch` — is authoritative about
  the final name. A collision is reported with a free alternative rather than
  silently changed. Merge, rebase and deletion reports scan each commit subject
  before it reaches the dialog.
- **Restricted paths stay out of reports.** Conflicted, changed and
  dirty-switch path lists pass through `PathPolicy`, so a restricted file such
  as `.env` is never named in a merge report or a refusal.
- **A mutation is a side-effecting action.** It is bracketed by #17's markers
  with `sideEffecting: true` and audited with the refs and outcome, never commit
  bodies. Recovery compares the recorded ref with the repository now and never
  retries.

## Repository Config Trust Boundary

Configuration comes from three files, applied in order: user (`<data_dir>/config/user.conf`), repository (`<repo>/.damaian/config.conf`), and admin (`DAMAIAN_ADMIN_CONFIG` or `<data_dir>/config/admin.conf`). Only the repository file arrives with a clone, so **repository config is untrusted input**: it may add restrictions and never remove one.

A repository cannot set `shell`, `command_path_from_interactive_shell`, `data_dir`, `allowed_roots`, `secret_patterns`, `audit_enabled`, `block_generated_secrets`, `enable_worker_sessions`, `checkpoint_retention_days`, `checkpoint_max_total_bytes`, `checkpoint_census_max_paths`, `web_domain_allowlist` or any `web_domain_allowlist.<repository id>` entry, any `language_server.<language>` command, or any `model_*` key including a `model_provider.<id>` entry. Those keys redirect where commands run, which programs start unprompted, where model traffic and the API key reference go, where data is written or how long it is kept, or which defences are active; a repository has no legitimate use for any of them. They are ignored, recorded in the audit log as `repository_config_key_rejected` with the key name and class — never the value, which is attacker-controlled text — and reported to the user once per repository. There is deliberately no override.

`restricted_patterns`, `ignore_patterns`, and `command_blocklist` are unioned with the user's, so a repository may add a restriction but never drop one. `require_approval_for_file_edits`, `require_approval_for_risky_commands`, and `require_approval_for_all_commands` may be turned on by a repository, never off. `mcp_enabled` and a server's `enabled` may be turned off, never on, and `mcp_server_allowlist` may only be narrowed. A repository may *define* an MCP server — a useful suggestion — but the definition is created disabled and approval-gated, and it cannot redefine a server the user has already configured. The same holds for hooks: a repository may define a hook of its own, but it arrives pending review at a definition hash, cannot record its own review, and cannot change a hook the user or admin config already defines.

Limits on how much one tool call, command or turn may read, run or spend may be lowered by a repository and never raised: `command_timeout_secs`, `max_read_lines`, `max_list_entries`, `max_search_matches`, `max_match_line_chars`, `repository_map_max_tokens`, `agent_max_task_tokens`, `agent_max_turn_messages`, `agent_max_exploration_rounds`, `agent_max_turn_notes_chars`, `allow_git_commits`, and `allow_git_branches`. Raising one would widen what leaves your machine or how long a command may run.

`command_allowlist` is never taken from repository config. An `Allow Always` decision is the user's decision about a repository, so it is stored in user config as `command_allowlist.<repository_id>`, keyed per checkout. The repository's own file cannot grant its commands no-approval execution, and a `command_allowlist` already present in a repository file is offered once for itemised keep-or-discard rather than honoured. `web_domain_allowlist` follows the same rule for the same reason: *Allow always* on a fetch writes `web_domain_allowlist.<repository_id>` to user config, and a repository cannot pre-approve a host its own content names.

Admin config keeps its ability to both widen and narrow: it is a local file, not something a clone carries.

## Worker Sessions

A worker ([spec 57](docs/specs/57_worker_sessions/proposal.md)) is the app's own executable re-launched as `--worker`. It runs turns through the same engine entry points as the shell, under the same command policy, redaction, approvals, working mode and audit log. Nothing in those is reimplemented for it. It is off unless the user turns on `enable_worker_sessions`. That key is in the repository-forbidden list above because turning it on adds a paragraph to every session's system prompt, and a repository must not be able to do that.

**The API key never leaves memory.** The shell takes the key from its own in-memory cache, so spawning a worker never prompts the Keychain. It writes the key to the worker's stdin as one JSON line together with a control token, and nothing else carries either. Neither is placed in the worker's environment or argv or written to disk, so no command the worker runs can read them from its environment. The worker refuses to start if stdin is a terminal, keeps the key only inside its model adapter, and keeps stdin open as its lifeline (below). Tests check that a command a worker runs sees neither value in its environment and that neither appears in the worker's argv or output.

**The control socket.** The shell controls a worker over a Unix domain socket, `<data_dir>/workers/<16 hex of sha256(session id)>.sock`, with mode `0600` in a directory with mode `0700`. A symlinked or non-directory `workers/` is refused. Every request must carry the 256-bit control token from the handover, compared in constant time. A missing or wrong token, a malformed or oversized line (256 KiB), an unknown command or an unexpected argument closes the connection or is refused. Steering notes are capped at 16 KiB. The socket accepts one connection at a time and closes it after 5 idle seconds. The socket exposes only the commands the UI offers: message, note, pause, resume, approve, reject, stop, shutdown and status.

**Nothing survives the application.** The shell registers each worker in the process registry (spec 46) and holds the write end of its stdin. When the shell exits for any reason, `SIGKILL` included, the kernel closes that pipe, and the worker cancels its turn and terminates every process it registered before exiting. That does not depend on the shell running any cleanup code. An end-to-end test `SIGKILL`s a shell whose worker is running a command and checks that both are gone within 5 seconds. Every signal a shell sends a worker goes to the registered pid only while its recorded start time still matches, and is audited as `worker_terminated`. Nothing is killed by name.

**`.damaian/worktrees` is checked as untrusted.** `.damaian/` arrives with a clone, so before creating `<repo>/.damaian/worktrees/<slug>` Damaian refuses if any component of that path is a symlink, if the path exists, if Git tracks anything under it, or if it resolves outside the repository. All of these checks run before the first change. Removing a worktree is refused unless it is a registered worktree directly under that directory, reached through no symlink, and its uncommitted changes are discarded only after a second confirmation that names them. The branch is never deleted. A repository cannot start a worker, choose where it goes, or skip the user's click. Clicking *Start worker* is the approval for the Git commands, which are recorded exactly in the audit log (`worker_spawned`).

## Permission Profiles Narrow Only

A permission profile (built in, or a custom file under `<data_dir>/config/profiles/`) is applied after user, repository and admin config, and only ever narrows the result. A profile value that would loosen what is already in force has no effect. A profile cannot carry the redirecting keys (`shell`, `data_dir`, `allowed_roots`, `secret_patterns`, `audit_enabled`, `block_generated_secrets`, any `language_server.<language>`, any `model_*`), `command_allowlist`, `web_domain_allowlist`, or an MCP server definition; such keys are ignored and audited by key and class, never by value. The selection is the user's, stored in user config per checkout, so a repository cannot choose its own profile. Admin config cannot undo a narrower profile the user picked. The capability a session has is the profile intersected with the session's working mode, checked at every refusal point, and a command the profile blocks is also blocked in the command policy, so a stored proposal run by id is re-classified and refused.

An exported profile contains only keys a profile may carry, so it never names a credential reference (`model_api_key_env`, an MCP server's `auth_token_env`). An imported profile is untrusted input: keys a profile cannot carry are not written, keys that would loosen are listed and, like any profile key, cannot widen anything, and importing never selects the profile.

Two limits are deliberate and documented rather than hidden:

- `command_access=local` means no command Damaian recognises as networked. That is a name heuristic, not a security boundary: a command Damaian does not recognise, such as `python -c` with `urllib`, can still reach the network.
- The Offline private profile does not stop model traffic. Prompts and code in context still go to the configured model provider, and `model_base_url` is not something a profile controls. The settings view warns when that address is not loopback.

The same rule covers the other untrusted repository-supplied inputs. `AGENTS.md` instructions cannot widen a working mode or grant capability, and an MCP server descriptor's tool names and descriptions are data, not instructions.
