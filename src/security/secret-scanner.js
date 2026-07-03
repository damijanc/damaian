import { sha256 } from '../core/hash.js';

const BUILTIN_DETECTORS = [
  {
    category: 'private_key',
    regex: /-----BEGIN (?:RSA |DSA |EC |OPENSSH |PGP )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |DSA |EC |OPENSSH |PGP )?PRIVATE KEY-----/g,
    valueGroup: 0
  },
  {
    category: 'aws_access_key',
    regex: /\bAKIA[0-9A-Z]{16}\b/g,
    valueGroup: 0
  },
  {
    category: 'bearer_token',
    regex: /\bBearer\s+([A-Za-z0-9._~+/=-]{16,})/gi,
    valueGroup: 1
  },
  {
    category: 'database_url',
    regex: /\b(?:postgres|postgresql|mysql|mongodb|redis):\/\/[^:\s/@]+:([^@\s]+)@[^\s]+/gi,
    valueGroup: 1
  },
  {
    category: 'credential_assignment',
    regex: /\b(password|passwd|pwd|secret|api[_-]?key|token|access[_-]?token|client[_-]?secret)\b\s*[:=]\s*(['"]?)([^\s'"]{8,})\2/gi,
    valueGroup: 3
  },
  {
    category: 'generic_api_key',
    regex: /\b(?:sk|pk|rk|ghp|github_pat|xoxb|xoxp)[A-Za-z0-9_\-]{20,}\b/g,
    valueGroup: 0
  }
];

function regexFromCustomPattern(pattern) {
  if (pattern instanceof RegExp) return pattern;
  if (typeof pattern === 'string' && pattern.length > 0) return new RegExp(pattern, 'g');
  return null;
}

function matchValueBounds(fullMatch, matchIndex, value) {
  const offset = fullMatch.indexOf(value);
  if (offset === -1) {
    return { start: matchIndex, end: matchIndex + fullMatch.length };
  }
  return { start: matchIndex + offset, end: matchIndex + offset + value.length };
}

function placeholderFor(category, value) {
  const digest = sha256(value).slice('sha256:'.length, 'sha256:'.length + 10);
  return `[REDACTED_${category.toUpperCase()}_${digest}]`;
}

function removeOverlappingFindings(findings) {
  const ordered = [...findings].sort((a, b) => a.start - b.start || b.end - a.end);
  const accepted = [];
  for (const finding of ordered) {
    const overlaps = accepted.some((existing) => finding.start < existing.end && finding.end > existing.start);
    if (!overlaps) accepted.push(finding);
  }
  return accepted.sort((a, b) => a.start - b.start);
}

export class SecretScanner {
  constructor({ customPatterns = [] } = {}) {
    this.detectors = [
      ...BUILTIN_DETECTORS,
      ...customPatterns
        .map(regexFromCustomPattern)
        .filter(Boolean)
        .map((regex) => ({ category: 'custom_secret', regex, valueGroup: 0 }))
    ];
  }

  scan(text) {
    if (typeof text !== 'string' || text.length === 0) return [];

    const findings = [];
    for (const detector of this.detectors) {
      detector.regex.lastIndex = 0;
      let match;
      while ((match = detector.regex.exec(text)) !== null) {
        const value = match[detector.valueGroup] ?? match[0];
        if (!value) continue;
        const { start, end } = matchValueBounds(match[0], match.index, value);
        findings.push({
          category: detector.category,
          start,
          end,
          length: end - start,
          placeholder: placeholderFor(detector.category, value)
        });
        if (match[0].length === 0) detector.regex.lastIndex += 1;
      }
    }
    return removeOverlappingFindings(findings);
  }

  redact(text) {
    const findings = this.scan(text);
    if (findings.length === 0) return { text, findings };

    let cursor = 0;
    let redacted = '';
    for (const finding of findings) {
      redacted += text.slice(cursor, finding.start);
      redacted += finding.placeholder;
      cursor = finding.end;
    }
    redacted += text.slice(cursor);
    return { text: redacted, findings };
  }

  containsSecrets(text) {
    return this.scan(text).length > 0;
  }

  redactObject(value) {
    if (typeof value === 'string') return this.redact(value).text;
    if (Array.isArray(value)) return value.map((item) => this.redactObject(item));
    if (value && typeof value === 'object') {
      return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, this.redactObject(item)]));
    }
    return value;
  }
}
