# Security Policy

Damaian is a developer preview. It is designed to keep repository access, file writes, command execution, secret redaction, and audit logging under local application control.

## Supported Versions

Security fixes target the current `main` branch. Published preview builds should be treated as experimental until code signing, notarization, and Keychain-backed credential storage are implemented.

## Reporting a Vulnerability

If you find a security issue, avoid posting exploit details, API keys, repository contents, or private user data in a public issue.

Use GitHub private vulnerability reporting if it is enabled for the repository. If private reporting is not available, open a public issue with a short, non-sensitive summary and offer to share details privately.

Useful reports include:

- affected version or commit
- operating system and architecture
- impact and expected behavior
- minimal reproduction steps that do not include secrets

## Secret Handling

Do not commit real API keys, provider tokens, certificates, private keys, or local `.damaian` data. Use environment variables for model provider keys, and keep real values out of examples, logs, screenshots, and test fixtures.
