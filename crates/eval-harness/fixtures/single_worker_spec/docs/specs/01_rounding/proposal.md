# Feature: Round invoice amounts to the nearest cent

Status: In progress

## Motivation

`pricing::to_cents` truncates: 2.349 becomes 2.34. Customers notice that their
invoices are always a fraction of a cent low, and finance wants amounts rounded
to the nearest cent instead.

## Requirements

1. `pricing::to_cents` rounds an amount in mills to the nearest whole cent
   instead of truncating it. 2.349 (2349 mills) becomes 2.35 (235 cents); 2.341
   becomes 2.34.
2. Invoice totals follow the same rule, because `invoice` converts every line
   through `pricing::to_cents`. Update the invoice tests to the new rounding.
3. Negative amounts (credit lines) are out of scope for this feature.
4. An amount exactly halfway between two cents, such as 2.345, must also round
   to a whole cent. The rule for that case is still to be confirmed with
   finance.

## Acceptance

- `pricing` has tests for an amount that rounds up, one that rounds down, and
  the halfway case.
- `invoice` tests pass with the new rounding.
- The scoped checks in `AGENTS.md` pass.
