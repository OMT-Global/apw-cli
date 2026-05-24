import {
  BridgeController,
  DEFAULT_SETTINGS,
  NATIVE_HOST_NAME,
} from './bridge-controller.js';

let latestState = {
  browser: 'chrome',
  host: DEFAULT_SETTINGS.host,
  port: DEFAULT_SETTINGS.port,
  websocket: 'idle',
  nativeHost: 'idle',
  queueDepth: 0,
  lastError: null,
};

const settingsStore = {
  async load() {
    const stored = await chrome.storage.local.get(DEFAULT_SETTINGS);
    return {
      ...DEFAULT_SETTINGS,
      ...stored,
      port: Number(stored.port ?? DEFAULT_SETTINGS.port),
    };
  },
  async save(settings) {
    await chrome.storage.local.set({
      host: settings.host,
      port: Number(settings.port),
    });
  },
};

const controller = new BridgeController({
  nativeHostName: NATIVE_HOST_NAME,
  browserName: 'chrome',
  browserVersion: navigator.userAgent,
  nativeConnect: (hostName) => chrome.runtime.connectNative(hostName),
  createWebSocket: (url) => new WebSocket(url),
  settingsStore,
  getLastNativeError: () => chrome.runtime.lastError?.message ?? null,
  onStateChange: (state) => {
    latestState = state;
  },
});

async function initializeController() {
  try {
    await controller.start();
  } catch (error) {
    latestState = {
      ...latestState,
      websocket: 'error',
      lastError: error.message,
    };
  }
}

initializeController();

chrome.runtime.onStartup.addListener(() => {
  initializeController();
});

chrome.runtime.onInstalled.addListener(() => {
  initializeController();
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === 'popup:getState') {
    sendResponse(latestState);
    return;
  }

  if (message?.type === 'popup:updateSettings') {
    controller
      .updateSettings({
        host: message.host,
        port: Number(message.port),
      })
      .then((state) => sendResponse(state))
      .catch((error) =>
        sendResponse({
          ...latestState,
          lastError: error.message,
        }),
      );
    return true;
  }
});
