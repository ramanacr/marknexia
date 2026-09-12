import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';

const bridge = await readFile(new URL('../../src/Marknexia.Rendering/Assets/bridge.js', import.meta.url), 'utf8');
const documentCss = await readFile(new URL('../../src/Marknexia.Rendering/Assets/github-markdown.css', import.meta.url), 'utf8');
let browser;
before(async () => {
  browser = await chromium.launch({ headless: true, channel: process.env.MARKNEXIA_TEST_BROWSER || undefined });
});
after(async () => { await browser?.close(); });

async function withDocument(html, check) {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    // Stub only the native-host boundary. Run the production bridge before a
    // real document load so its DOMContentLoaded handlers are exercised too.
    await page.addInitScript({ content: `
      window.hostMessages = [];
      window.chrome ||= {};
      window.chrome.webview = { postMessage: message => window.hostMessages.push(message) };
      ${bridge}` });
    await page.goto('data:text/html;charset=utf-8,' + encodeURIComponent(
      `<body><aside>read outside document</aside><main class="markdown-body">${html}</main></body>`));
    await check(page);
    assert.deepEqual(errors, [], 'bridge must not raise uncaught browser errors');
  } finally { await page.close(); }
}

async function find(page, query, backwards = false) {
  return page.evaluate(({query, backwards}) => window.marknexiaBridge.findText(query, backwards), {query, backwards});
}

test('search counts rendered occurrences and advances/wraps in both directions', async () => {
  await withDocument('<p>Read, reread, READER</p>', async page => {
    assert.deepEqual(await find(page, 'read'), {query:'read', matchCount:3, currentMatch:1});
    assert.equal(await page.evaluate(() => getSelection().toString()), 'Read');
    assert.equal((await find(page, 'read')).currentMatch, 2);
    assert.equal((await find(page, 'read')).currentMatch, 3);
    assert.equal((await find(page, 'read')).currentMatch, 1);
    assert.equal((await find(page, 'read', true)).currentMatch, 3);
  });
});

test('search spans inline markup, but does not join unrelated paragraphs', async () => {
  await withDocument('<p>please <strong>read</strong> me</p><p>read</p><p>me</p>', async page => {
    assert.equal((await find(page, 'read me')).matchCount, 1);
    assert.equal(await page.evaluate(() => getSelection().toString()), 'read me');
  });
});

test('hidden source, closed details, buttons, scripts, and URL attributes are not matches', async () => {
  await withDocument(`<p>Read <a href="https://read.test">link</a></p>
    <div style="display:none"><pre>read hidden source</pre></div>
    <div style="visibility:hidden">read invisible</div>
    <details><summary>Details</summary><p>read collapsed</p></details>
    <button>Read</button><script>window.example = 'read';</script>`, async page => {
    assert.equal((await find(page, 'hidden source')).matchCount, 0, 'display:none source');
    assert.equal((await find(page, 'invisible')).matchCount, 0, 'visibility:hidden text');
    assert.equal((await find(page, 'collapsed')).matchCount, 0, 'closed details content');
    assert.equal((await find(page, 'read')).matchCount, 1);
  });
});

test('query metacharacters and quotes stay literal', async () => {
  await withDocument(`<p>it's [x].* &quot;quoted&quot; λ</p>`, async page => {
    assert.equal((await find(page, `it's [x].* "quoted" λ`)).matchCount, 1);
    assert.deepEqual(await find(page, 'absent'), {query:'absent', matchCount:0, currentMatch:0});
    assert.equal(await page.evaluate(() => getSelection().toString()), '');
  });
});

test('literal query whitespace is preserved, including preformatted spaces', async () => {
  await withDocument('<p>read reader read</p><pre>alpha  beta</pre>', async page => {
    assert.deepEqual(await find(page, 'read '), {query:'read ', matchCount:1, currentMatch:1});
    assert.deepEqual(await find(page, '  '), {query:'  ', matchCount:1, currentMatch:1});
  });
});

test('search follows collapsed rendered whitespace while preserving preformatted text', async () => {
  await withDocument('<p>read\nme</p><p>read    me</p><p>read <strong> \n me</strong></p><pre>read    me</pre>', async page => {
    assert.equal((await find(page, 'read me')).matchCount, 3);
    assert.equal(await page.evaluate(() => getSelection().toString()), 'read me');
    assert.equal((await find(page, 'read    me')).matchCount, 1);
  });
});

test('preserved inline-code whitespace does not consume adjacent normal whitespace', async () => {
  await withDocument(`<style>${documentCss}</style><p><code>read </code> me</p>`, async page => {
    assert.equal(await page.locator('p').innerText(), 'read  me');
    assert.equal((await find(page, 'read  me')).matchCount, 1);
    assert.equal(await page.evaluate(() => getSelection().toString()), 'read  me');
    assert.equal((await find(page, 'read me')).matchCount, 0);
  });
});

test('collapsed trailing and leading whitespace is not searchable at block edges', async () => {
  await withDocument('<p>  read  </p><p>me</p>', async page => {
    assert.equal((await find(page, ' read')).matchCount, 0);
    assert.equal((await find(page, 'read ')).matchCount, 0);
  });
});

test('summary, definition, and CSS block boundaries cannot form a false match', async () => {
  await withDocument('<details open><summary>read</summary>me</details><dl><dt>read</dt><dd>me</dd></dl><div><span style="display:block">read</span>me</div>', async page => {
    assert.equal((await find(page, 'readme')).matchCount, 0);
  });
});

test('search reveals matches in both axes of scrollable code blocks', async () => {
  const code = 'line\n'.repeat(50) + 'x'.repeat(240) + 'needle';
  await withDocument(`<style>${documentCss}\npre { width: 300px; max-height: 100px; }</style><pre>${code}</pre>`, async page => {
    assert.equal((await find(page, 'needle')).matchCount, 1);
    const position = await page.evaluate(() => {
      const pre = document.querySelector('pre');
      const frame = pre.getBoundingClientRect();
      const match = getSelection().getRangeAt(0).getBoundingClientRect();
      return {left:match.left, right:match.right, top:match.top, bottom:match.bottom,
        frameLeft:frame.left + pre.clientLeft, frameRight:frame.left + pre.clientLeft + pre.clientWidth,
        frameTop:frame.top + pre.clientTop, frameBottom:frame.top + pre.clientTop + pre.clientHeight,
        scrollLeft:pre.scrollLeft, scrollTop:pre.scrollTop};
    });
    assert.ok(position.scrollLeft > 0 && position.scrollTop > 0, JSON.stringify(position));
    assert.ok(position.left >= position.frameLeft - 1 && position.right <= position.frameRight + 1, JSON.stringify(position));
    assert.ok(position.top >= position.frameTop - 1 && position.bottom <= position.frameBottom + 1, JSON.stringify(position));
  });
});

test('closed details retain visible summaries and opening refreshes the results', async () => {
  await withDocument('<details><summary><strong>read summary</strong></summary><p>read body</p><details><summary>read nested</summary></details></details>', async page => {
    assert.equal((await find(page, 'read')).matchCount, 1);
    await page.locator('details').first().evaluate(element => element.open = true);
    assert.equal((await find(page, 'read')).matchCount, 3);
    await page.locator('details').first().evaluate(element => element.open = false);
    assert.equal((await find(page, 'read')).matchCount, 1);
  });
});

test('search does not join text after a block to the preceding paragraph', async () => {
  await withDocument('<div><p>read</p> me</div>', async page => {
    assert.equal((await find(page, 'read me')).matchCount, 0);
  });
});

test('rendered content changes invalidate search and clear leaves markup unchanged', async () => {
  await withDocument('<p id="content">read</p>', async page => {
    await find(page, 'read');
    await page.evaluate(() => document.getElementById('content').textContent = 'read read');
    const before = await page.locator('.markdown-body').innerHTML();
    assert.equal((await find(page, 'read')).matchCount, 2);
    await page.evaluate(() => window.marknexiaBridge.clearSearch());
    assert.equal(await page.evaluate(() => getSelection().toString()), '');
    assert.equal(await page.locator('.markdown-body').innerHTML(), before);
    assert.equal((await find(page, 'read', true)).currentMatch, 2);
  });
});

test('delegated copy sends exact Unicode source to the host', async () => {
  const source = 'const text = "<tag> & \'quoted\' λ";\n\n$1';
  const payload = encodeURIComponent(source).replaceAll("'", '&#39;');
  await withDocument(`<button data-marknexia-action="copy" data-copy-text='${payload}'><strong>Copy code</strong></button>`, async page => {
    await page.getByText('Copy code', {exact:true}).click();
    assert.deepEqual(await page.evaluate(() => window.hostMessages), [{type:'copyText', text:source}]);
  });
});

test('source toggle changes only its enclosing diagram and invalidates search', async () => {
  const diagram = id => `<div class="marknexia-mermaid" id="${id}">
    <button data-marknexia-action="toggle-source">Source ${id}</button>
    <div id="${id}-viewport">read diagram</div>
    <pre id="${id}-source" style="display:none">read read source</pre></div>`;
  await withDocument(diagram('first') + diagram('second'), async page => {
    assert.equal((await find(page, 'read')).matchCount, 2);
    await page.getByRole('button', {name:'Source first'}).click();
    assert.equal(await page.locator('#first-source').isVisible(), true);
    assert.equal(await page.locator('#first-viewport').isVisible(), false);
    assert.equal(await page.locator('#second-source').isVisible(), false);
    assert.equal((await find(page, 'read')).matchCount, 3);
    await page.getByRole('button', {name:'Source first'}).click();
    assert.equal((await find(page, 'read')).matchCount, 2);
  });
});

test('Mermaid diagrams expose bounded zoom controls and pointer zoom/pan', async () => {
  const diagram = `<div class="marknexia-mermaid" id="diagram">
    <div class="marknexia-diagram-toolbar">
      <div class="marknexia-diagram-actions">
        <button data-marknexia-action="zoom-out" aria-label="Zoom out">−</button>
        <button data-marknexia-action="zoom-reset" aria-label="Reset diagram zoom">100%</button>
        <button data-marknexia-action="zoom-in" aria-label="Zoom in">+</button>
        <button data-marknexia-action="expand" aria-label="Open diagram full window">⛶</button>
      </div>
      <span data-marknexia-zoom-status aria-live="polite">100%</span>
    </div>
    <div id="diagram-viewport" class="marknexia-diagram-viewport">
      <div class="marknexia-diagram-canvas"><svg id="diagram-svg" width="240" height="120"><rect width="240" height="120"></rect></svg></div>
    </div>
  </div>`;
  await withDocument(`<style>${documentCss}
    .marknexia-diagram-viewport { width: 360px; height: 200px; }
    .marknexia-diagram-canvas { width: 240px; height: 120px; }
  </style>${diagram}`, async page => {
    const status = page.locator('[data-marknexia-zoom-status]');
    assert.equal(await status.textContent(), '100%');
    assert.equal(await page.locator('#diagram-viewport').evaluate(element => getComputedStyle(element).overflow), 'auto');
    await page.getByRole('button', {name:'Zoom in'}).click();
    assert.equal(await status.textContent(), '125%');
    assert.match(await page.locator('.marknexia-diagram-canvas').getAttribute('style'), /scale\(1\.25\)/);
    await page.locator('#diagram-viewport').dispatchEvent('wheel', {deltaY:-120, clientX:180, clientY:100});
    assert.equal(await status.textContent(), '150%');
    await page.getByRole('button', {name:'Reset diagram zoom'}).click();
    assert.equal(await status.textContent(), '100%');
    assert.match(await page.locator('#diagram-viewport').getAttribute('aria-label'), /zoom/i);
    const expand = page.getByRole('button', {name:'Open diagram full window'});
    await expand.click();
    assert.equal(await page.locator('#diagram').evaluate(element => element.classList.contains('is-expanded')), true);
    assert.equal(await page.locator('html').evaluate(element => element.classList.contains('marknexia-diagram-expanded')), true);
    assert.equal(await page.locator('#diagram-viewport').getAttribute('aria-modal'), 'true');
    await page.keyboard.press('Escape');
    assert.equal(await page.locator('#diagram').evaluate(element => element.classList.contains('is-expanded')), false);
    assert.equal(await page.getByRole('button', {name:'Open diagram full window'}).evaluate(element => element === document.activeElement), true);
  });
});

test('link click preserves relative destination and delegates navigation to host', async () => {
  await withDocument('<a href="../docs/a%20b.md#heading"><strong>Open document</strong></a>', async page => {
    const url = page.url();
    await page.getByText('Open document', {exact:true}).click();
    assert.deepEqual(await page.evaluate(() => window.hostMessages), [{type:'openLink', href:'../docs/a%20b.md#heading'}]);
    assert.equal(page.url(), url);
  });
});

test('failed images become accessible inline fallbacks without a broken-image icon', async () => {
  await withDocument('<p>Before</p><img alt="Architecture diagram" src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw=="><p>After</p>', async page => {
    await page.locator('img').evaluate(image => image.dispatchEvent(new Event('error')));
    const fallback = page.locator('.marknexia-image-fallback');
    assert.equal(await fallback.count(), 1);
    assert.equal(await fallback.getAttribute('role'), 'img');
    assert.equal(await fallback.getAttribute('aria-label'), 'Image unavailable: Architecture diagram');
    assert.equal(await fallback.textContent(), 'Image unavailable: Architecture diagram');
    assert.equal(await page.locator('img').count(), 0);
  });
});

test('location reset exposes a deterministic scroll-to-top bridge action', async () => {
  await withDocument('<style>body { min-height: 2400px; }</style><p>Document</p>', async page => {
    await page.evaluate(() => window.scrollTo(0, 900));
    assert.ok(await page.evaluate(() => window.scrollY > 0));
    await page.evaluate(() => window.marknexiaBridge.scrollToTop());
    assert.equal(await page.evaluate(() => window.scrollY), 0);
  });
});
