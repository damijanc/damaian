import { createHash, randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';

export function sha256(value) {
  return `sha256:${createHash('sha256').update(value).digest('hex')}`;
}

export async function fileHash(filePath) {
  return sha256(await readFile(filePath));
}

export function createId(prefix) {
  return `${prefix}_${randomUUID()}`;
}

export function nowIso() {
  return new Date().toISOString();
}
