import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { symlink } from 'node:fs/promises';
import { PathPolicy, AccessDeniedError } from '../src/index.js';
import { withTempDir, writeFixture } from './helpers/tmp.js';

test('denies symlink traversal outside selected repository', async () => {
  await withTempDir('path-policy', async (tmp) => {
    const repo = path.join(tmp, 'repo');
    const outside = path.join(tmp, 'outside');
    await writeFixture(repo, 'src/app.js', 'console.log("ok");');
    await writeFixture(outside, 'secret.txt', 'password=supersecret');
    await symlink(path.join(outside, 'secret.txt'), path.join(repo, 'linked-secret.txt'));

    const policy = new PathPolicy({ allowedRoots: [repo] });
    await assert.rejects(
      () => policy.resolveExisting(repo, 'linked-secret.txt'),
      AccessDeniedError
    );
  });
});

test('marks restricted dotenv files', async () => {
  const policy = new PathPolicy();
  assert.equal(policy.isRestricted('.env'), true);
  assert.equal(policy.isRestricted('src/app.js'), false);
});
