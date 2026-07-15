#!/usr/bin/env node
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

const version = (process.env.RELEASE_VERSION || process.argv[2] || '').replace(/^v/, '').trim();
const tag = (process.env.TAG_NAME || process.argv[3] || (version ? `v${version}` : '')).trim();
const repo = (process.env.GH_REPO || process.argv[4] || '').trim();

if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  console.error('RELEASE_VERSION or first argument must be a semantic version, for example 0.1.4');
  process.exit(2);
}

if (!tag) {
  console.error('TAG_NAME or second argument is required');
  process.exit(2);
}

if (!repo || !/^[^/]+\/[^/]+$/.test(repo)) {
  console.error('GH_REPO or third argument must be owner/repository');
  process.exit(2);
}

const bundleName = 'Damaian.app.tar.gz';
const signaturePath = `target/release/bundle/macos/${bundleName}.sig`;
const signature = (await readFile(signaturePath, 'utf8')).trim();
const releaseBaseUrl = `https://github.com/${repo}/releases/download/${tag}`;

const manifest = {
  version,
  notes: `Damaian ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    'darwin-aarch64': {
      signature,
      url: `${releaseBaseUrl}/${bundleName}`,
    },
  },
};

const outputDir = 'target/release/bundle/updater';
await mkdir(outputDir, { recursive: true });
await writeFile(path.join(outputDir, 'latest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Created updater manifest for ${tag}`);
