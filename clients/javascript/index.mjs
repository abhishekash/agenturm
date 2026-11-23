import { openSync, closeSync, readFileSync, fstatSync, constants } from 'node:fs';
import { request as httpRequest } from 'node:http';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { randomBytes } from 'node:crypto';
import { AsyncLocalStorage } from 'node:async_hooks';

const activeSpan = new AsyncLocalStorage();
const MAX_REQUEST_BYTES = 8 * 1024 * 1024;
const MAX_OTLP_SPANS = 1024;
const MAX_OTLP_REPLY_BYTES = 4096;
const MAX_OTLP_TEXT_BYTES = 2048;
const MAX_OTLP_PAYLOAD_BYTES = 64 * 1024;
const OTLP_PAYLOAD_KEYS = ['agenturm.payload.input', 'agenturm.payload.output'];

const OTLP_STATUS = { UNSET: 0, OK: 1, ERROR: 2 };
const COST_AMOUNT = 'agenturm.reported_cost.amount';
const COST_CURRENCY = 'agenturm.reported_cost.currency';
const COST_FIELDS = [
  [COST_AMOUNT, COST_CURRENCY],
  ['gen_ai.usage.cost', 'gen_ai.usage.cost.currency'],
  ['llm.cost.total', 'llm.cost.currency'],
];
const USD_COST = 'llm.cost_usd';
const NANOUNITS = 1_000_000_000n;

function costNanounits(value) {
  if ((typeof value !== 'string' && typeof value !== 'number') || Object.is(value, -0)) return undefined;
  const text = String(value);
  if (text.length > 48) return undefined;
  const decimal = /^([0-9]+)(?:\.([0-9]*))?(?:[eE]([+-]?[0-9]+))?$/.exec(text);
  if (!decimal || decimal[0] !== text) return undefined;
  const [, whole, fraction = '', exponent = '0'] = decimal;
  const power = Number(exponent);
  if (!Number.isInteger(power) || Math.abs(power) > 64) return undefined;
  const digits = whole + fraction;
  const noTrailing = digits.replace(/0+$/, '');
  const significant = noTrailing.replace(/^0+/, '');
  if (!significant) return 0n;
  const shift = 9 + power + digits.length - noTrailing.length - fraction.length;
  if (!Number.isInteger(shift) || shift < 0 || shift > 19 - significant.length) return undefined;
  const amount = BigInt(significant) * 10n ** BigInt(shift);
  return amount <= 9_223_372_036_854_775_807n ? amount : undefined;
}

function reportedCostAttributes(attributes) {
  if (Object.hasOwn(attributes, 'llm.cost_known') && attributes['llm.cost_known'] !== true) return [];
  const amounts = new Set();
  const currencies = new Set();
  if (Object.hasOwn(attributes, USD_COST)) {
    amounts.add(costNanounits(attributes[USD_COST]));
    currencies.add('USD');
  }
  for (const [amountField, currencyField] of COST_FIELDS) {
    const hasCurrency = Object.hasOwn(attributes, currencyField);
    if (Object.hasOwn(attributes, amountField)) {
      if (!hasCurrency) return [];
      amounts.add(costNanounits(attributes[amountField]));
    }
    if (!hasCurrency) continue;
    const unit = attributes[currencyField];
    if (typeof unit !== 'string' || unit.length !== 3 || !/^[A-Z]{3}$/.test(unit)) return [];
    currencies.add(unit);
  }
  if (amounts.size !== 1 || amounts.has(undefined) || currencies.size !== 1) return [];
  const [amount] = amounts;
  const [currency] = currencies;
  const fraction = String(amount % NANOUNITS).padStart(9, '0').replace(/0+$/, '');
  const whole = String(amount / NANOUNITS);
  return [
    { key: COST_AMOUNT, value: { stringValue: fraction ? `${whole}.${fraction}` : whole } },
    { key: COST_CURRENCY, value: { stringValue: currency } },
  ];
}

function otlpTraceEndpoint(raw) {
  const endpoint = String(raw);
  const literal = /^http:\/\/(?:127\.0\.0\.1|\[[0-9a-f:]+\])(?::[0-9]+)?(?:\/(?:v1\/traces)?)?$/i;
  const error = 'OTLP endpoint must be a loopback HTTP trace receiver';
  if (!literal.test(endpoint) || /[\s\x00-\x1f]/.test(endpoint)) throw new TypeError(error);
  let url;
  try { url = new URL(endpoint); }
  catch { throw new TypeError(error); }
  if (!['127.0.0.1', '[::1]'].includes(url.hostname) || url.port === '0'
      || !['/', '/v1/traces'].includes(url.pathname)) {
    throw new TypeError(error);
  }
  url.pathname = '/v1/traces';
  return url.href;
}

function otlpAuthorization(endpoint, configuredEndpoint) {
  if (!configuredEndpoint) return undefined;
  try {
    if (otlpTraceEndpoint(configuredEndpoint) !== endpoint) return undefined;
    const headers = process.env.OTEL_EXPORTER_OTLP_TRACES_HEADERS ?? '';
    if (headers.length > 4096) return undefined;
    const matches = headers.split(',').map(item => item.split('='))
      .filter(([key]) => key.trim().toLowerCase() === 'authorization');
    if (matches.length !== 1 || matches[0].length !== 2) return undefined;
    const value = decodeURIComponent(matches[0][1]);
    if (value.length === 71 && /^Bearer [0-9a-f]{64}$/.test(value)) return value;
  } catch {
    // A malformed environment cannot supply a credential.
  }
  return undefined;
}

function capturedAttributes(source, sessionId) {
  const attributes = [];
  let bytes = 0;
  for (const key of OTLP_PAYLOAD_KEYS) {
    if (!Object.hasOwn(source, key)) continue;
    if (sessionId === null) {
      throw new TypeError('OTLP captured text requires an explicit session identity');
    }
    const text = source[key];
    if (typeof text !== 'string' || !text || text.length > MAX_OTLP_TEXT_BYTES
        || Buffer.byteLength(text) > MAX_OTLP_TEXT_BYTES) {
      throw new TypeError('OTLP captured text is invalid or exceeds its byte limit');
    }
    const markers = /private key|sk-|[sr]k_(?:live|test)_|gh[pousr]_|github_pat_|akia|aiza|xox[baprs]-|\bbearer\b/i;
    const field = /authorization|password|passwd|token|secret|credential|api[_-]?key/i;
    const control = /[\x00-\x08\x0b-\x1f\x7f-\x9f\u061c\u200e\u200f\u2028-\u202e\u2066-\u2069]/;
    if (markers.test(text) || control.test(text)
        || /:\/\/[^/\s]+:[^/\s]+@/.test(text)
        || text.split('\n').some(line => /[:=]/.test(line) && field.test(line))) {
      throw new TypeError('OTLP captured text was withheld');
    }
    bytes += Buffer.byteLength(text);
    attributes.push({ key, value: { stringValue: text } });
  }
  return { attributes, bytes };
}

async function postOtlpJson(endpoint, headers, body, timeoutMs) {
  // Use a fresh direct connection. Global fetch can inherit environment proxies.
  const response = await new Promise((resolve, reject) => {
    const request = httpRequest(endpoint, {
      method: 'POST', agent: false, maxHeaderSize: MAX_OTLP_REPLY_BYTES,
      signal: AbortSignal.timeout(timeoutMs),
      headers: { ...headers, 'content-length': Buffer.byteLength(body), connection: 'close' },
    }, resolve);
    request.once('error', error => {
      const message = error.code === 'ABORT_ERR' ? 'OTLP request deadline exceeded' : 'OTLP request failed';
      reject(new Error(message));
    });
    request.end(body);
  });
  const status = response.statusCode ?? 0;
  if (status < 200 || status >= 300) {
    response.destroy();
    throw new Error(`OTLP receiver returned HTTP ${status}`);
  }
  const contentLength = response.headers['content-length'] ?? '';
  const length = Number(contentLength);
  if (!/^[0-9]+$/.test(contentLength) || length > MAX_OTLP_REPLY_BYTES) {
    response.destroy();
    throw new Error('OTLP receiver returned an unbounded reply');
  }
  const chunks = [];
  let received = 0;
  let reply;
  try {
    for await (const chunk of response) {
      received += chunk.length;
      if (received > length) throw new Error();
      chunks.push(chunk);
    }
    if (received !== length) throw new Error();
    reply = JSON.parse(Buffer.concat(chunks, received).toString('utf8'));
    if (!reply || typeof reply !== 'object' || Array.isArray(reply)) throw new Error();
  } catch {
    response.destroy();
    throw new Error('OTLP receiver returned an invalid reply');
  }
  if (Number(reply.partialSuccess?.rejectedSpans ?? 0) !== 0) {
    throw new Error('OTLP receiver rejected one or more spans');
  }
}

function id(bytes) {
  let value;
  do value = randomBytes(bytes).toString('hex');
  while (/^0+$/.test(value));
  return value;
}

function executionId(value, option) {
  if (value == null) return null;
  if (typeof value !== 'string' || value.length > 512
      || !/^[A-Za-z0-9]/.test(value) || /[^A-Za-z0-9._:/-]/.test(value)) {
    throw new TypeError(`${option} must be an exact native execution identifier`);
  }
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
  #otlpAuthorization;
  #otlpEndpoint;

  constructor({ endpoint, service = 'node-agent', token, transport = 'agenturm',
                timeoutMs = 3000, strict = false, resourceAttributes = {}, captureContent = false } = {}) {
    if (!['agenturm', 'otlp'].includes(transport)) throw new TypeError('unknown trace transport');
    if (captureContent && transport !== 'otlp') {
      throw new TypeError('captureContent requires OTLP transport');
    }
    this.transport = transport;
    this.captureContent = Boolean(captureContent);
    this.service = String(service);
    this.configurationError = null;
    this.endpoint = transport === 'otlp' ? ''
      : String(endpoint === undefined ? 'http://127.0.0.1:4318' : endpoint).replace(/\/+$/, '');
    try {
      if (transport === 'otlp') {
        const configuredEndpoint = process.env.OTEL_EXPORTER_OTLP_TRACES_ENDPOINT;
        this.endpoint = otlpTraceEndpoint(endpoint ?? (configuredEndpoint
          || process.env.OTEL_EXPORTER_OTLP_ENDPOINT || 'http://127.0.0.1:14318'));
        this.#otlpEndpoint = this.endpoint;
        this.token = token;
        if (token === undefined) {
          this.#otlpAuthorization = otlpAuthorization(this.#otlpEndpoint, configuredEndpoint);
        }
        if (this.captureContent && !this.#otlpAuthorization) {
          throw new TypeError('OTLP captured text requires an authenticated trace receiver');
        }
      } else {
        this.token = receiverToken(this.endpoint, token);
      }
    }
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
      const body = JSON.stringify(payload);
      if (this.transport === 'otlp') {
        if (this.#otlpAuthorization && !headers.authorization) {
          headers.authorization = this.#otlpAuthorization;
        }
        await postOtlpJson(this.#otlpEndpoint, headers, body, this.timeoutMs);
      } else {
        const response = await fetch(`${this.endpoint}${path}`, {
          method: 'POST', redirect: 'error', headers, body,
          signal: AbortSignal.timeout(this.timeoutMs),
        });
        if (!response.ok) throw new Error(`Goechala returned HTTP ${response.status}`);
      }
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

  async _postOtlp(run) {
    const attributes = [];
    for (const [key, value] of [
      ['session.id', run.sessionId],
      ['agenturm.source.id', run.harnessId],
      ['agenturm.turn.id', run.turnId],
      ['agenturm.worker.id', run.workerId],
    ]) {
      if (value !== null) attributes.push({ key, value: { stringValue: value } });
    }
    let failure = null;
    let batch = [];
    let privateBytes = 0;
    const send = async () => {
      if (!batch.length) return;
      const payload = { resourceSpans: [{ scopeSpans: [{
        scope: { name: '@agenturm/client' }, spans: batch,
      }] }] };
      if (!await this._post('', payload)) failure = this.lastError;
      batch = [];
      privateBytes = 0;
    };
    for (const span of run.spans) {
      let captured;
      try {
        captured = this.captureContent
          ? capturedAttributes(span.attributes, run.sessionId)
          : { attributes: [], bytes: 0 };
      } catch (error) {
        failure = error;
        if (this.strict) throw error;
        continue;
      }
      if (batch.length === MAX_OTLP_SPANS || privateBytes + captured.bytes > MAX_OTLP_PAYLOAD_BYTES) {
        await send();
      }
      batch.push(span.exportOtlp([...attributes, ...captured.attributes]));
      privateBytes += captured.bytes;
    }
    await send();
    if (failure) this.lastError = failure;
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
      if (this.transport === 'otlp') await this._postOtlp(run);
      else await this._postSpans(run.spans.map(span => span.export()));
    }
    if (failure) throw failure;
    return result;
  }
}

class Run {
  constructor(client, {
    name = 'agent.run', task, sessionId, harnessId, turnId, workerId, attributes = {},
  } = {}) {
    this.client = client;
    this.trace_id = id(16);
    this.sessionId = sessionId == null ? null : String(sessionId).trim();
    if (this.sessionId !== null && (!this.sessionId || this.sessionId.length > 512)) {
      throw new TypeError('sessionId must contain 1 to 512 characters');
    }
    this.harnessId = null;
    this.turnId = null;
    this.workerId = null;
    if (client.transport === 'otlp') {
      if (this.sessionId !== null && !/^[A-Za-z0-9][A-Za-z0-9._:-]*$/.test(this.sessionId)) {
        throw new TypeError('sessionId must be an exact native session identifier');
      }
      if (harnessId != null) {
        this.harnessId = String(harnessId);
        if (!this.sessionId || !/^[a-z][a-z0-9._/-]{0,95}$/.test(this.harnessId)
            || this.harnessId.startsWith('otlp.')) {
          throw new TypeError('harnessId requires an exact sessionId and native harness identifier');
        }
      }
      this.turnId = executionId(turnId, 'turnId');
      this.workerId = executionId(workerId, 'workerId');
      if (this.turnId !== null && (!this.harnessId || !this.sessionId)) {
        throw new TypeError('turnId requires an explicit harnessId and sessionId');
      }
      if (this.workerId !== null && this.turnId === null) {
        throw new TypeError('workerId requires an explicit turnId');
      }
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
    if (this.client.transport === 'otlp') {
      const error = new Error('OTLP transport does not export progress messages');
      this.client.lastError = error;
      if (this.client.strict) throw error;
      return false;
    }
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

  exportOtlp(attributes) {
    this.finish();
    return {
      traceId: this.run.trace_id,
      spanId: this.span_id,
      parentSpanId: this.parent_span_id ?? undefined,
      name: this.parent_span_id ? 'agent.span' : 'agent.run',
      startTimeUnixNano: this.start_unix_nano,
      endTimeUnixNano: this.end_unix_nano,
      status: { code: OTLP_STATUS[this.status] },
      attributes: [...attributes, ...reportedCostAttributes(this.attributes)],
    };
  }
}
