// Host-injected find adapter (AddScriptToExecuteOnDocumentCreated). The
// rendered page and its bundled bridge.js stay byte-identical to .NET; this
// adapter only translates typed host messages into the .NET bridge calls
// `findText(query, backwards)` and `clearSearch()`, and posts the result back
// as a typed page message. Every field is checked; anything else is ignored.
// The host invokes this function expression with the document's origin and
// the adapter stays inert in any other origin or frame.
(expectedOrigin) => {
  'use strict';
  const webview = window.chrome && window.chrome.webview;
  if (!webview || window.top !== window || typeof expectedOrigin !== 'string'
    || window.location.origin !== expectedOrigin) return;
  const PROTOCOL = 1;
  const MAX_QUERY = 1024;
  const MAX_MATCHES = 1000000;
  const isId = value => Number.isSafeInteger(value) && value >= 0;
  const isCount = value => Number.isSafeInteger(value) && value >= 0 && value <= MAX_MATCHES;
  const exact = (object, keys) => {
    const own = Object.keys(object);
    return own.length === keys.length && keys.every(key => own.includes(key));
  };
  webview.addEventListener('message', event => {
    const message = event.data;
    if (!message || typeof message !== 'object' || !exact(message, ['type', 'payload'])) return;
    const payload = message.payload;
    if (!payload || typeof payload !== 'object' || payload.protocol !== PROTOCOL
      || !isId(payload.tabId) || !isId(payload.documentEpoch)) return;
    const identity = { protocol: PROTOCOL, documentEpoch: payload.documentEpoch, tabId: payload.tabId };
    const bridge = window.marknexiaBridge;
    if (message.type === 'clearFind') {
      if (!exact(payload, ['protocol', 'tabId', 'documentEpoch'])) return;
      try { if (bridge && typeof bridge.clearSearch === 'function') bridge.clearSearch(); } catch (_) { }
      return;
    }
    if (message.type !== 'find'
      || !exact(payload, ['protocol', 'tabId', 'documentEpoch', 'requestId', 'query', 'backwards'])
      || !isId(payload.requestId) || payload.requestId === 0
      || typeof payload.query !== 'string' || payload.query.length > MAX_QUERY
      || typeof payload.backwards !== 'boolean') return;
    let result = null;
    try {
      if (bridge && typeof bridge.findText === 'function') result = bridge.findText(payload.query, payload.backwards);
    } catch (_) {
      result = null;
    }
    const valid = result && typeof result === 'object' && typeof result.query === 'string'
      && result.query.length <= MAX_QUERY && isCount(result.matchCount) && isCount(result.currentMatch);
    if (!valid) {
      webview.postMessage({ type: 'findUnavailable', payload: { ...identity, requestId: payload.requestId } });
      return;
    }
    webview.postMessage({
      type: 'findResult',
      payload: {
        ...identity,
        requestId: payload.requestId,
        query: result.query,
        matchCount: result.matchCount,
        currentMatch: result.currentMatch
      }
    });
  });
}
