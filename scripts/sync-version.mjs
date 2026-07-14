#!/usr/bin/env node
import { readFile, writeFile } from 'node:fs/promises';

const rawVersion = process.argv[2]?.trim();

if (!rawVersion) {
  console.error('usage: node scripts/sync-version.mjs <version>');
  process.exit(2);
}

const version = rawVersion.replace(/^v/, '');
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`invalid version: ${rawVersion}`);
  process.exit(2);
}

async function updateJson(path, mutate) {
  const value = JSON.parse(await readFile(path, 'utf8'));
  mutate(value);
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`);
}

await updateJson('package.json', (value) => {
  value.version = version;
});

await updateJson('crates/desktop-app/tauri.conf.json', (value) => {
  value.version = version;
});

const cargoPath = 'Cargo.toml';
const cargoToml = await readFile(cargoPath, 'utf8');
const updatedCargoToml = cargoToml.replace(
  /(\[workspace\.package\][\s\S]*?\nversion\s*=\s*")[^"]+(")/,
  `$1${version}$2`,
);

if (updatedCargoToml === cargoToml) {
  console.error('failed to update workspace package version in Cargo.toml');
  process.exit(1);
}

await writeFile(cargoPath, updatedCargoToml);
console.log(`Synced release version ${version}`);
