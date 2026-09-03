#!/usr/bin/env node
import { readFile, writeFile } from "node:fs/promises";

const configPath = "crates/desktop-app/tauri.conf.json";

// A stable install must never be offered a preview build, and a preview install
// must never be offered a stable one, so each channel has its own manifest.
const channel = (process.env.RELEASE_CHANNEL || "preview").trim();
if (channel !== "stable" && channel !== "preview") {
  console.error(`RELEASE_CHANNEL must be "stable" or "preview", got "${channel}"`);
  process.exit(2);
}
const manifestName = channel === "stable" ? "latest.json" : "preview.json";

// The repository stays hardcoded here, unlike create-updater-manifest.mjs which
// reads GH_REPO: this endpoint is baked into the shipped binary, so it must not
// vary with whatever fork happens to run the workflow. A fork that wants its own
// updater channel edits this constant deliberately.
const updaterEndpoint = `https://github.com/damijanc/damaian/releases/latest/download/${manifestName}`;
const updaterPubkey = (process.env.TAURI_UPDATER_PUBKEY || "").trim();

if (!updaterPubkey) {
  console.error("TAURI_UPDATER_PUBKEY is required to configure updater artifacts");
  process.exit(2);
}

const config = JSON.parse(await readFile(configPath, "utf8"));

config.bundle = config.bundle || {};
config.bundle.createUpdaterArtifacts = true;
config.plugins = config.plugins || {};
config.plugins.updater = {
  ...(config.plugins.updater || {}),
  pubkey: updaterPubkey,
  endpoints: [updaterEndpoint],
};

await writeFile(configPath, `${JSON.stringify(config, null, 2)}\n`);
console.log(`Enabled Tauri updater artifacts for ${channel} build (${manifestName})`);
