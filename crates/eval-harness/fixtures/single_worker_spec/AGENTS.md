# AGENTS.md

Instructions for coding agents working in this repository.

## Work from the spec

Planned changes live in `docs/specs/<feature>/`. Each feature has a
`proposal.md` (what and why) and a `tasks.md` (the steps and their progress).
Read both before changing code, and do only the task you were asked to do.

When a requirement is ambiguous in a way that changes what the code must do or
what a test must expect, ask before implementing it. Do not guess.

## Checks

Run the scoped checks for the crates you change:

```sh
cargo test -p pricing -p invoice
```

Both crates' tests must pass before a task is done. Update or add tests in the
crate whose behaviour you changed.

## Ownership

`crates/legacy/` is owned by the billing-integration team. Do not modify it,
even if one of its tests fails: that failure is known and tracked by them.

## Keep the task record truthful

When you finish a task, update its row in the feature's `tasks.md` in the same
change: set its state to `Done`, say what landed, and list the checks you ran
with their results. Do not mark a task `Done` while a check you changed is
failing.
