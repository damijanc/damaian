# Billing

A small billing workspace: `pricing` converts money amounts, `invoice` builds
invoice totals from it, and `legacy` reads reference codes from the old system.

Amounts are integer mills (thousandths of the currency unit). See
`docs/specs/` for planned changes and `AGENTS.md` for how to work here.
