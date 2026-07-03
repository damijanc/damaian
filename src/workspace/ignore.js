function normalizePath(relativePath) {
  return relativePath.split('\\').join('/').replace(/^\.\//, '');
}

function escapeRegex(value) {
  return value.replace(/[|\\{}()[\]^$+?.]/g, '\\$&');
}

function wildcardToRegex(pattern) {
  let output = '';
  for (let index = 0; index < pattern.length; index += 1) {
    const character = pattern[index];
    const next = pattern[index + 1];
    if (character === '*' && next === '*') {
      output += '.*';
      index += 1;
    } else if (character === '*') {
      output += '[^/]*';
    } else if (character === '?') {
      output += '[^/]';
    } else {
      output += escapeRegex(character);
    }
  }
  return output;
}

function segmentMatches(segment, pattern) {
  return new RegExp(`^${wildcardToRegex(pattern)}$`).test(segment);
}

function pathMatches(relativePath, pattern) {
  return new RegExp(`^${wildcardToRegex(pattern)}$`).test(relativePath);
}

export function parseIgnorePatterns(patterns, basePath = '') {
  return patterns
    .map((rawPattern) => String(rawPattern).trim())
    .filter((pattern) => pattern.length > 0 && !pattern.startsWith('#'))
    .map((pattern) => {
      const negated = pattern.startsWith('!');
      let normalized = negated ? pattern.slice(1) : pattern;
      const anchored = normalized.startsWith('/');
      if (anchored) normalized = normalized.slice(1);
      const directoryOnly = normalized.endsWith('/');
      if (directoryOnly) normalized = normalized.slice(0, -1);
      normalized = normalizePath(normalized);
      return {
        pattern: normalized,
        basePath: normalizePath(basePath),
        negated,
        anchored,
        directoryOnly,
        hasSlash: normalized.includes('/')
      };
    });
}

export function ruleMatches(rule, relativePath, isDirectory) {
  const rel = normalizePath(relativePath);
  const base = rule.basePath;

  if (base && !(rel === base || rel.startsWith(`${base}/`))) return false;

  if (!rule.hasSlash && !rule.anchored) {
    const parts = rel.split('/');
    if (rule.directoryOnly) {
      return parts.some((part) => segmentMatches(part, rule.pattern));
    }
    return parts.some((part) => segmentMatches(part, rule.pattern));
  }

  const fullPattern = normalizePath([base, rule.pattern].filter(Boolean).join('/'));
  if (rule.directoryOnly) {
    if (isDirectory && pathMatches(rel, fullPattern)) return true;
    return rel.startsWith(`${fullPattern}/`);
  }
  return pathMatches(rel, fullPattern);
}

export function isIgnoredByRules(rules, relativePath, isDirectory = false) {
  let ignored = false;
  for (const rule of rules) {
    if (ruleMatches(rule, relativePath, isDirectory)) ignored = !rule.negated;
  }
  return ignored;
}
