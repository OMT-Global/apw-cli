function updateView(state) {
  document.getElementById('host').value = state.host ?? '127.0.0.1';
  document.getElementById('port').value = Number(state.port ?? 10000);
  document.getElementById('websocket-status').textContent = state.websocket ?? 'idle';
  document.getElementById('native-status').textContent = state.nativeHost ?? 'idle';
  document.getElementById('queue-depth').textContent = String(state.queueDepth ?? 0);
  document.getElementById('last-error').textContent = state.lastError ?? '';
}

function readSettings() {
  return {
    host: document.getElementById('host').value.trim() || '127.0.0.1',
    port: Number(document.getElementById('port').value || 10000),
  };
}

chrome.runtime.sendMessage({ type: 'popup:getState' }, (state) => {
  updateView(state ?? {});
});

document.getElementById('save').addEventListener('click', () => {
  const settings = readSettings();
  chrome.runtime.sendMessage(
    {
      type: 'popup:updateSettings',
      ...settings,
    },
    (state) => {
      updateView(state ?? settings);
    },
  );
});
