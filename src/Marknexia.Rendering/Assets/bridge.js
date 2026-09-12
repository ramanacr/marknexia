window.marknexiaBridge = {
  openLink: function(href) {
    if (window.chrome && window.chrome.webview) {
      window.chrome.webview.postMessage({ type: 'openLink', href: href });
    }
  },
  copyText: async function(text) {
    if (window.chrome && window.chrome.webview) {
      window.chrome.webview.postMessage({ type: 'copyText', text: text });
    } else if (navigator.clipboard) {
      await navigator.clipboard.writeText(text);
    }
  },
  toggleSource: function(diagramId) {
    const srcEl = document.getElementById(diagramId + '-source');
    const viewEl = document.getElementById(diagramId + '-viewport');
    if (srcEl && viewEl) {
      if (srcEl.style.display === 'none') {
        srcEl.style.display = 'block';
        viewEl.style.display = 'none';
      } else {
        srcEl.style.display = 'none';
        viewEl.style.display = 'block';
      }
    }
  },
  scrollToAnchor: function(anchorId) {
    const safeAnchor = String(anchorId || '');
    const target = document.getElementById(safeAnchor) || document.querySelector('[name="' + CSS.escape(safeAnchor) + '"]');
    if (target) {
      target.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
  },
  scrollToTop: function() {
    window.scrollTo({ left: 0, top: 0, behavior: 'instant' });
  }
};

// Search the displayed document, not Markdown syntax or hidden diagram source.
// Ranges preserve markup and allow matches across inline formatting boundaries.
(() => {
  let root = null;
  let observer = null;
  let cached = null;

  function clearSelection() {
    window.getSelection()?.removeAllRanges();
    if (window.CSS?.highlights) CSS.highlights.delete('marknexia-search-current');
  }

  window.marknexiaBridge.clearSearch = () => {
    cached = null;
    clearSelection();
  };

  function revealRange(range) {
    const delta = (start, end, visibleStart, visibleEnd) => start < visibleStart
      ? start - visibleStart : end > visibleEnd ? Math.min(end - visibleEnd, start - visibleStart) : 0;
    // Scrolling the window does not reveal text clipped by a pre/table/diagram.
    // Reveal the first match rectangle from the innermost scroller outward.
    for (let ancestor = range.startContainer.parentElement; ancestor; ancestor = ancestor.parentElement) {
      if (ancestor === document.scrollingElement) continue;
      const style = getComputedStyle(ancestor);
      const bounds = ancestor.getBoundingClientRect();
      const rect = range.getClientRects()[0] || range.getBoundingClientRect();
      if (['auto', 'scroll', 'hidden'].includes(style.overflowX) && ancestor.scrollWidth > ancestor.clientWidth) {
        const left = bounds.left + ancestor.clientLeft;
        ancestor.scrollLeft += delta(rect.left, rect.right, left, left + ancestor.clientWidth);
      }
      if (['auto', 'scroll', 'hidden'].includes(style.overflowY) && ancestor.scrollHeight > ancestor.clientHeight) {
        const top = bounds.top + ancestor.clientTop;
        ancestor.scrollTop += delta(rect.top, rect.bottom, top, top + ancestor.clientHeight);
      }
    }
    const rect = range.getClientRects()[0] || range.getBoundingClientRect();
    window.scrollBy({ left: delta(rect.left, rect.right, 0, window.innerWidth),
      top: rect.top - window.innerHeight / 2, behavior: 'instant' });
  }

  function collectText() {
    const segments = [];
    let text = '';
    let boundary = false;
    let previousBlock;
    const styles = new WeakMap();
    const styleFor = element => {
      if (!styles.has(element)) styles.set(element, getComputedStyle(element));
      return styles.get(element);
    };
    const isBlock = element => {
      const display = styleFor(element).display;
      return display !== 'none' && display !== 'contents' && !display.startsWith('inline');
    };
    const containingBlock = element => {
      for (let ancestor = element; ancestor && ancestor !== root; ancestor = ancestor.parentElement)
        if (isBlock(ancestor)) return ancestor;
      return root;
    };
    const trimTrailingSpace = () => {
      const last = segments.at(-1);
      if (!last?.collapse || !text.endsWith(' ')) return;
      text = text.slice(0, -1);
      if (--last.end === last.start) segments.pop();
    };
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.nodeType === Node.ELEMENT_NODE) {
        if ((node.tagName === 'BR' || isBlock(node)) && node.getClientRects().length) boundary = true;
        continue;
      }
      const parent = node.parentElement;
      if (!parent || parent.closest('script,style,button,input,textarea,select,[hidden]')) continue;
      // Closed details can still report text rectangles. Only their first
      // summary is displayed; check every ancestor, including nested details.
      let collapsed = false;
      for (let ancestor = parent; ancestor && ancestor !== root; ancestor = ancestor.parentElement) {
        if (ancestor.tagName !== 'DETAILS' || ancestor.open) continue;
        const summary = [...ancestor.children].find(child => child.tagName === 'SUMMARY');
        if (!summary?.contains(parent)) { collapsed = true; break; }
      }
      if (collapsed) continue;
      if (styleFor(parent).visibility !== 'visible') continue;
      const visibleRange = document.createRange();
      visibleRange.selectNodeContents(node);
      if (![...visibleRange.getClientRects()].some(rect => rect.width > 0 && rect.height > 0)) continue;
      const block = containingBlock(parent);
      if (previousBlock !== undefined && previousBlock !== block) boundary = true;
      if (boundary) {
        trimTrailingSpace();
        if (text.length) text += '\n';
      }
      boundary = false;
      previousBlock = block;

      const whiteSpace = styleFor(parent).whiteSpace;
      const collapse = !['pre', 'pre-wrap', 'break-spaces'].includes(whiteSpace);
      const preserveLines = whiteSpace === 'pre-line';
      const raw = node.data;
      let displayed = raw;
      let starts = null;
      let ends = null;
      if (collapse && /[\t\n\r\f ]/.test(raw)) {
        displayed = '';
        starts = new Uint32Array(raw.length);
        ends = new Uint32Array(raw.length);
        const append = (character, from, to) => {
          starts[displayed.length] = from;
          ends[displayed.length] = to;
          displayed += character;
        };
        for (let index = 0; index < raw.length;) {
          const from = index;
          if (preserveLines && raw[index] === '\n') {
            if (displayed.endsWith(' ')) displayed = displayed.slice(0, -1);
            else if (!displayed) trimTrailingSpace();
            append('\n', index, ++index);
          } else if (/[\t\n\r\f ]/.test(raw[index])) {
            do { index++; }
            while (index < raw.length && /[\t\n\r\f ]/.test(raw[index])
              && !(preserveLines && raw[index] === '\n'));
            const previous = displayed.at(-1) ?? text.at(-1);
            const previousSpaceIsPreserved = !displayed && segments.at(-1)?.collapse === false;
            if (previous && previous !== '\n' && (previous !== ' ' || previousSpaceIsPreserved))
              append(' ', from, index);
          } else {
            append(raw[index], index, ++index);
          }
        }
        // Most text nodes need no mapping; retain offset arrays only where CSS
        // whitespace processing actually changed their rendered character stream.
        if (displayed === raw) { starts = null; ends = null; }
      }
      if (!displayed) continue;
      const start = text.length;
      text += displayed;
      segments.push({ node, start, end: text.length, starts, ends, collapse });
    }
    trimTrailingSpace();
    return { text, segments };
  }

  window.marknexiaBridge.findText = (value, backwards = false) => {
    const query = String(value ?? '');
    const currentRoot = document.querySelector('.markdown-body');
    if (root !== currentRoot) {
      observer?.disconnect();
      root = currentRoot;
      cached = null;
      if (root) {
        observer = new MutationObserver(() => { cached = null; });
        observer.observe(root, { subtree: true, childList: true, characterData: true, attributes: true,
          attributeFilter: ['style', 'class', 'hidden', 'open'] });
      }
    }
    if (!root || !query) {
      window.marknexiaBridge.clearSearch();
      return { query, matchCount: 0, currentMatch: 0 };
    }
    if (!cached || cached.query !== query) {
      const { text, segments } = collectText();
      const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      const matches = Array.from(text.matchAll(new RegExp(escaped, 'giu')), match => ({start: match.index, end: match.index + match[0].length}));
      cached = { query, segments, matches, position: backwards ? 0 : -1 };
    }
    clearSelection();
    const count = cached.matches.length;
    if (!count) return { query, matchCount: 0, currentMatch: 0 };
    cached.position = (cached.position + (backwards ? -1 : 1) + count) % count;
    const match = cached.matches[cached.position];
    const start = cached.segments.find(segment => segment.end > match.start);
    const end = cached.segments.find(segment => segment.end >= match.end);
    const range = document.createRange();
    const startOffset = match.start - start.start;
    const endOffset = match.end - end.start;
    range.setStart(start.node, start.starts ? start.starts[startOffset] : startOffset);
    range.setEnd(end.node, end.ends ? end.ends[endOffset - 1] : endOffset);
    window.getSelection()?.addRange(range);
    if (window.CSS?.highlights && window.Highlight) CSS.highlights.set('marknexia-search-current', new Highlight(range));
    revealRange(range);
    return { query, matchCount: count, currentMatch: cached.position + 1 };
  };
})();

document.addEventListener('DOMContentLoaded', () => {
  installImageFallbacks();

  // Intercept all <a> clicks
  document.body.addEventListener('click', (e) => {
    const button = e.target.closest('button[data-marknexia-action]');
    if (button) {
      e.preventDefault();
      const action = button.getAttribute('data-marknexia-action');
      if (action === 'copy') {
        try {
          const text = decodeURIComponent(button.getAttribute('data-copy-text') || '');
          window.marknexiaBridge.copyText(text).catch(err => console.error('Copy failed', err));
        } catch (err) {
          console.error('Invalid copy payload', err);
        }
      } else if (action === 'toggle-source') {
        const diagram = button.closest('.marknexia-mermaid');
        if (diagram) window.marknexiaBridge.toggleSource(diagram.id);
      } else if (action === 'zoom-in' || action === 'zoom-out' || action === 'zoom-reset') {
        const diagram = button.closest('.marknexia-mermaid');
        if (diagram) {
          if (action === 'zoom-reset') resetDiagramZoom(diagram);
          else adjustDiagramZoom(diagram, action === 'zoom-in' ? 1 : -1);
        }
      } else if (action === 'expand') {
        const diagram = button.closest('.marknexia-mermaid');
        if (diagram) toggleDiagramExpanded(diagram);
      }
      return;
    }
    const link = e.target.closest('a');
    if (link) {
      const href = link.getAttribute('href');
      if (href) {
        e.preventDefault();
        window.marknexiaBridge.openLink(href);
      }
    }
  });

  // Render Mermaid diagrams offline safely
  document.querySelectorAll('.marknexia-mermaid').forEach(installDiagramInteractions);
  if (typeof mermaid !== 'undefined') {
    try {
      const isDark = document.documentElement.getAttribute('data-theme') === 'dark';
      mermaid.initialize({
        startOnLoad: false,
        theme: isDark ? 'dark' : 'default',
        securityLevel: 'strict'
      });
      
      document.querySelectorAll('.marknexia-mermaid').forEach(container => {
        const id = container.id;
        const preEl = container.querySelector('.mermaid');
        if (preEl) {
          const code = preEl.textContent;
          try {
            mermaid.render(id + '-svg', code).then(res => {
              preEl.innerHTML = res.svg;
            }).catch(err => {
              showDiagramError(id, err);
            });
          } catch (err) {
            showDiagramError(id, err);
          }
        }
      });
    } catch (e) {
      console.error('Mermaid initialization failed', e);
    }
  }
});

const MIN_DIAGRAM_ZOOM = 0.25;
const MAX_DIAGRAM_ZOOM = 4;
const DIAGRAM_ZOOM_STEP = 0.25;

function diagramState(diagram) {
  if (!diagram._marknexiaZoomState) {
    diagram._marknexiaZoomState = { zoom: 1, panX: 0, panY: 0, pointerId: null, lastX: 0, lastY: 0 };
  }
  return diagram._marknexiaZoomState;
}

function clampDiagramZoom(value) {
  return Math.min(MAX_DIAGRAM_ZOOM, Math.max(MIN_DIAGRAM_ZOOM, value));
}

function updateDiagramZoomUi(diagram) {
  const state = diagramState(diagram);
  const canvas = diagram.querySelector('.marknexia-diagram-canvas');
  const viewport = diagram.querySelector('.marknexia-diagram-viewport');
  const status = diagram.querySelector('[data-marknexia-zoom-status]');
  if (canvas) canvas.style.transform = `translate(${state.panX}px, ${state.panY}px) scale(${state.zoom})`;
  const percent = `${Math.round(state.zoom * 100)}%`;
  if (status) status.textContent = percent;
  const reset = diagram.querySelector('[data-marknexia-action="zoom-reset"]');
  if (reset) reset.textContent = percent;
  if (viewport) viewport.classList.toggle('is-zoomed', state.zoom !== 1 || state.panX !== 0 || state.panY !== 0);
}

function adjustDiagramZoom(diagram, direction) {
  const state = diagramState(diagram);
  const next = clampDiagramZoom(state.zoom + direction * DIAGRAM_ZOOM_STEP);
  if (next === state.zoom) return;
  state.zoom = next;
  if (state.zoom === 1) { state.panX = 0; state.panY = 0; }
  updateDiagramZoomUi(diagram);
}

function resetDiagramZoom(diagram) {
  const state = diagramState(diagram);
  state.zoom = 1;
  state.panX = 0;
  state.panY = 0;
  updateDiagramZoomUi(diagram);
}

function toggleDiagramExpanded(diagram) {
  const expanded = !diagram.classList.contains('is-expanded');
  const viewport = diagram.querySelector('.marknexia-diagram-viewport');
  const button = diagram.querySelector('[data-marknexia-action="expand"]');
  const state = diagramState(diagram);
  if (!viewport || !button) return;

  if (expanded) {
    document.querySelectorAll('.marknexia-mermaid.is-expanded').forEach(other => {
      if (other !== diagram) toggleDiagramExpanded(other);
    });
    state.previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }

  diagram.classList.toggle('is-expanded', expanded);
  document.documentElement.classList.toggle('marknexia-diagram-expanded', expanded);
  if (expanded) {
    viewport.setAttribute('aria-modal', 'true');
    viewport.setAttribute('aria-label', 'Expanded Mermaid diagram. Use zoom controls, mouse wheel, or drag to explore.');
    button.textContent = '×';
    button.setAttribute('aria-label', 'Close diagram full window');
    button.title = 'Close full-window diagram';
    viewport.focus();
  } else {
    viewport.removeAttribute('aria-modal');
    viewport.setAttribute('aria-label', 'Mermaid diagram. Use zoom controls, mouse wheel, or drag to explore.');
    button.textContent = '⛶';
    button.setAttribute('aria-label', 'Open diagram full window');
    button.title = 'Open full-window diagram';
    if (state.previousFocus instanceof HTMLElement) state.previousFocus.focus();
    state.previousFocus = null;
  }
}

function installDiagramInteractions(diagram) {
  if (diagram.dataset.marknexiaZoomReady === 'true') return;
  diagram.dataset.marknexiaZoomReady = 'true';
  const viewport = diagram.querySelector('.marknexia-diagram-viewport');
  if (!viewport) return;
  viewport.tabIndex = 0;
  viewport.setAttribute('role', 'application');
  viewport.setAttribute('aria-label', 'Mermaid diagram. Use zoom controls, mouse wheel, or drag to explore.');
  updateDiagramZoomUi(diagram);

  viewport.addEventListener('wheel', event => {
    event.preventDefault();
    const state = diagramState(diagram);
    const oldZoom = state.zoom;
    const nextZoom = clampDiagramZoom(oldZoom + (event.deltaY < 0 ? DIAGRAM_ZOOM_STEP : -DIAGRAM_ZOOM_STEP));
    if (nextZoom === oldZoom) return;
    const bounds = viewport.getBoundingClientRect();
    const anchorX = event.clientX - bounds.left - state.panX;
    const anchorY = event.clientY - bounds.top - state.panY;
    const ratio = nextZoom / oldZoom;
    state.panX -= anchorX * (ratio - 1);
    state.panY -= anchorY * (ratio - 1);
    state.zoom = nextZoom;
    updateDiagramZoomUi(diagram);
  }, { passive: false });

  viewport.addEventListener('pointerdown', event => {
    const state = diagramState(diagram);
    if (state.zoom === 1 || (event.button !== 0 && event.button !== 1)) return;
    state.pointerId = event.pointerId;
    state.lastX = event.clientX;
    state.lastY = event.clientY;
    viewport.setPointerCapture(event.pointerId);
    viewport.classList.add('is-panning');
  });
  viewport.addEventListener('pointermove', event => {
    const state = diagramState(diagram);
    if (state.pointerId !== event.pointerId) return;
    state.panX += event.clientX - state.lastX;
    state.panY += event.clientY - state.lastY;
    state.lastX = event.clientX;
    state.lastY = event.clientY;
    updateDiagramZoomUi(diagram);
  });
  const stopPanning = event => {
    const state = diagramState(diagram);
    if (state.pointerId !== event.pointerId) return;
    state.pointerId = null;
    viewport.classList.remove('is-panning');
    if (viewport.hasPointerCapture(event.pointerId)) viewport.releasePointerCapture(event.pointerId);
  };
  viewport.addEventListener('pointerup', stopPanning);
  viewport.addEventListener('pointercancel', stopPanning);
  viewport.addEventListener('keydown', event => {
    if (event.key === '+' || event.key === '=') { event.preventDefault(); adjustDiagramZoom(diagram, 1); }
    else if (event.key === '-' || event.key === '_') { event.preventDefault(); adjustDiagramZoom(diagram, -1); }
    else if (event.key === '0') { event.preventDefault(); resetDiagramZoom(diagram); }
  });
  diagram.addEventListener('keydown', event => {
    if (event.key === 'Escape' && diagram.classList.contains('is-expanded')) {
      event.preventDefault();
      toggleDiagramExpanded(diagram);
    }
  });
}

function showDiagramError(id, err) {
  const errorEl = document.getElementById(id + '-error');
  const msgEl = document.getElementById(id + '-error-msg');
  const viewportEl = document.getElementById(id + '-viewport');
  if (errorEl && msgEl) {
    msgEl.textContent = err.message || err.str || String(err);
    errorEl.style.display = 'block';
    if (viewportEl) viewportEl.style.display = 'none';
  }
}

function markImageUnavailable(image) {
  if (!(image instanceof HTMLImageElement) || image.dataset.marknexiaFallback === 'true') return;
  image.dataset.marknexiaFallback = 'true';
  const alt = (image.getAttribute('alt') || '').trim();
  const label = alt ? `Image unavailable: ${alt}` : 'Image unavailable (missing, blocked, or unsupported)';
  const fallback = document.createElement('span');
  fallback.className = 'marknexia-image-fallback';
  fallback.setAttribute('role', 'img');
  fallback.setAttribute('aria-label', label);
  fallback.textContent = label;
  image.replaceWith(fallback);
}

function installImageFallbacks() {
  window.addEventListener('error', event => {
    if (event.target instanceof HTMLImageElement) markImageUnavailable(event.target);
  }, true);

  const scan = () => document.querySelectorAll('img').forEach(image => {
    if (image.complete && image.naturalWidth === 0) markImageUnavailable(image);
  });
  scan();
  const observer = new MutationObserver(scan);
  observer.observe(document.body, { childList: true, subtree: true });
}
