import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { createDefaultEngine } from '../src/index.js';
import { withTempDir, writeFixture } from './helpers/tmp.js';

test('indexes source files while respecting gitignore and redacting secrets', async () => {
  await withTempDir('indexer', async (repo) => {
    await writeFixture(repo, '.gitignore', 'dist/\nignored.js\n');
    await writeFixture(repo, 'src/auth.js', 'export function login() { return true; }\n');
    await writeFixture(repo, 'dist/bundle.js', 'generated');
    await writeFixture(repo, 'ignored.js', 'ignored');
    await writeFixture(repo, 'src/secret.js', 'const api_key = "sk_test_12345678901234567890";\n');

    const engine = createDefaultEngine({ config: { dataDir: path.join(repo, '.damaian') } });
    const index = await engine.indexer.indexRepository(repo);

    assert.deepEqual(index.files.map((file) => file.path), ['src/auth.js', 'src/secret.js']);
    assert.equal(index.skipped.some((file) => file.path === 'dist' || file.path === 'dist/bundle.js'), true);
    const secretFile = index.files.find((file) => file.path === 'src/secret.js');
    assert.equal(secretFile.chunks.every((chunk) => !chunk.text.includes('sk_test_12345678901234567890')), true);
    assert.equal(secretFile.chunks.some((chunk) => chunk.text.includes('[REDACTED_')), true);
    assert.equal(index.keywordSearch('login')[0].path, 'src/auth.js');
  });
});
