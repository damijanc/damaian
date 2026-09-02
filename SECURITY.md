# Security Policy

Damaian is a developer preview. It is designed to keep repository access, file writes, command execution, secret redaction, and audit logging under local application control.

## Supported Versions

Security fixes target the current `main` branch. Published preview builds should be treated as experimental until Developer ID signing and notarization are in place.

## Reporting a Vulnerability

If you find a security issue, avoid posting exploit details, API keys, repository contents, or private user data in a public issue.

Use GitHub private vulnerability reporting if it is enabled for the repository. If private reporting is not available, open a public issue with a short, non-sensitive summary and offer to share details privately.

Useful reports include:

- affected version or commit
- operating system and architecture
- impact and expected behavior
- minimal reproduction steps that do not include secrets

## Secret Handling

Do not commit real API keys, provider tokens, certificates, private keys, or local `.damaian` data. Keep real values out of examples, logs, screenshots, and test fixtures.

Damaian supports macOS Keychain-backed model API key storage from desktop settings. Config files should store Keychain references such as `keychain:model-api-key` or environment variable names for CLI/development workflows, never raw provider keys.

Secret scanning redacts detected credentials from indexed context, command output, patch diffs, Git diff output, audit log fields, and model-visible command results. Generated file content is still checked before apply and is blocked by default when hardcoded secrets are detected.

That block is a warning the user can overrule, not a hard stop. When a selected file is flagged, the apply reports which file tripped the check and which detection categories fired — never the matched value — and applies nothing. The user reviews the diff and may then apply anyway (`Apply Anyway` in the desktop app, `--allow-generated-secrets` in the CLI). The override is per-apply, is never inferred or remembered, and is recorded in the audit log as `generatedSecretOverride=true`.

Assignment-shaped detection (`password=`, `api_key:`, database URL credentials) exempts values that are structurally not credentials: template and variable syntax (`<your-key>`, `${DB_PASSWORD}`, `{{ vault_password }}`, `$OPENAI_KEY`), Keychain references, repeated filler (`xxxxxxxx`), environment variable names, and values naming themselves as examples. Setup documentation is the dominant source of these, and blocking or redacting them protected nothing while making generated READMEs unappliable. Structural detectors — private keys, AWS keys, JWTs, provider token prefixes, Azure keys, custom patterns — are not exempted.

## Local App Boundary

The desktop shell binds to loopback on the fixed app origin `http://127.0.0.1:4765`. Startup refuses to continue if that port is already occupied, and the Tauri capability is scoped to that exact localhost origin.

The desktop API token is never served over HTTP. It is delivered to the webview through a Tauri IPC command (`damaian_desktop_bootstrap`), which only the app's own webview process can invoke, so no local HTTP request against the shell server can retrieve it. Local `/api/*` requests require that token.

Model provider requests use `curl --config -` so the provider API key is passed through the child process stdin configuration, not as a command-line argument.

Patch rollback snapshots are redacted through the same secret scanner used for diffs and Git output before being written to disk, so pre-edit file content captured for rollback does not retain hardcoded credentials.

## Repository Config Trust Boundary

Configuration comes from three files, applied in order: user (`<data_dir>/config/user.conf`), repository (`<repo>/.damaian/config.conf`), and admin (`DAMAIAN_ADMIN_CONFIG` or `<data_dir>/config/admin.conf`). Only the repository file arrives with a clone, so **repository config is untrusted input**: it may add restrictions and never remove one.

A repository cannot set `shell`, `data_dir`, `allowed_roots`, `secret_patterns`, `audit_enabled`, `block_generated_secrets`, or any `model_*` key including a `model_provider.<id>` entry. Those keys redirect where commands run, where model traffic and the API key reference go, where data is written, or which defences are active; a repository has no legitimate use for any of them. They are ignored, recorded in the audit log as `repository_config_key_rejected` with the key name and class — never the value, which is attacker-controlled text — and reported to the user once per repository. There is deliberately no override.

`restricted_patterns`, `ignore_patterns`, and `command_blocklist` are unioned with the user's, so a repository may add a restriction but never drop one. `require_approval_for_file_edits`, `require_approval_for_risky_commands`, and `require_approval_for_all_commands` may be turned on by a repository, never off. `mcp_enabled` and a server's `enabled` may be turned off, never on, and `mcp_server_allowlist` may only be narrowed. A repository may *define* an MCP server — a useful suggestion — but the definition is created disabled and approval-gated, and it cannot redefine a server the user has already configured.

`command_allowlist` is never taken from repository config. An `Allow Always` decision is the user's decision about a repository, so it is stored in user config as `command_allowlist.<repository_id>`, keyed per checkout. The repository's own file cannot grant its commands no-approval execution, and a `command_allowlist` already present in a repository file is offered once for itemised keep-or-discard rather than honoured.

Admin config keeps its ability to both widen and narrow: it is a local file, not something a clone carries.

The same rule covers the other untrusted repository-supplied inputs. `AGENTS.md` instructions cannot widen a working mode or grant capability, and an MCP server descriptor's tool names and descriptions are data, not instructions.
