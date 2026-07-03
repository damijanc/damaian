import test from 'node:test';
import assert from 'node:assert/strict';
import { SecretScanner } from '../src/index.js';

test('redacts credential assignments without removing the key name', () => {
  const scanner = new SecretScanner();
  const result = scanner.redact('api_key = "sk_test_12345678901234567890"');

  assert.match(result.text, /api_key = "/);
  assert.match(result.text, /\[REDACTED_/);
  assert.equal(result.findings.length, 1);
});

test('detects private keys', () => {
  const scanner = new SecretScanner();
  const secret = '-----BEGIN PRIVATE KEY-----\nabc123\n-----END PRIVATE KEY-----';
  const findings = scanner.scan(secret);

  assert.equal(findings.length, 1);
  assert.equal(findings[0].category, 'private_key');
});
