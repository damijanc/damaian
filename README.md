# Damaian Client

Local-first AI coding assistant client foundation.

This repository currently implements the workspace-engine slice from the product specification:

- repository indexing with default exclusions and `.gitignore` support
- controlled file reads scoped to selected repository roots
- context assembly with secret redaction
- structured patch preview and safe apply with hash checks
- terminal command risk classification and approval-gated execution
- basic Git status/diff wrappers
- append-only local audit logs
- provider-isolated model adapter interfaces

The macOS desktop shell can layer on top of these services without taking over file access, command execution, patching, or audit decisions.

## Commands

```sh
npm test

# Command-line Rust implementation
cargo test
cargo run -p damaian-cli -- search /path/to/repo "auth token"
DAMAIAN_MOCK_MODEL_RESPONSE="Mock answer" cargo run -p damaian-cli -- ask /path/to/repo "What does auth do?"
OPENAI_API_KEY=... cargo run -p damaian-cli -- ask /path/to/repo "Explain the project"
DAMAIAN_MOCK_MODEL_RESPONSE=$'DAMAIAN_EDIT_V1\nSUMMARY: Update file\nFILE: src/app.js\nSTATUS: modified\nCONTENT:\n...\nEND_FILE\nEND_PATCH\n' cargo run -p damaian-cli -- propose-edit /path/to/repo "Make the change"
cargo run -p damaian-cli -- apply-patch /path/to/repo patch_id_from_preview
cargo run -p damaian-cli -- reject-patch patch_id_from_preview

# Command-line Node reference implementation
node ./bin/damaian-client.js index /path/to/repo
node ./bin/damaian-client.js search /path/to/repo "auth token"
node ./bin/damaian-client.js git-status /path/to/repo
node ./bin/damaian-client.js classify-command "npm test"
```

Set `DAMAIAN_DATA_DIR=.damaian` when you want CLI audit and rollback data to stay inside the current workspace during local development.

No runtime dependencies are required for this first implementation slice.
