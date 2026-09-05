window.marknexiaBridge = {
  openLink: function(href) {
    if (window.chrome && window.chrome.webview) {
      window.chrome.webview.postMessage({ type: 'openLink', href: href });
    }
  },
  copyText: function(text) {
    if (navigator.clipboard) {
      navigator.clipboard.writeText(text);
    } else if (window.chrome && window.chrome.webview) {
      window.chrome.webview.postMessage({ type: 'copyText', text: text });
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
        viewEl.style.display = 'flex';
      }
    }
  },
  scrollToAnchor: function(anchorId) {
    const target = document.getElementById(anchorId) || document.querySelector('[name="' + anchorId + '"]');
    if (target) {
      target.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
  }
};

document.addEventListener('DOMContentLoaded', () => {
  // Intercept all <a> clicks
  document.body.addEventListener('click', (e) => {
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
