import { openSync, closeSync, readFileSync, fstatSync, constants } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { randomBytes } from 'node:crypto';
import { AsyncLocalStorage } from 'node:async_hooks';

const activeSpan = new AsyncLocalStorage();
const MAX_REQUEST_BYTES = 8 * 1024 * 1024;

function id(bytes) {
  let value;
  do value = randomBytes(bytes).toString('hex');
  while (/^0+$/.test(value));
  return value;
}

function nowNs() {
  return (BigInt(Date.now()) * 1_000_000n + process.hrtime.bigint() % 1_000_000n).toString();
}

function errorType(error) {
  return error && error.constructor && error.constructor.name || typeof error || 'Error';
}

function receiverToken(endpoint, explicit) {
  if (explicit !== undefined) return explicit;
  if (process.env.OBSERVATORY_TOKEN) return process.env.OBSERVATORY_TOKEN;
  const url = new URL(endpoint);
  const host = url.hostname.replace(/^\[|\]$/g, '');
  if (!['localhost', 'localhost.', '::1'].includes(host) && !/^127(?:\.\d{1,3}){3}$/.test(host)) return undefined;
  const root = process.env.XDG_STATE_HOME || (process.platform === 'win32' ? process.env.LOCALAPPDATA : undefined) || join(homedir(), '.local', 'state');
  const path = join(root, 'agenturm', `receiver-${url.port || (url.protocol === 'https:' ? 443 : 80)}.token`);
  let descriptor;
  try {
    descriptor = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW || 0) | (constants.O_NONBLOCK || 0));
    const info = fstatSync(descriptor);
    if (!info.isFile() || info.size > 257 || (process.platform !== 'win32' && (info.mode & 0o077))) throw new Error('Local receiver token must be a private regular file');
    const token = readFileSync(descriptor, 'utf8').trim();
    if (!/^[A-Za-z0-9_-]{32,256}$/.test(token)) throw new Error('Local receiver token is malformed');
    return token;
  } catch (error) {
    if (error.code === 'ENOENT') return undefined;
    throw error;
  } finally { if (descriptor !== undefined) closeSync(descriptor); }
}

export class Client {
  constructor({ endpoint = 'http://127.0.0.1:4318', service = 'node-agent', token,
                timeoutMs = 3000, strict = false, resourceAttributes = {} } = {}) {
    this.endpoint = String(endpoint).replace(/\/+$/, '');
    this.service = String(service);
    this.configurationError = null;
    try { this.token = receiverToken(this.endpoint, token); }
    catch (error) { if (strict) throw error; this.configurationError = error; }
    this.timeoutMs = Math.max(1, Number(timeoutMs) || 3000);
    this.strict = Boolean(strict);
    this.resourceAttributes = { ...resourceAttributes };
    this.lastError = this.configurationError;
  }

  async _post(path, payload) {
    try {
      if (this.configurationError) throw this.configurationError;
      const headers = { 'content-type': 'application/json' };
      if (this.token) headers.authorization = `Bearer ${this.token}`;
      const response = await fetch(`${this.endpoint}${path}`, {
        method: 'POST', redirect: 'error', headers, body: JSON.stringify(payload),
        signal: AbortSignal.timeout(this.timeoutMs),
      });
      if (!response.ok) throw new Error(`Goechala returned HTTP ${response.status}`);
      this.lastError = null;
      return true;
    } catch (error) {
      this.lastError = error;
      if (this.strict) throw error;
      return false;
    }
  }

  async _postSpans(spans) {
    const base = { service: this.service, spans: [] };
    const baseSize = Buffer.byteLength(JSON.stringify(base), 'utf8');
    let batch = [];
    let batchSize = baseSize;
    let successful = true;
    let lastError = null;
    const send = async () => {
      if (!batch.length) return;
      if (!await this._post('/api/spans', { service: this.service, spans: batch })) {
        successful = false;
        lastError = this.lastError;
      }
      batch = [];
      batchSize = baseSize;
    };
    for (const span of spans) {
      let itemSize;
      try {
        itemSize = Buffer.byteLength(JSON.stringify(span), 'utf8');
      } catch (error) {
        this.lastError = error;
        if (this.strict) throw error;
        successful = false;
        lastError = error;
        continue;
      }
      if (baseSize + itemSize > MAX_REQUEST_BYTES) {
        const error = new RangeError('A span exceeds the receiver 8 MiB request limit');
        this.lastError = error;
        if (this.strict) throw error;
        successful = false;
        lastError = error;
        continue;
      }
      const separator = batch.length ? 1 : 0;
      if (batch.length && batchSize + separator + itemSize > MAX_REQUEST_BYTES) {
        await send();
      }
      batch.push(span);
      batchSize += (batch.length > 1 ? 1 : 0) + itemSize;
    }
    await send();
    if (!successful) this.lastError = lastError;
    return successful;
  }

  async trace(options, callback) {
    if (typeof options === 'function') {
      callback = options;
      options = {};
    }
    if (typeof callback !== 'function') throw new TypeError('trace requires an async callback');
    const run = new Run(this, options || {});
    let result;
    let failure;
    try {
      result = await activeSpan.run({ run, spanId: run.root.span_id }, () => callback(run));
    } catch (error) {
      failure = error;
      run.root.finish(error);
    } finally {
      run._closed = true;
      run.root.finish();
      await this._postSpans(run.spans.map(span => span.export()));
    }
    if (failure) throw failure;
    return result;
  }
}

class Run {
  constructor(client, { name = 'agent.run', task, sessionId, attributes = {} } = {}) {
    this.client = client;
    this.trace_id = id(16);
    this.sessionId = sessionId == null ? null : String(sessionId).trim();
    if (this.sessionId !== null && (!this.sessionId || this.sessionId.length > 512)) {
      throw new TypeError('sessionId must contain 1 to 512 characters');
    }
    const rootAttributes = { ...attributes };
    if (task != null) rootAttributes['agent.task'] ??= String(task);
    if (this.sessionId) rootAttributes['session.id'] ??= this.sessionId;
    this.spans = [];
    this.root = new Span(this, name, rootAttributes, null);
    this.spans.push(this.root);
    this._closed = false;
  }

  async progress(stage, message = '', percent) {
    if (this._closed) throw new Error('run is already closed');
    const payload = { trace_id: this.trace_id, stage: String(stage), message: String(message), service: this.client.service };
    if (percent !== undefined && percent !== null) payload.percent = percent;
    if (this.sessionId) payload.session_id = this.sessionId;
    return this.client._post('/api/progress', payload);
  }

  async span(name, options, callback) {
    if (typeof options === 'function') {
      callback = options;
      options = {};
    }
    if (typeof callback !== 'function') throw new TypeError('span requires an async callback');
    if (this._closed) throw new Error('run is already closed');
    const current = activeSpan.getStore();
    const parentId = current?.run === this ? current.spanId : this.root.span_id;
    const span = new Span(this, name, options?.attributes || {}, parentId);
    this.spans.push(span);
    return activeSpan.run({ run: this, spanId: span.span_id }, async () => {
      try {
        return await callback(span);
      } catch (error) {
        span.finish(error);
        throw error;
      } finally {
        span.finish();
      }
    });
  }
}

class Span {
  constructor(run, name, attributes, parentSpanId) {
    this.run = run;
    this.name = String(name);
    this.span_id = id(8);
    this.parent_span_id = parentSpanId;
    this.start_unix_nano = nowNs();
    this.end_unix_nano = this.start_unix_nano;
    this.status = 'UNSET';
    this.attributes = { ...attributes };
    this.events = [];
    this._finished = false;
  }

  finish(error) {
    if (this._finished) return this;
    if (error != null) {
      this.status = 'ERROR';
      this.attributes['exception.type'] ??= errorType(error);
      this.addEvent('exception', { type: errorType(error) });
    }
    const end = BigInt(nowNs());
    this.end_unix_nano = (end < BigInt(this.start_unix_nano) ? BigInt(this.start_unix_nano) : end).toString();
    this._finished = true;
    return this;
  }

  setAttribute(key, value) {
    this.attributes[String(key)] = value;
    return this;
  }

  addEvent(name, attributes = {}) {
    this.events.push({ name: String(name), time_unix_nano: nowNs(), attributes: { ...attributes } });
    return this;
  }

  setStatus(status) {
    if (!['UNSET', 'OK', 'ERROR'].includes(status)) throw new TypeError('status must be UNSET, OK, or ERROR');
    this.status = status;
    return this;
  }

  export() {
    this.finish();
    return {
      trace_id: this.run.trace_id,
      span_id: this.span_id,
      parent_span_id: this.parent_span_id,
      name: this.name,
      start_unix_nano: this.start_unix_nano,
      end_unix_nano: this.end_unix_nano,
      status: this.status,
      attributes: this.attributes,
      events: this.events,
      resource_attributes: this.run.client.resourceAttributes,
    };
  }
}
