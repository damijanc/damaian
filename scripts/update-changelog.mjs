#!/usr/bin/env node
// Append newly tagged releases to CHANGELOG.md.
//
// Run it after tagging. It adds a row to the releases table for every tag the
// changelog does not already document, inserted directly under the table's
// separator line, and it **never rewrites an existing row**. Everything below
// the insertion point is spliced through byte for byte, so a row you hand-edited
// after generating it stays edited — re-running is a no-op for any version
// already present.
//
// A tag whose range contains no commits is omitted entirely: it recorded no
// change, so it is not a release worth a heading.
//
// The generated prose is your commit subjects, tidied. That is the point — the
// one-line commit convention in AGENTS.md is what makes this readable. Edit a
// section afterwards if a release deserves better than its commit log; the
// script will not undo it.
//
//   node scripts/update-changelog.mjs            update the file
//   node scripts/update-changelog.mjs --dry-run  print what would be inserted
//   node scripts/update-changelog.mjs --check    exit 1 if any tag is missing
//
// Two deliberate differences from the hand-written rows below the insertion
// point. A commit touching several components is filed under one primary
// component here, rather than under a combined "Engine, App" label — the
// generated text has to be deterministic, and the result is easy to merge by
// hand. And the layout is a two-column Markdown table, so a row is one long
// line: bullets are `<br>`-separated because that is what a table cell holds.

import { readFile, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";

const CHANGELOG = "CHANGELOG.md";

const MODES = new Set(["--dry-run", "--check"]);
const mode = process.argv[2];
if (mode !== undefined && !MODES.has(mode)) {
  console.error("usage: node scripts/update-changelog.mjs [--dry-run|--check]");
  process.exit(2);
}

function git(...args) {
  return execFileSync("git", args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
}

// Longest prefix wins, so `crates/desktop-app` is not shadowed by a shorter
// rule. `Docs` is a real component here only so that a docs-only release can
// say so rather than vanish; it never produces bullets.
const PATH_COMPONENTS = [
  ["crates/workspace-engine", "Engine"],
  ["crates/desktop-shell", "App"],
  ["crates/desktop-app", "App"],
  ["crates/damaian-cli", "CLI"],
  ["crates/eval-harness", "Evaluation"],
  [".github/", "Release"],
  ["scripts/", "Release"],
  ["docs/", "Docs"],
];

const ROOT_RELEASE_FILES = new Set([
  "Cargo.toml",
  "Cargo.lock",
  "deny.toml",
  "package.json",
  "package-lock.json",
  "biome.json",
  "_typos.toml",
  "rust-toolchain.toml",
]);

// Precedence for assigning a multi-component commit to one heading. A change
// that touches the engine is described as an engine change even when it also
// adjusted the interface that shows it.
const COMPONENT_ORDER = ["Engine", "App", "CLI", "Evaluation", "Release", "Other", "Docs"];

function componentFor(path) {
  for (const [prefix, component] of PATH_COMPONENTS) {
    if (path.startsWith(prefix)) return component;
  }
  if (ROOT_RELEASE_FILES.has(path)) return "Release";
  if (path.endsWith(".md")) return "Docs";
  return "Other";
}

function primaryComponent(paths) {
  const found = new Set(paths.map(componentFor));
  for (const component of COMPONENT_ORDER) {
    if (found.has(component)) return component;
  }
  return "Other";
}

// `git tag` order is not history order, and creation order lies when tags are
// applied out of sequence — this repository has two. Reachable-commit count is
// a total order that matches history for a linear branch; the semver tiebreak
// keeps two tags on one commit deterministic.
function semverKey(tag) {
  const parts = tag.replace(/^v/, "").split(/[.\-+]/);
  return parts.map((part) => (/^\d+$/.test(part) ? Number(part) : part));
}

function compareSemver(a, b) {
  const left = semverKey(a);
  const right = semverKey(b);
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    const l = left[index] ?? 0;
    const r = right[index] ?? 0;
    if (l === r) continue;
    return typeof l === "number" && typeof r === "number"
      ? l - r
      : String(l).localeCompare(String(r));
  }
  return 0;
}

function orderedTags() {
  const tags = git("tag")
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  // No date is read: the layout drops dates, and a `git log` per tag for a
  // field nothing renders is 40-odd subprocesses for nothing.
  const measured = tags.map((tag) => ({
    tag,
    reach: Number(git("rev-list", "--count", tag).trim()),
  }));
  measured.sort((a, b) => (a.reach !== b.reach ? a.reach - b.reach : compareSemver(a.tag, b.tag)));
  return measured;
}

function commitsBetween(from, to) {
  const range = from ? `${from}..${to}` : to;
  // Hash, one space, subject. A control-character delimiter would be tidier
  // to split on and is not worth it: the formatter rewrites the escape into a
  // literal invisible byte in this file, which is a trap for whoever reads it
  // next. A subject cannot contain a newline, so the first space is enough.
  const raw = git("log", "--format=%H %s", range);
  return raw
    .split("\n")
    .filter((line) => line.trim())
    .map((line) => {
      const split = line.indexOf(" ");
      const hash = line.slice(0, split);
      const subject = line.slice(split + 1);
      const paths = git("show", "--name-only", "--format=", "--first-parent", "-m", hash)
        .split("\n")
        .map((path) => path.trim())
        .filter(Boolean);
      return { hash, subject, component: primaryComponent(paths) };
    });
}

// Commit subjects are already written as sentences (AGENTS.md asks for one
// subject line, no body). Strip a conventional-commit prefix if one slipped in,
// then make it read as a changelog bullet.
function toBullet(subject) {
  let text = subject.replace(
    /^(?:feat|fix|chore|docs|refactor|test|perf|build|ci)(?:\([^)]*\))?:\s*/i,
    "",
  );
  text = text.trim();
  if (!text) return null;
  text = text[0].toUpperCase() + text.slice(1);
  if (!/[.!?]$/.test(text)) text += ".";
  return text;
}

// A pipe inside a cell ends the cell. Commit subjects quote shell commands, so
// this is not hypothetical.
function escapeCell(text) {
  return text.replace(/\|/g, "\\|");
}

// One release is one table row: version on the left, everything it changed on
// the right. Bullets are `<br>`-separated rather than a real list, which is
// what a Markdown table cell can hold.
function renderRow({ tag }, commits) {
  const withBullets = commits.filter((commit) => commit.component !== "Docs");

  if (withBullets.length === 0) {
    return `| **${tag}** | Documentation and specification updates only. |`;
  }

  const blocks = [];
  for (const component of COMPONENT_ORDER) {
    if (component === "Docs") continue;
    const forComponent = withBullets.filter((commit) => commit.component === component);
    if (forComponent.length === 0) continue;
    const bullets = forComponent
      .map((commit) => toBullet(commit.subject))
      .filter(Boolean)
      .map((bullet) => `<br>- ${escapeCell(bullet)}`)
      .join("");
    blocks.push(`**${component}**${bullets}`);
  }
  return `| **${tag}** | ${blocks.join("<br><br>")} |`;
}

const changelog = await readFile(CHANGELOG, "utf8");
const documented = new Set(
  [...changelog.matchAll(/^\|\s*\*\*(v[^*\s]+)\*\*\s*\|/gm)].map((match) => match[1]),
);

const tags = orderedTags();
const missing = [];
for (let index = 0; index < tags.length; index += 1) {
  const entry = tags[index];
  if (documented.has(entry.tag)) continue;
  const previous = index > 0 ? tags[index - 1].tag : null;
  const commits = commitsBetween(previous, entry.tag);
  // The rule: a tag that introduced no commits is not a release.
  if (commits.length === 0) continue;
  missing.push({ entry, commits });
}

if (missing.length === 0) {
  console.log("CHANGELOG.md is up to date.");
  process.exit(0);
}

const names = missing.map(({ entry }) => entry.tag).join(", ");

if (mode === "--check") {
  console.error(`CHANGELOG.md is missing: ${names}`);
  console.error("Run: npm run changelog:update");
  process.exit(1);
}

// Newest first, to match the file.
const rows = missing
  .slice()
  .reverse()
  .map(({ entry, commits }) => renderRow(entry, commits))
  .join("\n");

if (mode === "--dry-run") {
  process.stdout.write(`${rows}\n`);
  process.exit(0);
}

// Splice, never rewrite: new rows go directly under the releases table's
// separator row, and every existing row below is carried through unchanged.
//
// The marker is what makes the insertion point unambiguous. Without it the
// script would have to guess which of the file's tables is the release table,
// and the Unreleased table above it has exactly the same shape.
const MARKER = "<!-- releases -->";
const markerAt = changelog.indexOf(MARKER);
if (markerAt === -1) {
  console.error(`refusing to write: ${MARKER} not found in ${CHANGELOG}`);
  process.exit(1);
}
const separator = changelog.indexOf("\n", changelog.indexOf("|---|---|", markerAt));
if (separator === -1) {
  console.error(`refusing to write: no table separator after ${MARKER}`);
  process.exit(1);
}

const head = changelog.slice(0, separator + 1);
const tail = changelog.slice(separator + 1);
const updated = `${head}${rows}\n${tail}`;

if (tail && !updated.endsWith(tail)) {
  console.error("refusing to write: existing releases would have changed");
  process.exit(1);
}

await writeFile(CHANGELOG, updated);
console.log(`Added to CHANGELOG.md: ${names}`);
console.log("Review the generated text, and move anything that shipped out of Unreleased.");
