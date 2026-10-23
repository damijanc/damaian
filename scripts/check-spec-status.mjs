#!/usr/bin/env node
// Reports two kinds of disagreement between the records that describe a spec:
//
// 1. A spec's `Depends on:` line disagrees with the depended-on spec's own
//    `Status:` line.
// 2. A spec's status disagrees across the places that record it: its own
//    `Status:` line, its row in `docs/specs/README.md`'s implementation-order
//    table, and — for a folder spec with a `tasks.md` — that file's progress
//    table and `**Started:**`/`**Done:**` header.
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
// foundation does not exist. The status records are the same problem one level
// down: AGENTS.md requires them to agree, and a stale one is read as truth.
//
// This is the mechanical half of the checklist in AGENTS.md, "Before you change
// a feature". The rest of that checklist — the CHANGELOG's `Unreleased`
// section, the user-facing docs — is prose and stays a human obligation.
//
// Node built-ins only, so CI can run it without `npm ci`.

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const SPECS = "docs/specs";
const INDEX = join(SPECS, "README.md");

/**
 * Specs whose records legitimately disagree, each with the rules it is excused
 * from and why. An explicit list rather than a looser rule, so a new
 * disagreement is still reported and an exception has to be argued here.
 */
const STATUS_EXCEPTIONS = new Map([
  [
    49,
    {
      // Its tasks.md plans only the accounting slice (its header says so).
      // That slice is Done, every task in it is Done and the header carries a
      // Done date, while the reuse slice keeps the spec as a whole In progress.
      skip: new Set(["all-tasks-done", "done-date"]),
      why: "tasks.md covers only the accounting slice",
    },
  ],
]);

/** Every spec, keyed by its number: flat `NN_name.md` or folder `NN_name/proposal.md`. */
function readSpecs() {
  const specs = new Map();
  for (const entry of readdirSync(SPECS)) {
    const match = entry.match(/^(\d+)_/);
    if (!match) continue;
    const full = join(SPECS, entry);
    const folder = statSync(full).isDirectory();
    const path = folder ? join(full, "proposal.md") : full;
    let text;
    try {
      text = readFileSync(path, "utf8");
    } catch {
      // A folder without a proposal.md is a different defect, and not this
      // check's business to diagnose.
      continue;
    }
    const tasksPath = folder ? join(full, "tasks.md") : null;
    const tasks = tasksPath && existsSync(tasksPath) ? readFileSync(tasksPath, "utf8") : null;
    specs.set(Number(match[1]), { number: Number(match[1]), path, text, tasksPath, tasks });
  }
  return specs;
}

/**
 * The status class a piece of status text opens with, or null.
 *
 * Only the opening words count: "Done, with one measurement outstanding" is
 * Done, and #15's "Partially done, remainder skipped" is its own class, which
 * agrees with itself wherever it is recorded and is never Done.
 */
function statusClass(text) {
  const match = text.trim().match(/^(not started|in progress|partially done|done)\b/i);
  if (!match) return null;
  const word = match[1].toLowerCase();
  return word.charAt(0).toUpperCase() + word.slice(1);
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

/**
 * The implementation-order table's status for each spec: the bold text that
 * opens the row's third column, e.g. `**Done 2026-10-04.**` or `**In progress**`.
 */
function readIndexRows(text) {
  const rows = new Map();
  const section = text.indexOf("\n## Implementation order");
  if (section === -1) return rows;
  // From the heading itself, so line 0 is the heading and the loop below stops
  // at the next one.
  const lines = text.slice(section + 1).split("\n");
  const firstLine = lineOf(text, section + 1);
  for (const [i, line] of lines.entries()) {
    if (i > 0 && line.startsWith("## ")) break;
    const match = line.match(/^\|\s*(\d+)\s*\|[^|]*\|\s*(.*)\|\s*$/);
    if (!match) continue;
    const bold = match[2].match(/^\*\*(.+?)\*\*/);
    rows.set(Number(match[1]), {
      line: firstLine + i,
      status: bold ? statusClass(bold[1]) : null,
      raw: bold ? bold[1] : match[2].slice(0, 40),
    });
  }
  return rows;
}

/**
 * The progress table in a tasks.md: the first table whose header is
 * `| Task | State | …`. A table without a State column (#53 records "What
 * landed" instead) has no per-task state to compare, so it is not read.
 */
function readTaskStates(text) {
  const lines = text.split("\n");
  const header = lines.findIndex((line) => /^\|\s*Task\s*\|\s*State\s*\|/i.test(line));
  if (header === -1) return null;
  const tasks = [];
  for (let i = header + 2; i < lines.length && lines[i].startsWith("|"); i += 1) {
    const cells = lines[i].split("|").map((cell) => cell.trim());
    const name = cells[1].replace(/\*\*/g, "");
    const state = cells[2].replace(/\*\*/g, "").trim();
    let kind = null;
    if (/^not started/i.test(state)) kind = "notStarted";
    else if (/^in progress/i.test(state)) kind = "inProgress";
    // "Done 2026-10-16; acceptance not met" and "Code done; baseline awaiting
    // review" both record a task whose work landed.
    else if (/\bdone\b/i.test(state)) kind = "done";
    tasks.push({ name, state, kind, line: i + 1 });
  }
  return tasks;
}

/** The `**Started:** … — **Done:** …` header line, which may wrap onto the next line. */
function readTaskHeader(text) {
  const match = text.match(/^\*\*Started:\*\*(.*)(?:\n(?!\n)(.*))?/m);
  if (!match) return null;
  const rest = `${match[1]} ${match[2] ?? ""}`;
  return {
    line: lineOf(text, match.index),
    started: /^\s*\d{4}-\d{2}-\d{2}/.test(match[1]),
    done: /\*\*Done:\*\*\s*\d{4}-\d{2}-\d{2}/.test(rest),
  };
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
        kind: "dependency",
        path: spec.path,
        line,
        summary: `says #${number} is **not built**, but ${target.path} is Done.`,
        fix: `Flip it to built, then promote what it unblocked in ${INDEX}'s "What to build next".`,
      });
    } else if (claimsBuilt && !built) {
      problems.push({
        kind: "dependency",
        path: spec.path,
        line,
        summary: `says #${number} is built, but ${target.path} is not Done.`,
        fix: "Either that spec's Status is stale, or this dependency was marked built too early.",
      });
    }
  }
}

const indexText = readFileSync(INDEX, "utf8");
const indexRows = readIndexRows(indexText);

for (const spec of specs.values()) {
  const statusMatch = spec.text.match(/^Status:\s*(.*)$/m);
  const statusLine = statusMatch ? lineOf(spec.text, statusMatch.index) : 1;
  const own = statusMatch ? statusClass(statusMatch[1]) : null;
  const skip = STATUS_EXCEPTIONS.get(spec.number)?.skip ?? new Set();
  const report = (path, line, summary, fix) =>
    problems.push({ kind: "status", path, line, summary, fix });

  if (!own) {
    report(
      spec.path,
      statusLine,
      "has no `Status:` line opening with Not started, In progress, Partially done or Done.",
      "Give it one; every other status record is compared against it.",
    );
    continue;
  }

  // (b) The README row.
  const row = indexRows.get(spec.number);
  if (!row) {
    report(
      INDEX,
      1,
      `has no implementation-order row for #${spec.number} (${spec.path}).`,
      "Add one, opening its third column with the bold status.",
    );
  } else if (row.status !== own) {
    report(
      INDEX,
      row.line,
      `row #${spec.number} reads "${row.raw}", but ${spec.path} says ${own}.`,
      "The spec's own Status: line is the source; bring the row into line with it.",
    );
  }

  // (c) tasks.md, for a folder spec that has one.
  if (!spec.tasks) continue;
  const tasks = readTaskStates(spec.tasks) ?? [];
  const header = readTaskHeader(spec.tasks);
  const started = tasks.filter((task) => task.kind === "done" || task.kind === "inProgress");
  const allDone = tasks.length > 0 && tasks.every((task) => task.kind === "done");

  for (const task of tasks.filter((t) => t.kind === null)) {
    report(
      spec.tasksPath,
      task.line,
      `task "${task.name}" has state "${task.state}", which is not Not started, In progress or Done.`,
      "Open the State cell with one of those three.",
    );
  }
  if (allDone && own !== "Done" && !skip.has("all-tasks-done")) {
    report(
      spec.tasksPath,
      tasks[0].line,
      `marks every task Done, but ${spec.path} says ${own}.`,
      "Either the spec is Done and its Status: line is stale, or a task is not really Done.",
    );
  }
  if (own === "Done" && tasks.length > 0 && !allDone) {
    const open = tasks.find((task) => task.kind !== "done");
    report(
      spec.tasksPath,
      open.line,
      `task "${open.name}" is "${open.state}", but ${spec.path} says Done.`,
      "Tick and record the task, or the spec is not Done yet.",
    );
  }
  if (started.length > 0 && own === "Not started") {
    report(
      spec.tasksPath,
      started[0].line,
      `task "${started[0].name}" is "${started[0].state}", but ${spec.path} says Not started.`,
      "Move the spec, and its README row, to In progress.",
    );
  }
  if (started.length > 0 && !header?.started) {
    report(
      spec.tasksPath,
      header?.line ?? 1,
      "has work under way but no dated `**Started:** YYYY-MM-DD` header.",
      "Record the date the first task started.",
    );
  }
  if (own === "Done" && !header?.done) {
    report(
      spec.tasksPath,
      header?.line ?? 1,
      `has no \`**Done:** YYYY-MM-DD\` in its header, but ${spec.path} says Done.`,
      "Append the completion date to the Started line.",
    );
  }
  if (header?.done && own !== "Done" && !skip.has("done-date")) {
    report(
      spec.tasksPath,
      header.line,
      `carries a **Done:** date, but ${spec.path} says ${own}.`,
      "Either the spec is Done and its Status: line is stale, or the date is premature.",
    );
  }
}

if (problems.length === 0) {
  console.log(
    `Spec status check passed: ${specs.size} specs, dependency lines and status records agree.`,
  );
  process.exit(0);
}

const label = strict ? "failed" : "warning";
const dependencies = problems.filter((problem) => problem.kind === "dependency").length;
const statuses = problems.length - dependencies;
console.error(
  `Spec status check ${label}: ${dependencies} dependency ${dependencies === 1 ? "line disagrees" : "lines disagree"} with the specs they name, ${statuses} status ${statuses === 1 ? "record disagrees" : "records disagree"} with the spec's own Status: line.\n`,
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
console.error('\nSee AGENTS.md, "Before you change a feature".');

// Advisory by default: the finding is worth saying and not worth stopping a
// build for. `--strict` is the switch, not an edit to this file.
process.exit(strict ? 1 : 0);
