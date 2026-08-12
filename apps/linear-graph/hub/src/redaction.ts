const secretPatterns: RegExp[] = [
  /(?:sk|rk|sess|access|refresh)-[A-Za-z0-9._-]{12,}/gi,
  /\b(?:ghp|gho|ghs|ghr|github_pat|xox[baprs])-[A-Za-z0-9._-]{10,}\b/gi,
  /\bBearer\s+[A-Za-z0-9._~+/=-]+/gi,
  /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g,
  /\b(?:api[_-]?key|token|secret|password|authorization)\s*[:=]\s*[^\s,;]+/gi,
];

export function redactSensitiveText(value: string, knownSecrets: readonly string[] = []): string {
  let result = value;
  for (const secret of knownSecrets) {
    if (secret.length >= 6) result = result.split(secret).join("[redacted]");
  }
  for (const pattern of secretPatterns) result = result.replace(pattern, (match) => match.includes("=") ? `${match.slice(0, match.indexOf("=") + 1)}[redacted]` : "[redacted]");
  return result;
}

export function boundText(value: string, max: number): string {
  if (value.length <= max) return value;
  const marker = `\n[output truncated at ${max} characters]`;
  return `${value.slice(0, Math.max(0, max - marker.length))}${marker}`;
}
