(() => {
  const root = document.body;
  const tabId = Number(root.dataset.tabId);
  const documentEpoch = Number(root.dataset.documentEpoch);
  const send = (type, fields = {}) => window.chrome.webview.postMessage({
    type,
    payload: { protocol: 1, tabId, documentEpoch, ...fields },
  });
  send('ready');
  document.querySelector('a').addEventListener('click', (event) => {
    event.preventDefault();
    send('openLink', { href: event.currentTarget.href });
  });
  document.querySelector('#copy').addEventListener('click', () => {
    send('copyText', { text: 'Marknexia probe' });
  });
  window.addEventListener('focus', () => send('focusChanged', { focused: true }));
  window.addEventListener('blur', () => send('focusChanged', { focused: false }));
})();
