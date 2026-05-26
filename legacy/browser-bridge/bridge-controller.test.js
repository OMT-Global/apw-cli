import test from 'node:test';
import assert from 'node:assert/strict';

import {
  BridgeController,
  DEFAULT_SETTINGS,
  NATIVE_HOST_NAME,
} from './bridge-controller.js';

class FakeChromeEvent {
  constructor() {
    this.listeners = [];
  }

  addListener(listener) {
    this.listeners.push(listener);
  }

  emit(payload) {
    for (const listener of this.listeners) {
      listener(payload);
    }
  }
}

class FakeNativePort {
  constructor() {
    this.sent = [];
    this.onMessage = new FakeChromeEvent();
    this.onDisconnect = new FakeChromeEvent();
    this.disconnected = false;
  }

  postMessage(payload) {
    this.sent.push(payload);
  }

  disconnect() {
    this.disconnected = true;
  }
}

class FakeWebSocket {
  constructor(url) {
    this.url = url;
    this.readyState = 0;
    this.sent = [];
    this.listeners = {
      open: [],
      message: [],
      close: [],
      error: [],
    };
  }

  addEventListener(type, listener) {
    this.listeners[type].push(listener);
  }

  emit(type, payload = {}) {
    for (const listener of this.listeners[type]) {
      listener(payload);
    }
  }

  open() {
    this.readyState = 1;
    this.emit('open');
  }

  send(payload) {
    this.sent.push(JSON.parse(payload));
  }

  receive(message) {
    this.emit('message', {
      data: JSON.stringify(message),
    });
  }

  close() {
    this.readyState = 3;
    this.emit('close');
  }
}

function makeControllerHarness() {
  const sockets = [];
  const nativePorts = [];
  const timers = [];

  const controller = new BridgeController({
    nativeHostName: NATIVE_HOST_NAME,
    nativeConnect: () => {
      const port = new FakeNativePort();
      nativePorts.push(port);
      return port;
    },
    createWebSocket: (url) => {
      const socket = new FakeWebSocket(url);
      sockets.push(socket);
      return socket;
    },
    settingsStore: {
      async load() {
        return DEFAULT_SETTINGS;
      },
      async save() {},
    },
    schedule: (fn, delay) => {
      const timer = {
        fn,
        delay,
      };
      timers.push(timer);
      return timer;
    },
    clearScheduled: (timer) => {
      const index = timers.indexOf(timer);
      if (index >= 0) {
        timers.splice(index, 1);
      }
    },
    getLastNativeError: () => 'Native host crashed.',
    logger: {
      error() {},
    },
  });

  return {
    controller,
    sockets,
    nativePorts,
    timers,
  };
}

test('bridge controller queues requests, preserves request ids, and reconnects after websocket close', async () => {
  const harness = makeControllerHarness();
  await harness.controller.start();

  assert.equal(harness.sockets.length, 1);
  const socket = harness.sockets[0];
  assert.equal(socket.url, 'ws://127.0.0.1:10000');

  socket.open();
  assert.equal(socket.sent[0].type, 'hello');
  assert.equal(socket.sent[0].browser, 'chrome');

  socket.receive({
    type: 'request',
    requestId: 'req-1',
    payload: { cmd: 14 },
  });
  socket.receive({
    type: 'request',
    requestId: 'req-2',
    payload: { cmd: 4 },
  });

  assert.equal(harness.nativePorts.length, 1);
  const nativePort = harness.nativePorts[0];
  assert.deepEqual(nativePort.sent, [{ cmd: 14 }]);

  nativePort.onMessage.emit({ ok: true, payload: 'first' });
  assert.deepEqual(socket.sent[1], {
    type: 'response',
    requestId: 'req-1',
    ok: true,
    payload: { ok: true, payload: 'first' },
  });
  assert.deepEqual(nativePort.sent, [{ cmd: 14 }, { cmd: 4 }]);

  nativePort.onMessage.emit({ ok: true, payload: 'second' });
  assert.deepEqual(socket.sent[2], {
    type: 'response',
    requestId: 'req-2',
    ok: true,
    payload: { ok: true, payload: 'second' },
  });

  socket.close();
  assert.equal(harness.timers.length, 1);
  harness.timers[0].fn();

  assert.equal(harness.sockets.length, 2);
  harness.sockets[1].open();
  assert.equal(harness.sockets[1].sent[0].type, 'hello');
});

test('bridge controller reports native disconnects and fails pending requests deterministically', async () => {
  const harness = makeControllerHarness();
  await harness.controller.start();
  const socket = harness.sockets[0];
  socket.open();

  socket.receive({
    type: 'request',
    requestId: 'req-1',
    payload: { cmd: 14 },
  });
  socket.receive({
    type: 'request',
    requestId: 'req-2',
    payload: { cmd: 4 },
  });

  const nativePort = harness.nativePorts[0];
  nativePort.onDisconnect.emit();

  assert.deepEqual(socket.sent.slice(1), [
    {
      type: 'status',
      status: 'error',
      error: 'Native host crashed.',
    },
    {
      type: 'response',
      requestId: 'req-1',
      ok: false,
      code: 103,
      error: 'Native host crashed.',
    },
    {
      type: 'response',
      requestId: 'req-2',
      ok: false,
      code: 103,
      error: 'Native host crashed.',
    },
  ]);
});

test('bridge controller forwards helper envelopes verbatim and rejects malformed native payloads', async () => {
  const harness = makeControllerHarness();
  await harness.controller.start();
  const socket = harness.sockets[0];
  socket.open();

  socket.receive({
    type: 'request',
    requestId: 'req-ok',
    payload: { cmd: 14 },
  });
  const nativePort = harness.nativePorts[0];
  nativePort.onMessage.emit({
    ok: false,
    code: 103,
    error: 'Helper process not running.',
  });

  assert.deepEqual(socket.sent[1], {
    type: 'response',
    requestId: 'req-ok',
    ok: true,
    payload: {
      ok: false,
      code: 103,
      error: 'Helper process not running.',
    },
  });

  socket.receive({
    type: 'request',
    requestId: 'req-bad',
    payload: { cmd: 4 },
  });
  nativePort.onMessage.emit('bad-payload');

  assert.deepEqual(socket.sent[2], {
    type: 'response',
    requestId: 'req-bad',
    ok: false,
    code: 104,
    error: 'Native host returned malformed helper payload.',
  });
});
