export const NATIVE_HOST_NAME = 'dev.omt.apw.bridge.chromium';
export const DEFAULT_SETTINGS = {
  host: '127.0.0.1',
  port: 10000,
};

const RESPONSE_CODE_PROCESS_NOT_RUNNING = 103;
const RESPONSE_CODE_PROTO_INVALID_RESPONSE = 104;

function makeStatusError(message) {
  return {
    code: RESPONSE_CODE_PROCESS_NOT_RUNNING,
    error: message,
    ok: false,
  };
}

function isObject(value) {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

export class BridgeController {
  constructor({
    nativeHostName = NATIVE_HOST_NAME,
    browserName = 'chrome',
    browserVersion = 'extension',
    nativeConnect,
    createWebSocket,
    settingsStore,
    schedule = globalThis.setTimeout.bind(globalThis),
    clearScheduled = globalThis.clearTimeout.bind(globalThis),
    logger = console,
    reconnectDelayMs = 750,
    getLastNativeError = () => null,
    onStateChange = () => {},
  }) {
    this.nativeHostName = nativeHostName;
    this.browserName = browserName;
    this.browserVersion = browserVersion;
    this.nativeConnect = nativeConnect;
    this.createWebSocket = createWebSocket;
    this.settingsStore = settingsStore;
    this.schedule = schedule;
    this.clearScheduled = clearScheduled;
    this.logger = logger;
    this.reconnectDelayMs = reconnectDelayMs;
    this.getLastNativeError = getLastNativeError;
    this.onStateChange = onStateChange;

    this.settings = { ...DEFAULT_SETTINGS };
    this.state = {
      browser: browserName,
      host: DEFAULT_SETTINGS.host,
      port: DEFAULT_SETTINGS.port,
      websocket: 'idle',
      nativeHost: 'idle',
      queueDepth: 0,
      lastError: null,
    };

    this.websocket = null;
    this.nativePort = null;
    this.pendingQueue = [];
    this.currentRequest = null;
    this.reconnectTimer = null;
    this.started = false;
  }

  async start() {
    if (this.started) {
      return this.getState();
    }

    this.started = true;
    const stored = await this.settingsStore.load();
    this.settings = {
      ...DEFAULT_SETTINGS,
      ...stored,
    };
    this.updateState({
      host: this.settings.host,
      port: this.settings.port,
    });
    this.connectWebSocket();
    return this.getState();
  }

  stop() {
    this.started = false;
    this.cancelReconnect();
    this.disconnectNative(null);
    this.disconnectWebSocket();
  }

  async updateSettings(nextSettings) {
    this.settings = {
      ...this.settings,
      ...nextSettings,
    };
    await this.settingsStore.save(this.settings);
    this.updateState({
      host: this.settings.host,
      port: this.settings.port,
      lastError: null,
    });
    this.disconnectWebSocket();
    this.connectWebSocket();
    return this.getState();
  }

  getState() {
    return {
      ...this.state,
    };
  }

  updateState(patch) {
    this.state = {
      ...this.state,
      ...patch,
      queueDepth: this.pendingQueue.length + (this.currentRequest ? 1 : 0),
    };
    this.onStateChange(this.getState());
  }

  connectWebSocket() {
    if (!this.started || this.websocket) {
      return;
    }

    let websocket;
    const url = `ws://${this.settings.host}:${this.settings.port}`;
    try {
      websocket = this.createWebSocket(url);
    } catch (error) {
      const message = `Failed to create daemon WebSocket: ${error.message}`;
      this.updateState({
        websocket: 'error',
        lastError: message,
      });
      this.scheduleReconnect();
      return;
    }

    this.websocket = websocket;
    this.updateState({
      websocket: 'connecting',
      lastError: null,
    });

    websocket.addEventListener('open', () => {
      this.cancelReconnect();
      this.updateState({
        websocket: 'connected',
        lastError: null,
      });
      this.sendWebSocket({
        type: 'hello',
        browser: this.browserName,
        version: this.browserVersion,
      });
      this.flushQueue();
    });

    websocket.addEventListener('message', (event) => {
      this.handleWebSocketMessage(event.data);
    });

    websocket.addEventListener('error', () => {
      this.updateState({
        websocket: 'error',
        lastError: 'Daemon bridge WebSocket reported an error.',
      });
    });

    websocket.addEventListener('close', () => {
      this.websocket = null;
      this.updateState({
        websocket: 'disconnected',
      });
      this.failCurrentAndQueue(
        'Daemon bridge WebSocket disconnected before requests completed.',
      );
      this.scheduleReconnect();
    });
  }

  disconnectWebSocket() {
    const websocket = this.websocket;
    this.websocket = null;
    if (websocket && websocket.readyState < 2) {
      websocket.close();
    }
  }

  scheduleReconnect() {
    if (!this.started || this.reconnectTimer) {
      return;
    }
    this.reconnectTimer = this.schedule(() => {
      this.reconnectTimer = null;
      this.connectWebSocket();
    }, this.reconnectDelayMs);
  }

  cancelReconnect() {
    if (!this.reconnectTimer) {
      return;
    }
    this.clearScheduled(this.reconnectTimer);
    this.reconnectTimer = null;
  }

  handleWebSocketMessage(raw) {
    let parsed;
    try {
      parsed = JSON.parse(raw);
    } catch (_error) {
      this.sendStatus(
        'error',
        'Browser bridge received malformed daemon JSON.',
      );
      return;
    }

    if (!isObject(parsed) || parsed.type !== 'request' || !parsed.requestId) {
      this.sendStatus(
        'error',
        'Browser bridge received malformed daemon request envelope.',
      );
      return;
    }

    this.pendingQueue.push({
      requestId: parsed.requestId,
      payload: parsed.payload,
    });
    this.updateState({});
    this.flushQueue();
  }

  connectNative() {
    if (this.nativePort) {
      return this.nativePort;
    }

    try {
      this.nativePort = this.nativeConnect(this.nativeHostName);
    } catch (error) {
      const message = `Failed to connect native host ${this.nativeHostName}: ${error.message}`;
      this.updateState({
        nativeHost: 'error',
        lastError: message,
      });
      this.sendStatus('error', message);
      return null;
    }

    this.nativePort.onMessage.addListener((message) => {
      this.handleNativeMessage(message);
    });
    this.nativePort.onDisconnect.addListener(() => {
      const runtimeError = this.getLastNativeError();
      this.disconnectNative(runtimeError);
    });

    this.updateState({
      nativeHost: 'connected',
      lastError: null,
    });
    return this.nativePort;
  }

  disconnectNative(runtimeErrorMessage) {
    if (this.nativePort) {
      try {
        this.nativePort.disconnect();
      } catch (_error) {
        // Chrome ports throw when disconnecting twice; the state reset below is sufficient.
      }
    }
    this.nativePort = null;

    const message =
      runtimeErrorMessage ||
      'Native host disconnected before the helper response completed.';
    this.updateState({
      nativeHost: runtimeErrorMessage ? 'error' : 'disconnected',
      lastError: runtimeErrorMessage || this.state.lastError,
    });
    if (runtimeErrorMessage) {
      this.sendStatus('error', message);
    }
    this.failCurrentAndQueue(message);
  }

  flushQueue() {
    if (this.currentRequest || this.pendingQueue.length === 0) {
      this.updateState({});
      return;
    }

    if (!this.websocket || this.websocket.readyState !== 1) {
      this.updateState({});
      return;
    }

    const nativePort = this.connectNative();
    if (!nativePort) {
      this.failCurrentAndQueue(
        `Failed to connect native host ${this.nativeHostName}.`,
      );
      return;
    }

    this.currentRequest = this.pendingQueue.shift();
    this.updateState({});

    try {
      nativePort.postMessage(this.currentRequest.payload);
    } catch (error) {
      const message = `Native host write failed: ${error.message}`;
      this.sendStatus('error', message);
      this.failCurrentAndQueue(message);
    }
  }

  handleNativeMessage(message) {
    if (!this.currentRequest) {
      this.sendStatus(
        'error',
        'Native host returned an unexpected response with no pending daemon request.',
      );
      return;
    }

    const current = this.currentRequest;
    this.currentRequest = null;

    if (!isObject(message)) {
      this.sendResponse({
        requestId: current.requestId,
        ...makeStatusError('Native host returned malformed helper payload.'),
        code: RESPONSE_CODE_PROTO_INVALID_RESPONSE,
      });
      this.flushQueue();
      return;
    }

    this.sendResponse({
      requestId: current.requestId,
      ok: true,
      payload: message,
    });
    this.flushQueue();
  }

  failCurrentAndQueue(message) {
    const rejected = [];
    if (this.currentRequest) {
      rejected.push(this.currentRequest);
      this.currentRequest = null;
    }
    rejected.push(...this.pendingQueue);
    this.pendingQueue = [];

    for (const pending of rejected) {
      this.sendResponse({
        requestId: pending.requestId,
        ...makeStatusError(message),
      });
    }

    this.updateState({});
  }

  sendStatus(status, error) {
    this.sendWebSocket({
      type: 'status',
      status,
      error,
    });
  }

  sendResponse(response) {
    this.sendWebSocket({
      type: 'response',
      ...response,
    });
  }

  sendWebSocket(message) {
    if (!this.websocket || this.websocket.readyState !== 1) {
      return;
    }

    try {
      this.websocket.send(JSON.stringify(message));
    } catch (error) {
      this.logger.error('failed to write browser bridge websocket', error);
    }
  }
}
