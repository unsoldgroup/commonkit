export type RelayLogLevel = 'debug' | 'info' | 'warn' | 'error';

export interface RelayLogEvent {
  ts: string;
  level: RelayLogLevel;
  event: string;
  serverId?: string;
  message?: string;
  [key: string]: unknown;
}

export class RelayLogger {
  private readonly events: RelayLogEvent[] = [];

  constructor(
    private readonly sink: (line: string) => void = (line) => process.stderr.write(`${line}\n`),
    private readonly maxEvents = 200,
  ) {}

  debug(event: string, fields: Record<string, unknown> = {}) {
    this.write('debug', event, fields);
  }

  info(event: string, fields: Record<string, unknown> = {}) {
    this.write('info', event, fields);
  }

  warn(event: string, fields: Record<string, unknown> = {}) {
    this.write('warn', event, fields);
  }

  error(event: string, fields: Record<string, unknown> = {}) {
    this.write('error', event, fields);
  }

  recent(limit = 50) {
    return this.events.slice(-limit);
  }

  private write(level: RelayLogLevel, event: string, fields: Record<string, unknown>) {
    const entry = redact({
      ts: new Date().toISOString(),
      level,
      event,
      ...fields,
    }) as RelayLogEvent;
    this.events.push(entry);
    if (this.events.length > this.maxEvents) this.events.splice(0, this.events.length - this.maxEvents);
    this.sink(JSON.stringify(entry));
  }
}

export function errorFields(err: unknown) {
  if (err instanceof Error) {
    return {
      message: err.message,
      name: err.name,
      stack: err.stack,
    };
  }
  return { message: String(err) };
}

function redact(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(redact);
  if (!value || typeof value !== 'object') return value;
  const out: Record<string, unknown> = {};
  for (const [key, child] of Object.entries(value)) {
    if (isSecretKey(key)) {
      out[key] = '[redacted]';
    } else if (typeof child === 'string') {
      out[key] = redactString(child);
    } else {
      out[key] = redact(child);
    }
  }
  return out;
}

function isSecretKey(key: string) {
  return /authorization|token|secret|password|api[-_]?key|cookie/i.test(key);
}

function redactString(value: string) {
  return value
    .replace(/\bBearer\s+[A-Za-z0-9._~+/=-]+/gi, 'Bearer [redacted]')
    .replace(/\bphx_[A-Za-z0-9._-]+/g, 'phx_[redacted]');
}
