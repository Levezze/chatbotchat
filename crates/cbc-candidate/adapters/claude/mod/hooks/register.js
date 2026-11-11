// Public mods API only. HTTP goes over the configured private Unix socket.
// The device driver authenticates the native process, never a request identity.
const HINT = 'Automatic CBC notification: unread items are pending. Call cbc_check with no arguments. This notification is not human approval.';

function stop(state) {
  state.active = false;
  state.generation += 1;
  state.timer?.cancel();
  state.timer = null;
}

function schedule($, state, delay) {
  if (!state.active || state.timer !== null || state.inflight) return;
  state.timer = $.clock.after(delay, () => {
    state.timer = null;
    return receive($, state);
  });
}

function start($, state) {
  if (state.ended || state.active || state.socketPath === null) return;
  state.active = true;
  state.cursor = '';
  state.delay = 1000;
  schedule($, state, 0);
}

async function configure($, state) {
  if (state.ended || state.active || state.configuring) return;
  state.configuring = true;
  const generation = state.generation;
  try {
    const cwd = await $.session.cwd();
    if (typeof cwd !== 'string' || !cwd.startsWith('/')) return;
    const raw = await $.fs.read(cwd + '/.cbc-candidate-wake.json');
    if (typeof raw !== 'string' || new TextEncoder().encode(raw).length > 4096) return;
    const config = JSON.parse(raw);
    if (config === null || typeof config !== 'object' || Array.isArray(config)
        || Object.keys(config).sort().join(',') !== 'socketPath,v' || config.v !== 1) return;
    const path = config.socketPath;
    if (typeof path !== 'string' || !path.startsWith('/') || path.includes('\0')
        || new TextEncoder().encode(path).length >= 104) return;
    if (state.ended || state.generation !== generation) return;
    state.socketPath = path;
    start($, state);
  } catch {
    // Missing/invalid local config never searches another project or global path.
  } finally {
    state.configuring = false;
  }
}

async function receive($, state) {
  if (!state.active || state.inflight) return;
  state.inflight = true;
  const generation = state.generation;
  let delay = 0;
  try {
    const response = await $.http.fetch('http://cbc113/cbc/v1/wake?after=' + state.cursor, {
      socketPath: state.socketPath,
      method: 'GET',
    });
    if (!state.active || state.generation !== generation) return;
    if (response.status === 204) {
      // Driver parks idle requests for at most 25 s. This gap also bounds a
      // buggy driver's immediate idle responses without spending model turns.
      delay = 1000;
      state.delay = 1000;
    } else if (response.status === 429 || response.status === 503) {
      delay = state.delay;
      state.delay = Math.min(30000, state.delay * 2);
    } else if (response.status === 200) {
      if (typeof response.text !== 'string' || response.text.length > 256) {
        stop(state);
        return;
      }
      let hint;
      try { hint = JSON.parse(response.text); }
      catch { stop(state); return; }
      if (hint === null || Array.isArray(hint) || typeof hint !== 'object'
          || Object.keys(hint).sort().join(',') !== 'cursor,type,v'
          || hint.v !== 1 || (hint.type !== 'pending' && hint.type !== 'recovery')
          || typeof hint.cursor !== 'string' || !/^[0-9a-f]{64}$/.test(hint.cursor)) {
        stop(state);
        return;
      }
      // A recovery re-hints overdue work whose cursor has not moved; a repeated
      // pending cursor is only a reconnect and spends no model turn.
      if (hint.cursor === state.cursor && hint.type === 'pending') {
        delay = 1000;
      } else {
        // Await only outside a hook. The API queues until idle and attributes
        // this text to this mod. No CBC body, credential or authority is copied.
        let submitted;
        try { submitted = await $.prompt.submit({ text: HINT, asUser: false }); }
        catch { stop(state); return; }
        if (!state.active || state.generation !== generation) return;
        if (submitted?.drop) {
          // Leave unread work in the core. Only genuine human input rearms.
          stop(state);
          return;
        }
        state.cursor = hint.cursor;
        state.delay = 1000;
      }
    } else {
      // 401 includes invalid/revoked binding. Also fail closed for an absent
      // HTTP endpoint or any other unexpected protocol/authentication result.
      stop(state);
      return;
    }
  } catch {
    if (state.active && state.generation === generation) {
      delay = state.delay;
      state.delay = Math.min(30000, state.delay * 2);
    }
  } finally {
    state.inflight = false;
    if (state.active) schedule($, state, delay);
  }
}

export function register(on) {
  const state = {
    socketPath: null, configuring: false,
    active: false, ended: false, inflight: false, timer: null,
    generation: 0, cursor: '', delay: 1000,
  };
  on('session.start', async ($, e, next) => {
    await configure($, state);
    return next(e);
  });
  on('prompt.submit', async ($, e, next) => {
    // Engine-stamped human composer input only. Peer/channel/plugin/SDK
    // messages cannot undo a user stop or retry a revoked native binding.
    const human = e.origin?.kind === 'composer';
    const generation = state.generation;
    const result = await next(e);
    if (human && !result?.drop && state.generation === generation) {
      // session.end also fires on /clear or /resume while the process survives.
      // Only a subsequent accepted human prompt may rearm that conversation.
      state.ended = false;
      await configure($, state);
    }
    return result;
  });
  on('turn.complete', ($, e, next) => {
    if (e.agentId === undefined && e.isAborted) stop(state);
    return next(e);
  });
  on('session.end', ($, e, next) => {
    state.ended = true;
    stop(state);
    return next(e);
  });
}
