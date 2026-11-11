import assert from 'node:assert/strict';
import test from 'node:test';
import { register } from './hooks/register.js';

const SOCKET = '/private/tmp/cbc113/core/device.sock';
const A = 'a'.repeat(64), B = '0'.repeat(64);
const pending = cursor => ({ status: 200, text: JSON.stringify({ v: 1, type: 'pending', cursor }) });
const recovery = cursor => ({ status: 200, text: JSON.stringify({ v: 1, type: 'recovery', cursor }) });
const tick = () => new Promise(resolve => setImmediate(resolve));
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function host(options = { socketPath: SOCKET }) {
  const hooks = new Map(), timers = [], requests = [], prompts = [], replies = [], reads = [];
  let config = JSON.stringify({ v: 1, ...options });
  let submit = async () => ({});
  const $ = {
    session: { cwd: async () => '/private/tmp/cbc113/claude-project' },
    fs: { read: async path => { reads.push(path); return config; } },
    clock: { after(delay, fn) {
      const timer = { delay, fn, cancelled: false, cancel() { this.cancelled = true; } };
      timers.push(timer);
      return timer;
    } },
    http: { fetch(url, init) {
      requests.push({ url, init });
      if (replies.length === 0) throw new Error('unexpected request');
      const reply = replies.shift();
      return typeof reply === 'function' ? reply() : Promise.resolve(reply);
    } },
    prompt: { submit(args) { prompts.push(args); return submit(args); } },
  };
  register((name, fn) => hooks.set(name, fn));
  const fire = (name, e = {}, result = {}) => hooks.get(name)($, e, async () => result);
  const waiting = () => timers.filter(timer => !timer.cancelled);
  async function run() {
    const timer = waiting()[0];
    assert.ok(timer, 'expected one queued transport callback');
    timer.cancelled = true;
    return timer.fn();
  }
  return { $, fire, waiting, run, requests, prompts, replies, reads,
    setSubmit(fn) { submit = fn; }, setConfig(raw) { config = raw; } };
}

test('idle transport opens one private UDS request and spends no model turn', async () => {
  const h = host();
  h.replies.push({ status: 204 });
  await h.fire('session.start');
  await h.fire('session.start');
  assert.equal(h.waiting().length, 1);
  await h.run();
  assert.deepEqual(h.requests, [{ url: 'http://cbc113/cbc/v1/wake?after=', init: { socketPath: SOCKET, method: 'GET' } }]);
  assert.equal(h.prompts.length, 0);
  assert.equal(h.waiting().length, 1);
  assert.equal(h.waiting()[0].delay, 1000);
  assert.deepEqual(h.reads, ['/private/tmp/cbc113/claude-project/.cbc-candidate-wake.json']);
});

test('opaque progress can move down; duplicate cursor never queues another hint', async () => {
  const h = host();
  h.replies.push(pending(A), pending(A), pending(B));
  await h.fire('session.start');
  await h.run(); await h.run(); await h.run();
  assert.equal(h.prompts.length, 2);
  assert.equal(h.requests[1].url, 'http://cbc113/cbc/v1/wake?after=' + A);
  assert.equal(h.requests[2].url, 'http://cbc113/cbc/v1/wake?after=' + A);
  assert.deepEqual(Object.keys(h.prompts[0]).sort(), ['asUser', 'text']);
  assert.equal(h.prompts[0].asUser, false);
  assert.match(h.prompts[0].text, /cbc_check with no arguments/);
  assert.match(h.prompts[0].text, /not human approval/);
  assert.equal(h.prompts[0].text.includes(A), false);
});

test('a recovery re-hints once even with an unchanged cursor', async () => {
  // Codex review 7: the driver re-hints an overdue item whose cursor has not moved.
  const h = host();
  h.replies.push(pending(A), recovery(A), pending(A));
  await h.fire('session.start');
  await h.run(); await h.run(); await h.run();
  assert.equal(h.prompts.length, 2, 'the recovery submits; the repeated pending does not');
  assert.equal(h.requests[2].url, 'http://cbc113/cbc/v1/wake?after=' + A);
  assert.equal(h.prompts[1].text, h.prompts[0].text);
});

test('no overlapping fetch or second prompt while native submission waits for idle', async () => {
  const h = host(), queued = deferred();
  h.setSubmit(() => queued.promise);
  h.replies.push(pending(A));
  await h.fire('session.start');
  const running = h.run();
  await tick();
  await h.fire('session.start');
  assert.equal(h.waiting().length, 0);
  assert.equal(h.requests.length, 1);
  assert.equal(h.prompts.length, 1);
  queued.resolve({});
  await running;
  assert.equal(h.waiting().length, 1);
});

test('transient statuses and connection errors back off without prompts', async () => {
  const h = host();
  h.replies.push({ status: 503 }, { status: 429 }, () => Promise.reject(new Error('offline')),
    ...Array.from({ length: 6 }, () => ({ status: 503 })));
  await h.fire('session.start');
  const delays = [];
  for (let i = 0; i < 9; i++) { await h.run(); delays.push(h.waiting()[0].delay); }
  assert.deepEqual(delays, [1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000, 30000]);
  assert.equal(h.prompts.length, 0);
});

test('invalid binding stops until accepted engine-stamped composer input', async () => {
  const h = host();
  h.replies.push({ status: 401 }, pending(A));
  await h.fire('session.start'); await h.run();
  assert.equal(h.waiting().length, 0);
  for (const kind of ['peer', 'channel', 'plugin', 'sdk', 'unclassified', 'bridge']) {
    await h.fire('prompt.submit', { origin: { kind } });
    assert.equal(h.waiting().length, 0);
  }
  await h.fire('prompt.submit', { origin: { kind: 'composer' } }, { drop: 'denied' });
  assert.equal(h.waiting().length, 0);
  await h.fire('prompt.submit', { origin: { kind: 'composer' } });
  await h.run();
  assert.equal(h.prompts.length, 1);
});

test('dropped or rejected prompt leaves unread work for human rearm', async () => {
  for (const failure of [() => Promise.resolve({ drop: 'stopped' }), () => Promise.reject(new Error('stopped'))]) {
    const h = host();
    h.setSubmit(failure);
    h.replies.push(pending(A), pending(A));
    await h.fire('session.start'); await h.run();
    assert.equal(h.waiting().length, 0);
    h.setSubmit(async () => ({}));
    await h.fire('prompt.submit', { origin: { kind: 'composer' } });
    await h.run();
    assert.equal(h.prompts.length, 2);
    assert.equal(h.requests[1].url, 'http://cbc113/cbc/v1/wake?after=');
  }
});

test('user interruption ignores late fetch; subagent interruption does not stop main', async () => {
  const h = host(), request = deferred();
  h.replies.push(() => request.promise);
  await h.fire('session.start');
  const running = h.run();
  await h.fire('turn.complete', { isAborted: true, agentId: 'child' });
  await h.fire('turn.complete', { isAborted: true });
  request.resolve(pending(A));
  await running;
  assert.equal(h.prompts.length, 0);
  assert.equal(h.waiting().length, 0);
});

test('session end cancels pending timer and ignores a late queued prompt', async () => {
  const idle = host();
  await idle.fire('session.start'); await idle.fire('session.end');
  assert.equal(idle.waiting().length, 0);
  await idle.fire('prompt.submit', { origin: { kind: 'plugin' } });
  assert.equal(idle.waiting().length, 0);
  await idle.fire('prompt.submit', { origin: { kind: 'composer' } });
  assert.equal(idle.waiting().length, 1);
  const h = host(), queued = deferred();
  h.setSubmit(() => queued.promise); h.replies.push(pending(A));
  await h.fire('session.start'); const running = h.run(); await tick();
  await h.fire('session.end'); queued.resolve({}); await running;
  assert.equal(h.waiting().length, 0);
});

test('malformed, oversized or body-bearing responses fail closed', async () => {
  for (const text of ['not json', 'null', '{}', ' '.repeat(257), JSON.stringify({ v: 1, type: 'pending', cursor: A, body: 'untrusted' }),
    JSON.stringify({ v: 1, type: 'pending', cursor: 'A'.repeat(64) }), JSON.stringify({ v: 1, type: 'pending', cursor: '' })]) {
    const h = host(); h.replies.push({ status: 200, text });
    await h.fire('session.start'); await h.run();
    assert.equal(h.prompts.length, 0); assert.equal(h.waiting().length, 0);
  }
});

test('no ambient/default/relative socket or too-long Unix path', async () => {
  for (const socketPath of [undefined, '', 'device.sock', '/bad\0socket', '/' + 'x'.repeat(103), '/' + 'é'.repeat(52)]) {
    const h = host({ socketPath }); await h.fire('session.start');
    assert.equal(h.waiting().length, 0);
  }
});

test('local configuration is exact, small, re-read on human rearm only', async () => {
  for (const raw of ['invalid', 'null', '{}', ' '.repeat(4097),
    JSON.stringify({ v: 1, socketPath: SOCKET, token: 'forbidden' }),
    JSON.stringify({ v: 2, socketPath: SOCKET })]) {
    const h = host(); h.setConfig(raw); await h.fire('session.start');
    assert.equal(h.waiting().length, 0);
  }
  const h = host(); h.replies.push({ status: 204 }, { status: 401 }, { status: 204 });
  await h.fire('session.start'); await h.run(); await h.run();
  assert.equal(h.reads.length, 1);
  await h.fire('prompt.submit', { origin: { kind: 'peer' } });
  assert.equal(h.reads.length, 1);
  await h.fire('prompt.submit', { origin: { kind: 'composer' } });
  assert.equal(h.reads.length, 2);
  await h.run();
});
