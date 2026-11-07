// Host-injected link adapter (AddScriptToExecuteOnDocumentCreated). The
// rendered page and its bundled bridge.js are not changed for links. The
// .NET bridge posts an untyped {type:'openLink', href} message that the
// strict Rust protocol rejects; this adapter takes link clicks first, in the
// capture phase on the window, stops them before bridge.js sees them, and
// posts the typed message with the document's identity instead.
// It also reports the hovered or focused link, the scroll offset (for back
// and forward) and the mouse back and forward buttons. The host invokes this
// function expression with the document's origin, tab id and epoch, and the
// adapter stays inert in any other origin or frame.
(expectedOrigin, tabId, documentEpoch) => {
  'use strict';
  const webview = window.chrome && window.chrome.webview;
  if (!webview || window.top !== window || typeof expectedOrigin !== 'string'
    || window.location.origin !== expectedOrigin
    || !Number.isSafeInteger(tabId) || tabId < 0
    || !Number.isSafeInteger(documentEpoch) || documentEpoch < 0) return;
  const PROTOCOL = 1;
  const MAX_HREF_BYTES = 8192;
  const encoder = new TextEncoder();
  const post = (type, fields) => {
    try {
      webview.postMessage({ type, payload: { protocol: PROTOCOL, tabId, documentEpoch, ...fields } });
    } catch (_) { }
  };
  // The raw href attribute of the link that contains `node`, as bridge.js
  // reads it; null for no link, an empty href or one over the size limit.
  const linkHref = node => {
    const element = node && node.nodeType === 1 ? node : node && node.parentElement;
    const link = element && typeof element.closest === 'function' ? element.closest('a[href]') : null;
    if (!link) return null;
    const href = link.getAttribute('href');
    if (typeof href !== 'string' || href.trim() === '' || encoder.encode(href).length > MAX_HREF_BYTES) return null;
    return href;
  };
  const isBridgeButton = node => {
    const element = node && node.nodeType === 1 ? node : node && node.parentElement;
    return !!(element && typeof element.closest === 'function' && element.closest('button[data-marknexia-action]'));
  };

  // Scroll offset, reported once scrolling settles; flushed before a link
  // is followed so the history entry keeps the offset the reader left.
  let scrollTimer = 0;
  let reportedTop = -1;
  const reportScroll = () => {
    if (scrollTimer) { clearTimeout(scrollTimer); scrollTimer = 0; }
    const top = Math.max(0, Math.min(4294967295, Math.round(window.scrollY || 0)));
    if (top !== reportedTop) {
      reportedTop = top;
      post('scrollChanged', { top });
    }
  };
  window.addEventListener('scroll', () => {
    if (scrollTimer) clearTimeout(scrollTimer);
    scrollTimer = setTimeout(reportScroll, 150);
  }, { passive: true });

  // A primary click on a link, by mouse or by Enter on a focused link (the
  // browser raises a click for both). Ctrl, Shift and Alt clicks follow the
  // link the same way, as in .NET, whose bridge ignores the modifiers.
  window.addEventListener('click', event => {
    if (event.button !== 0 || isBridgeButton(event.target)) return;
    const href = linkHref(event.target);
    if (href === null) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    reportScroll();
    post('openLink', { href });
  }, true);
  // A middle click never opens anything (.NET opens nothing either).
  window.addEventListener('auxclick', event => {
    if (event.button === 1 && linkHref(event.target) !== null) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  }, true);

  // Mouse back (button 3) and forward (button 4).
  const historyButton = event => event.button === 3 || event.button === 4;
  for (const type of ['mousedown', 'mouseup', 'pointerdown', 'pointerup']) {
    window.addEventListener(type, event => {
      if (!historyButton(event)) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (type === 'mouseup') {
        reportScroll();
        post('historyStep', { back: event.button === 3 });
      }
    }, true);
  }

  // Hovered or focused link, for the shell's status bar.
  let shown = null;
  const show = href => {
    if (href === shown) return;
    shown = href;
    post('linkHover', { href });
  };
  document.addEventListener('mouseover', event => show(linkHref(event.target)), true);
  document.addEventListener('mouseout', event => {
    if (!event.relatedTarget) show(null);
  }, true);
  document.addEventListener('focusin', event => show(linkHref(event.target)), true);
  document.addEventListener('focusout', event => {
    if (!event.relatedTarget || linkHref(event.relatedTarget) === null) show(null);
  }, true);
}
