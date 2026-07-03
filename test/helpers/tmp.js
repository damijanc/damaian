import os from 'node:os';
import path from 'node:path';
import { mkdtemp, rm, writeFile, mkdir } from 'node:fs/promises';

export async function withTempDir(name, callback) {
  const root = await mkdtemp(path.join(os.tmpdir(), `${name}-`));
  try {
    return await callback(root);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

export async function writeFixture(root, relativePath, content) {
  const filePath = path.join(root, relativePath);
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, content, 'utf8');
  return filePath;
}
