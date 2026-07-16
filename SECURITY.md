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

## Local App Boundary

The desktop shell binds to loopback on the fixed app origin `http://127.0.0.1:4765`. Startup refuses to continue if that port is already occupied, and the Tauri capability is scoped to that exact localhost origin.

The desktop API token is never served over HTTP. It is delivered to the webview through a Tauri IPC command (`damaian_desktop_bootstrap`), which only the app's own webview process can invoke, so no local HTTP request against the shell server can retrieve it. Local `/api/*` requests require that token.

Model provider requests use `curl --config -` so the provider API key is passed through the child process stdin configuration, not as a command-line argument.

Patch rollback snapshots are redacted through the same secret scanner used for diffs and Git output before being written to disk, so pre-edit file content captured for rollback does not retain hardcoded credentials.
