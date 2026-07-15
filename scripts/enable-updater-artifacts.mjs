#!/usr/bin/env node
import { readFile, writeFile } from 'node:fs/promises';

const configPath = 'crates/desktop-app/tauri.conf.json';
const updaterEndpoint = 'https://github.com/damijanc/damaian/releases/latest/download/latest.json';
const updaterPubkey = (process.env.TAURI_UPDATER_PUBKEY || '').trim();

if (!updaterPubkey) {
  console.error('TAURI_UPDATER_PUBKEY is required to configure updater artifacts');
  process.exit(2);
}

const config = JSON.parse(await readFile(configPath, 'utf8'));

config.bundle = config.bundle || {};
config.bundle.createUpdaterArtifacts = true;
config.plugins = config.plugins || {};
config.plugins.updater = {
  ...(config.plugins.updater || {}),
  pubkey: updaterPubkey,
  endpoints: [updaterEndpoint],
};

await writeFile(configPath, `${JSON.stringify(config, null, 2)}\n`);
console.log('Enabled Tauri updater artifacts for release build');
