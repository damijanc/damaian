import path from 'node:path';
import { appendFile, mkdir, readFile } from 'node:fs/promises';
import { createId, nowIso } from '../core/hash.js';
import { SecretScanner } from '../security/secret-scanner.js';

function datePart(timestamp) {
  return timestamp.slice(0, 10);
}

export class AuditLog {
  constructor({ dataDir, enabled = true, scanner = new SecretScanner(), localProfileId = 'local_user' } = {}) {
    this.dataDir = dataDir;
    this.enabled = enabled;
    this.scanner = scanner;
    this.localProfileId = localProfileId;
  }

  logPathFor(timestamp = nowIso()) {
    return path.join(this.dataDir, 'audit', `${datePart(timestamp)}.jsonl`);
  }

  async record(eventType, fields = {}) {
    const timestamp = nowIso();
    const event = {
      eventId: createId('evt'),
      timestamp,
      userId: this.localProfileId,
      eventType,
      ...fields
    };
    const safeEvent = this.scanner.redactObject(event);

    if (!this.enabled) return safeEvent;

    const logPath = this.logPathFor(timestamp);
    await mkdir(path.dirname(logPath), { recursive: true });
    await appendFile(logPath, `${JSON.stringify(safeEvent)}\n`, 'utf8');
    return safeEvent;
  }

  async readEventsForDate(date) {
    const logPath = path.join(this.dataDir, 'audit', `${date}.jsonl`);
    const content = await readFile(logPath, 'utf8');
    return content
      .split('\n')
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  }
}
