#!/usr/bin/env node
// Reports when a spec's `Depends on:` line disagrees with the depended-on
// spec's own `Status:` line.
//
// **Advisory by default: it warns and exits 0.** A documentation inconsistency
// should not stop a build — the person it blocks is usually the one who just
// did the work, and a gate that punishes finishing a spec teaches people not to
// mark specs Done. `--strict` exits 1 instead, which is how this becomes
// blocking the day that trade looks different.
//
// Under GitHub Actions it emits `::warning` annotations, so a warning nobody
// scrolls to still appears on the run and on the pull request.
//
// `docs/specs/README.md`'s "What to build next" is *derived* from those lines,
// so a spec that is Done while another still calls it **not built** leaves its
// dependants looking blocked — and re-deriving that section repairs nothing,
// because the source it reads is what is wrong. The reverse lie is worse: a
// line claiming a prerequisite is built sends someone to start work whose
// foundation does not exist.
//
// This is the mechanical half of the checklist in AGENTS.md, "Before you change
// a feature". The rest of that checklist — the CHANGELOG's `Unreleased`
// section, the user-facing docs — is prose and stays a human obligation.
//
// Node built-ins only, so CI can run it without `npm ci`.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const SPECS = "docs/specs";

/** Every spec, keyed by its number: flat `NN_name.md` or folder `NN_name/proposal.md`. */
function readSpecs() {
  const specs = new Map();
  for (const entry of readdirSync(SPECS)) {
    const match = entry.match(/^(\d+)_/);
    if (!match) continue;
    const full = join(SPECS, entry);
    const path = statSync(full).isDirectory() ? join(full, "proposal.md") : full;
    let text;
    try {
      text = readFileSync(path, "utf8");
    } catch {
      // A folder without a proposal.md is a different defect, and not this
      // check's business to diagnose.
      continue;
    }
    specs.set(Number(match[1]), { number: Number(match[1]), path, text });
  }
  return specs;
}

/**
 * `built` only when the spec's own status begins with "Done".
 *
 * Deliberately strict: #15 reads "Partially done, remainder skipped" and #19
 * "Done, with one measurement outstanding". The first is not something another
 * spec may build on; the second is. Anything that does not start with "Done"
 * counts as not built, so the failure direction is a dependant waiting rather
 * than a dependant starting on a foundation that is missing.
 */
function isBuilt(text) {
  const status = text.match(/^Status:\s*(.*)$/m);
  return status ? /^Done\b/i.test(status[1].trim()) : false;
}

/**
 * The `Depends on:` block, which runs until the next header key or section,
 * with its offset in the file so a finding can name a line number.
 */
function dependsBlock(text) {
  const start = text.indexOf("\nDepends on:");
  if (start === -1) return null;
  const rest = text.slice(start + 1);
  const end = rest.search(
    /\n(?:##\s|Related implementation specs:|Also in this spec:|Background and)/,
  );
  return { text: end === -1 ? rest : rest.slice(0, end), start: start + 1 };
}

/** 1-based line number of an offset, for the editor and for GitHub's annotation. */
function lineOf(text, offset) {
  let line = 1;
  for (let i = 0; i < offset; i += 1) if (text[i] === "\n") line += 1;
  return line;
}

const strict = process.argv.includes("--strict");
const specs = readSpecs();
const problems = [];

for (const spec of specs.values()) {
  const block = dependsBlock(spec.text);
  if (!block) continue;

  // Each dependency runs from its `#NN]` link to the next `;` or the end.
  for (const match of block.text.matchAll(/#(\d+)\]\([^)]*\)([^;]*)/g)) {
    const number = Number(match[1]);
    const claim = match[2];
    const target = specs.get(number);
    if (!target) continue;

    // "not built" must be tested first: it contains "built".
    const claimsNotBuilt = /not\s+\*{0,2}built/i.test(claim);
    const claimsBuilt = !claimsNotBuilt && /\bbuilt\b/i.test(claim);
    if (!claimsNotBuilt && !claimsBuilt) continue;

    const built = isBuilt(target.text);
    const line = lineOf(spec.text, block.start + match.index);
    if (claimsNotBuilt && built) {
      problems.push({
        path: spec.path,
        line,
        summary: `says #${number} is **not built**, but ${target.path} is Done.`,
        fix: `Flip it to built, then promote what it unblocked in ${SPECS}/README.md's "What to build next".`,
      });
    } else if (claimsBuilt && !built) {
      problems.push({
        path: spec.path,
        line,
        summary: `says #${number} is built, but ${target.path} is not Done.`,
        fix: "Either that spec's Status is stale, or this dependency was marked built too early.",
      });
    }
  }
}

if (problems.length === 0) {
  console.log(`Spec status check passed: ${specs.size} specs, dependency lines agree.`);
  process.exit(0);
}

const label = strict ? "failed" : "warning";
console.error(
  problems.length === 1
    ? "Spec status check " + label + ": 1 dependency line disagrees with the spec it names.\n"
    : `Spec status check ${label}: ${problems.length} dependency lines disagree with the specs they name.\n`,
);
for (const problem of problems) {
  console.error(`  - ${problem.path}:${problem.line}: ${problem.summary}`);
  console.error(`    ${problem.fix}`);
  // One line, because a newline would end the annotation.
  if (process.env.GITHUB_ACTIONS) {
    console.log(
      `::warning file=${problem.path},line=${problem.line}::${problem.summary} ${problem.fix}`,
    );
  }
}
console.error('\nSee AGENTS.md, "Finishing a spec goes further than those four".');

// Advisory by default: the finding is worth saying and not worth stopping a
// build for. `--strict` is the switch, not an edit to this file.
process.exit(strict ? 1 : 0);
