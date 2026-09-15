const fs = require('fs');
const path = require('path');

const logoBase64 = fs.readFileSync(path.join(__dirname, '../store-submission/assets/marknexia-512.png')).toString('base64');

// We will generate the images by launching a headless browser or having chrome-devtools-mcp navigate to generator.html
const htmlContent = `<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>Marknexia Store Asset Generator</title>
  <style>
    body { margin: 0; background: #000; overflow: hidden; font-family: 'Segoe UI Variable Display', 'Segoe UI', -apple-system, sans-serif; }
    canvas { display: block; }
  </style>
</head>
<body>
  <img id="logo" src="data:image/png;base64,${logoBase64}" style="display:none;" />
  <canvas id="c"></canvas>
  <script>
    const logoImg = document.getElementById('logo');
    const canvas = document.getElementById('c');
    const ctx = canvas.getContext('2d');

    function roundRect(ctx, x, y, w, h, r, fill, stroke) {
      if (typeof r === 'number') r = {tl: r, tr: r, br: r, bl: r};
      ctx.beginPath();
      ctx.moveTo(x + r.tl, y);
      ctx.lineTo(x + w - r.tr, y);
      ctx.quadraticCurveTo(x + w, y, x + w, y + r.tr);
      ctx.lineTo(x + w, y + h - r.br);
      ctx.quadraticCurveTo(x + w, y + h, x + w - r.br, y + h);
      ctx.lineTo(x + r.bl, y + h);
      ctx.quadraticCurveTo(x, y + h, x, y + h - r.bl);
      ctx.lineTo(x, y + r.tl);
      ctx.quadraticCurveTo(x, y, x + r.tl, y);
      ctx.closePath();
      if (fill) ctx.fill();
      if (stroke) ctx.stroke();
    }

    function drawShell(title, activeTab, tabs, isSearchActive) {
      canvas.width = 1920;
      canvas.height = 1080;
      ctx.fillStyle = '#0d1117';
      ctx.fillRect(0, 0, 1920, 1080);

      // Titlebar
      ctx.fillStyle = '#161b22';
      ctx.fillRect(0, 0, 1920, 38);
      ctx.drawImage(logoImg, 14, 8, 22, 22);
      ctx.fillStyle = '#e6edf3';
      ctx.font = '600 13px "Segoe UI", sans-serif';
      ctx.fillText('Marknexia — ' + title, 44, 24);

      // Window controls
      ctx.fillStyle = '#848d97';
      ctx.font = '14px "Segoe UI", sans-serif';
      ctx.fillText('—', 1780, 24);
      ctx.fillText('□', 1830, 23);
      ctx.fillText('✕', 1880, 23);

      // TabView Bar
      ctx.fillStyle = '#161b22';
      ctx.fillRect(0, 38, 1920, 42);
      let tabX = 14;
      tabs.forEach(t => {
        const isActive = t === activeTab;
        const tabWidth = 180;
        if (isActive) {
          ctx.fillStyle = '#0d1117';
          roundRect(ctx, tabX, 42, tabWidth, 38, {tl: 6, tr: 6, br: 0, bl: 0}, true, false);
          ctx.fillStyle = '#2f81f7';
          ctx.fillRect(tabX + 16, 78, tabWidth - 32, 2);
          ctx.fillStyle = '#ffffff';
          ctx.font = '600 13px "Segoe UI", sans-serif';
        } else {
          ctx.fillStyle = '#1f242c';
          roundRect(ctx, tabX, 45, tabWidth, 35, {tl: 6, tr: 6, br: 0, bl: 0}, true, false);
          ctx.fillStyle = '#848d97';
          ctx.font = '400 13px "Segoe UI", sans-serif';
        }
        ctx.fillText(t, tabX + 18, 64);
        ctx.fillStyle = '#848d97';
        ctx.font = '12px "Segoe UI", sans-serif';
        ctx.fillText('×', tabX + tabWidth - 22, 64);
        tabX += tabWidth + 6;
      });

      // New Tab Button
      ctx.fillStyle = '#21262d';
      roundRect(ctx, tabX, 45, 34, 34, 6, true, false);
      ctx.fillStyle = '#848d97';
      ctx.font = '16px "Segoe UI", sans-serif';
      ctx.fillText('+', tabX + 12, 67);

      // Command Toolbar
      ctx.fillStyle = '#161b22';
      ctx.fillRect(0, 80, 1920, 48);
      ctx.strokeStyle = '#30363d';
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, 128); ctx.lineTo(1920, 128);
      ctx.stroke();

      // Toolbar Buttons
      const buttons = [
        { label: 'Sidebar', icon: '☰' },
        { label: 'Open File', icon: '📄' },
        { label: 'Open Folder', icon: '📁' },
        { label: 'Theme: Dark', icon: '🌙' },
        { label: 'Zoom: 100%', icon: '🔍' },
        { label: 'Settings', icon: '⚙' }
      ];
      let btnX = 16;
      buttons.forEach(b => {
        ctx.fillStyle = '#21262d';
        roundRect(ctx, btnX, 88, 110, 32, 6, true, false);
        ctx.fillStyle = '#c9d1d9';
        ctx.font = '13px "Segoe UI", sans-serif';
        ctx.fillText(b.icon + ' ' + b.label, btnX + 12, 109);
        btnX += 120;
      });

      // Split View Sidebar
      ctx.fillStyle = '#161b22';
      ctx.fillRect(0, 128, 320, 952);
      ctx.strokeStyle = '#30363d';
      ctx.beginPath();
      ctx.moveTo(320, 128); ctx.lineTo(320, 1080);
      ctx.stroke();

      // Main Viewer Area
      ctx.fillStyle = '#0d1117';
      ctx.fillRect(321, 128, 1599, 952);

      if (isSearchActive) {
        const sX = 1520, sY = 144, sW = 360, sH = 46;
        ctx.fillStyle = '#1f2937';
        ctx.strokeStyle = '#3b82f6';
        ctx.lineWidth = 2;
        roundRect(ctx, sX, sY, sW, sH, 8, true, true);
        ctx.fillStyle = '#ffffff';
        ctx.font = '14px "Cascadia Code", monospace';
        ctx.fillText('Mermaid', sX + 16, sY + 28);
        ctx.fillStyle = '#9ca3af';
        ctx.font = '13px "Segoe UI", sans-serif';
        ctx.fillText('1 of 4 matches', sX + 140, sY + 28);
        ctx.fillText('↑  ↓  ✕', sX + 280, sY + 28);
      }
    }

    window.renderScreenshot1 = function() {
      drawShell('README.md', 'README.md', ['README.md', 'features.md', 'architecture.md'], false);

      ctx.fillStyle = '#848d97';
      ctx.font = 'bold 12px "Segoe UI", sans-serif';
      ctx.fillText('DOCUMENT OUTLINE', 24, 160);
      
      const outlineItems = [
        { level: 1, text: 'Marknexia' },
        { level: 2, text: '⚡ GitHub-Style Alerts' },
        { level: 2, text: '📊 GFM Tables & Tasks' },
        { level: 2, text: '🧭 Repository Navigation' },
        { level: 2, text: '🛠 Syntax Highlighting' },
        { level: 2, text: '🔒 Offline Security' }
      ];
      let oY = 195;
      outlineItems.forEach(item => {
        const indent = (item.level - 1) * 16;
        if (item.level === 1) {
          ctx.fillStyle = '#1f2937';
          roundRect(ctx, 16, oY - 18, 288, 28, 4, true, false);
          ctx.fillStyle = '#58a6ff';
          ctx.font = 'bold 13px "Segoe UI", sans-serif';
        } else {
          ctx.fillStyle = '#c9d1d9';
          ctx.font = '400 13px "Segoe UI", sans-serif';
        }
        ctx.fillText((item.level === 1 ? '● ' : '  ▸ ') + item.text, 24 + indent, oY);
        oY += 34;
      });

      let mX = 380, mY = 180;
      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 36px "Segoe UI Variable Display", sans-serif';
      ctx.fillText('Marknexia — GitHub-Style Markdown Viewer', mX, mY);
      mY += 28;

      ctx.fillStyle = '#848d97';
      ctx.font = '16px "Segoe UI", sans-serif';
      ctx.fillText('Native offline-first Windows Markdown documentation and repository reader with 100% GFM parity.', mX, mY);
      mY += 48;

      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 24px "Segoe UI", sans-serif';
      ctx.fillText('Authentic GitHub Alert Callouts', mX, mY);
      mY += 28;

      const alerts = [
        { type: 'NOTE', color: '#2f81f7', bg: 'rgba(56, 139, 253, 0.12)', text: 'Highlights information that users should take into account, even when skimming.' },
        { type: 'TIP', color: '#3fb950', bg: 'rgba(63, 185, 80, 0.12)', text: 'Optional advice to help a user be more successful with local repositories.' },
        { type: 'IMPORTANT', color: '#a371f7', bg: 'rgba(163, 113, 247, 0.12)', text: 'Crucial information necessary for users to succeed and avoid configuration errors.' },
        { type: 'WARNING', color: '#d29922', bg: 'rgba(210, 153, 34, 0.12)', text: 'Critical content demanding immediate user attention due to potential risks.' },
        { type: 'CAUTION', color: '#f85149', bg: 'rgba(248, 81, 73, 0.12)', text: 'Negative potential consequences of an action or missing dependency.' }
      ];

      alerts.forEach(a => {
        const aWidth = 960;
        ctx.fillStyle = a.bg;
        roundRect(ctx, mX, mY, aWidth, 48, 6, true, false);
        ctx.fillStyle = a.color;
        ctx.fillRect(mX, mY, 4, 48);
        ctx.fillStyle = a.color;
        ctx.font = 'bold 13px "Segoe UI", sans-serif';
        ctx.fillText(a.type, mX + 18, mY + 29);
        ctx.fillStyle = '#e6edf3';
        ctx.font = '14px "Segoe UI", sans-serif';
        ctx.fillText(a.text, mX + 120, mY + 29);
        mY += 56;
      });

      mY += 12;
      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 20px "Segoe UI", sans-serif';
      ctx.fillText('Extended GFM Tables & Task Lists', mX, mY);
      mY += 24;

      const tW = 960;
      ctx.fillStyle = '#161b22';
      ctx.fillRect(mX, mY, tW, 36);
      ctx.strokeStyle = '#30363d';
      ctx.strokeRect(mX, mY, tW, 36);
      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 13px "Segoe UI", sans-serif';
      ctx.fillText('FEATURE', mX + 20, mY + 23);
      ctx.fillText('SPECIFICATION', mX + 260, mY + 23);
      ctx.fillText('WINDOWS STATUS', mX + 560, mY + 23);
      ctx.fillText('OFFLINE CAPABILITY', mX + 760, mY + 23);
      mY += 36;

      const rows = [
        ['Alert Callouts', 'GitHub Primer Syntax [!NOTE]', '100% Native Rendered', 'Offline Bundled'],
        ['Mermaid 11.4', 'Flowchart, Sequence, ER', 'Hardware Accelerated', 'Offline Bundled'],
        ['Heading Anchors', 'GitHub slug algorithm', 'Deterministic Jump', 'Local Traversal Sandboxed']
      ];
      rows.forEach((r, idx) => {
        ctx.fillStyle = idx % 2 === 0 ? '#0d1117' : '#161b22';
        ctx.fillRect(mX, mY, tW, 32);
        ctx.strokeRect(mX, mY, tW, 32);
        ctx.fillStyle = '#e6edf3';
        ctx.font = '13px "Segoe UI", sans-serif';
        ctx.fillText(r[0], mX + 20, mY + 21);
        ctx.fillText(r[1], mX + 260, mY + 21);
        ctx.fillStyle = '#3fb950';
        ctx.fillText('✔ ' + r[2], mX + 560, mY + 21);
        ctx.fillStyle = '#58a6ff';
        ctx.fillText(r[3], mX + 760, mY + 21);
        mY += 32;
      });

      return canvas.toDataURL('image/png');
    };

    window.renderScreenshot2 = function() {
      drawShell('architecture.md', 'architecture.md', ['README.md', 'architecture.md', 'roadmap.md'], false);

      ctx.fillStyle = '#848d97';
      ctx.font = 'bold 12px "Segoe UI", sans-serif';
      ctx.fillText('ARCHITECTURE OUTLINE', 24, 160);
      const items = ['Overview', 'Data Pipeline', 'Mermaid Runtime', 'Sequence Flow', 'Security Bounds'];
      let sY = 195;
      items.forEach((it, i) => {
        ctx.fillStyle = i === 3 ? '#58a6ff' : '#c9d1d9';
        ctx.font = (i === 3 ? 'bold ' : '') + '13px "Segoe UI", sans-serif';
        ctx.fillText('▸ ' + it, 24, sY);
        sY += 32;
      });

      let mX = 380, mY = 180;
      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 36px "Segoe UI Variable Display", sans-serif';
      ctx.fillText('Offline Mermaid Diagrams & Visualizations', mX, mY);
      mY += 28;
      ctx.fillStyle = '#848d97';
      ctx.font = '16px "Segoe UI", sans-serif';
      ctx.fillText('Interactive sequence diagrams and flowcharts rendered completely offline with bundled Mermaid 11.4 runtime.', mX, mY);
      mY += 48;

      const boxW = 1000, boxH = 330;
      ctx.fillStyle = '#161b22';
      ctx.strokeStyle = '#30363d';
      ctx.lineWidth = 1;
      roundRect(ctx, mX, mY, boxW, boxH, 8, true, true);

      ctx.fillStyle = '#21262d';
      roundRect(ctx, mX, mY, boxW, 40, {tl: 8, tr: 8, br: 0, bl: 0}, true, false);
      ctx.fillStyle = '#58a6ff';
      ctx.font = 'bold 13px "Cascadia Code", monospace';
      ctx.fillText('mermaid: SequenceDiagram', mX + 16, mY + 25);
      
      ctx.fillStyle = '#30363d';
      roundRect(ctx, mX + boxW - 200, mY + 8, 90, 24, 4, true, false);
      roundRect(ctx, mX + boxW - 100, mY + 8, 86, 24, 4, true, false);
      ctx.fillStyle = '#c9d1d9';
      ctx.font = '11px "Segoe UI", sans-serif';
      ctx.fillText('Copy Source', mX + boxW - 188, mY + 24);
      ctx.fillText('Toggle Code', mX + boxW - 90, mY + 24);

      const pY = mY + 68;
      const participants = [
        { name: 'User / Windows', x: mX + 70 },
        { name: 'Marknexia App', x: mX + 310 },
        { name: 'GFM Parser', x: mX + 550 },
        { name: 'Offline Mermaid', x: mX + 790 }
      ];

      participants.forEach(p => {
        ctx.fillStyle = '#1f2937';
        ctx.strokeStyle = '#388bfd';
        ctx.lineWidth = 2;
        roundRect(ctx, p.x, pY, 150, 40, 6, true, true);
        ctx.fillStyle = '#ffffff';
        ctx.font = 'bold 13px "Segoe UI", sans-serif';
        ctx.fillText(p.name, p.x + 18, pY + 25);

        ctx.strokeStyle = '#30363d';
        ctx.setLineDash([4, 4]);
        ctx.beginPath();
        ctx.moveTo(p.x + 75, pY + 40);
        ctx.lineTo(p.x + 75, pY + 200);
        ctx.stroke();
        ctx.setLineDash([]);
      });

      const arrows = [
        { from: mX + 145, to: mX + 385, y: pY + 65, label: '1. Open repository document (.md)' },
        { from: mX + 385, to: mX + 625, y: pY + 105, label: '2. Generate GFM AST & Callouts' },
        { from: mX + 625, to: mX + 865, y: pY + 145, label: '3. Render SVG Diagrams offline' },
        { from: mX + 865, to: mX + 385, y: pY + 185, label: '4. Hardware-accelerated canvas' }
      ];

      arrows.forEach(arr => {
        ctx.strokeStyle = '#58a6ff';
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.moveTo(arr.from, arr.y);
        ctx.lineTo(arr.to, arr.y);
        ctx.stroke();

        const dir = arr.to > arr.from ? 1 : -1;
        ctx.fillStyle = '#58a6ff';
        ctx.beginPath();
        ctx.moveTo(arr.to, arr.y);
        ctx.lineTo(arr.to - dir * 8, arr.y - 5);
        ctx.lineTo(arr.to - dir * 8, arr.y + 5);
        ctx.closePath();
        ctx.fill();

        ctx.fillStyle = '#e6edf3';
        ctx.font = '12px "Cascadia Code", monospace';
        const midX = (arr.from + arr.to) / 2 - 100;
        ctx.fillText(arr.label, midX, arr.y - 6);
      });

      mY += boxH + 30;
      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 20px "Segoe UI", sans-serif';
      ctx.fillText('Offline Rendering Fallback & Security Sandbox', mX, mY);
      mY += 20;

      const fW = 1000, fH = 130;
      ctx.fillStyle = '#161b22';
      roundRect(ctx, mX, mY, fW, fH, 8, true, true);

      const nodes = [
        { text: 'Start: Local Markdown', x: mX + 50, y: mY + 40, color: '#388bfd' },
        { text: 'Sanitize DOM & HTML', x: mX + 290, y: mY + 40, color: '#8957e5' },
        { text: 'Offline Mermaid Engine', x: mX + 530, y: mY + 40, color: '#238636' },
        { text: 'Smooth Native Render', x: mX + 770, y: mY + 40, color: '#3fb950' }
      ];
      nodes.forEach((n, idx) => {
        ctx.fillStyle = '#21262d';
        ctx.strokeStyle = n.color;
        ctx.lineWidth = 2;
        roundRect(ctx, n.x, n.y, 175, 50, 8, true, true);
        ctx.fillStyle = '#ffffff';
        ctx.font = 'bold 12px "Segoe UI", sans-serif';
        ctx.fillText(n.text, n.x + 15, n.y + 30);

        if (idx < nodes.length - 1) {
          ctx.strokeStyle = '#58a6ff';
          ctx.lineWidth = 2;
          ctx.beginPath();
          ctx.moveTo(n.x + 175, n.y + 25);
          ctx.lineTo(n.x + 225, n.y + 25);
          ctx.stroke();
          ctx.fillStyle = '#58a6ff';
          ctx.beginPath();
          ctx.moveTo(n.x + 225, n.y + 25);
          ctx.lineTo(n.x + 217, n.y + 21);
          ctx.lineTo(n.x + 217, n.y + 29);
          ctx.closePath();
          ctx.fill();
        }
      });

      return canvas.toDataURL('image/png');
    };

    window.renderScreenshot3 = function() {
      drawShell('setup.md', 'setup.md', ['README.md', 'setup.md', 'api.md'], false);

      ctx.fillStyle = '#848d97';
      ctx.font = 'bold 12px "Segoe UI", sans-serif';
      ctx.fillText('REPOSITORY EXPLORER', 24, 160);

      const files = [
        { type: 'folder', name: 'marknexia', open: true, depth: 0 },
        { type: 'folder', name: '.github', open: false, depth: 1 },
        { type: 'folder', name: 'docs', open: true, depth: 1 },
        { type: 'file', name: 'architecture.md', depth: 2 },
        { type: 'file', name: 'setup.md', active: true, depth: 2 },
        { type: 'file', name: 'troubleshooting.md', depth: 2 },
        { type: 'folder', name: 'src', open: true, depth: 1 },
        { type: 'file', name: 'Marknexia.slnx', depth: 2 },
        { type: 'file', name: 'Directory.Build.props', depth: 2 },
        { type: 'file', name: 'README.md', depth: 1 },
        { type: 'file', name: 'LICENSE', depth: 1 }
      ];

      let fY = 195;
      files.forEach(f => {
        const indent = f.depth * 18;
        if (f.active) {
          ctx.fillStyle = '#1f2937';
          roundRect(ctx, 16, fY - 18, 288, 28, 4, true, false);
          ctx.fillStyle = '#58a6ff';
        } else {
          ctx.fillStyle = f.type === 'folder' ? '#d29922' : '#c9d1d9';
        }
        ctx.font = (f.active ? 'bold ' : '') + '13px "Segoe UI", sans-serif';
        const icon = f.type === 'folder' ? (f.open ? '▾ 📁 ' : '▸ 📁 ') : '  📄 ';
        ctx.fillText(icon + f.name, 20 + indent, fY);
        fY += 32;
      });

      let mX = 380, mY = 180;
      ctx.fillStyle = '#58a6ff';
      ctx.font = '13px "Cascadia Code", monospace';
      ctx.fillText('marknexia / docs / setup.md', mX, mY);
      mY += 32;

      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 36px "Segoe UI Variable Display", sans-serif';
      ctx.fillText('Repository Navigation & Cross-File Linking', mX, mY);
      mY += 28;
      ctx.fillStyle = '#848d97';
      ctx.font = '16px "Segoe UI", sans-serif';
      ctx.fillText('Smooth traversal across documentation trees with repository-relative root paths, anchor targeting, and history navigation.', mX, mY);
      mY += 48;

      const cW = 980;
      ctx.fillStyle = '#161b22';
      roundRect(ctx, mX, mY, cW, 360, 8, true, true);

      let lY = mY + 36;
      ctx.fillStyle = '#58a6ff';
      ctx.font = 'bold 18px "Segoe UI", sans-serif';
      ctx.fillText('Resilient Relative Path Resolution', mX + 24, lY);
      lY += 32;

      const linkDemos = [
        { label: 'Relative file link:', code: '[Architecture Guide](./architecture.md)', result: 'Resolves to docs/architecture.md within repository workspace' },
        { label: 'Repository root path:', code: '[Build Config](/Directory.Build.props)', result: 'Automatically maps / to the git repository root folder boundary' },
        { label: 'Cross-document anchor:', code: '[Security Model](./architecture.md#security-sandbox)', result: 'Opens target document and automatically scrolls smoothly to heading' },
        { label: 'Directory sandboxing:', code: '[Blocked Escape](../../../Windows/System32)', result: 'Blocked: Directory traversal sandboxing prevents unauthorized filesystem escapes' }
      ];

      linkDemos.forEach(ld => {
        ctx.fillStyle = '#848d97';
        ctx.font = 'bold 13px "Segoe UI", sans-serif';
        ctx.fillText(ld.label, mX + 24, lY);
        
        ctx.fillStyle = '#1f242c';
        roundRect(ctx, mX + 230, lY - 16, 330, 24, 4, true, false);
        ctx.fillStyle = '#79c0ff';
        ctx.font = '12px "Cascadia Code", monospace';
        ctx.fillText(ld.code, mX + 238, lY);

        ctx.fillStyle = '#3fb950';
        ctx.font = '12px "Segoe UI", sans-serif';
        ctx.fillText('➔ ' + ld.result, mX + 24, lY + 24);
        lY += 56;
      });

      return canvas.toDataURL('image/png');
    };

    window.renderScreenshot4 = function() {
      drawShell('specifications.md', 'specifications.md', ['README.md', 'specifications.md'], true);

      ctx.fillStyle = '#848d97';
      ctx.font = 'bold 12px "Segoe UI", sans-serif';
      ctx.fillText('OUTLINE (SEARCH FILTERED)', 24, 160);
      const items = ['1. Overview', '2. Mermaid Specification (2 matches)', '3. Diagram Architecture (1 match)', '4. Performance Metrics (1 match)'];
      let sY = 195;
      items.forEach((it, i) => {
        ctx.fillStyle = it.includes('matches') ? '#d29922' : '#c9d1d9';
        ctx.font = (it.includes('matches') ? 'bold ' : '') + '13px "Segoe UI", sans-serif';
        ctx.fillText('▸ ' + it, 24, sY);
        sY += 32;
      });

      let mX = 380, mY = 180;
      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 36px "Segoe UI Variable Display", sans-serif';
      ctx.fillText('In-Page Search with Instant Match Highlighting', mX, mY);
      mY += 28;
      ctx.fillStyle = '#848d97';
      ctx.font = '16px "Segoe UI", sans-serif';
      ctx.fillText('Built-in search engine (Ctrl+F) with real-time match counters, keyword navigation, and active hit highlighting.', mX, mY);
      mY += 48;

      const p1 = "Marknexia includes a dedicated offline ";
      const hl = "Mermaid";
      const p2 = " engine supporting Flowcharts, Sequence Diagrams, Class Diagrams, and State Diagrams.";
      
      ctx.fillStyle = '#e6edf3';
      ctx.font = '16px "Segoe UI", sans-serif';
      ctx.fillText(p1, mX, mY);
      const w1 = ctx.measureText(p1).width;
      
      ctx.fillStyle = '#f59e0b';
      roundRect(ctx, mX + w1 - 2, mY - 18, 74, 26, 4, true, false);
      ctx.fillStyle = '#000000';
      ctx.font = 'bold 16px "Segoe UI", sans-serif';
      ctx.fillText(hl, mX + w1 + 3, mY);

      ctx.fillStyle = '#e6edf3';
      ctx.font = '16px "Segoe UI", sans-serif';
      ctx.fillText(p2, mX + w1 + 78, mY);
      mY += 40;

      const p3 = "When an offline document embeds a code block,";
      const p4 = " code block, Marknexia parses the AST node and sends it to the isolated renderer.";
      ctx.fillText(p3, mX, mY);
      const w3 = ctx.measureText(p3).width;
      ctx.fillStyle = '#eab308';
      roundRect(ctx, mX + w3 - 2, mY - 18, 74, 26, 4, true, false);
      ctx.fillStyle = '#000000';
      ctx.font = 'bold 16px "Segoe UI", sans-serif';
      ctx.fillText(hl, mX + w3 + 3, mY);

      ctx.fillStyle = '#e6edf3';
      ctx.font = '16px "Segoe UI", sans-serif';
      ctx.fillText(p4, mX + w3 + 78, mY);
      mY += 60;

      const fW = 980;
      ctx.fillStyle = '#161b22';
      roundRect(ctx, mX, mY, fW, 250, 8, true, true);

      let gY = mY + 36;
      ctx.fillStyle = '#58a6ff';
      ctx.font = 'bold 18px "Segoe UI", sans-serif';
      ctx.fillText('Keyboard Search Accelerator & Controls', mX + 24, gY);
      gY += 32;

      const shortcuts = [
        { key: 'Ctrl + F', desc: 'Open in-page search bar instantly' },
        { key: 'Enter / F3', desc: 'Jump to next match smoothly' },
        { key: 'Shift + Enter / Shift + F3', desc: 'Jump to previous match' },
        { key: 'Escape', desc: 'Dismiss search bar and restore focus' }
      ];

      shortcuts.forEach(sc => {
        ctx.fillStyle = '#21262d';
        ctx.strokeStyle = '#388bfd';
        roundRect(ctx, mX + 24, gY - 16, 200, 26, 4, true, true);
        ctx.fillStyle = '#79c0ff';
        ctx.font = 'bold 12px "Cascadia Code", monospace';
        ctx.fillText(sc.key, mX + 36, gY + 1);

        ctx.fillStyle = '#e6edf3';
        ctx.font = '13px "Segoe UI", sans-serif';
        ctx.fillText(sc.desc, mX + 240, gY + 1);
        gY += 40;
      });

      return canvas.toDataURL('image/png');
    };

    window.renderScreenshot5 = function() {
      drawShell('CHANGELOG.md', 'CHANGELOG.md', ['README.md', 'CHANGELOG.md'], false);

      ctx.fillStyle = '#848d97';
      ctx.font = 'bold 12px "Segoe UI", sans-serif';
      ctx.fillText('VERSIONS & MILESTONES', 24, 160);
      const items = ['v1.0.17 (Latest)', 'v1.0.16', 'v1.0.15', 'v1.0.12', 'v1.0.0 Initial Release'];
      let sY = 195;
      items.forEach((it, i) => {
        ctx.fillStyle = i === 0 ? '#3fb950' : '#c9d1d9';
        ctx.font = (i === 0 ? 'bold ' : '') + '13px "Segoe UI", sans-serif';
        ctx.fillText((i === 0 ? '● ' : '  ') + it, 24, sY);
        sY += 32;
      });

      let mX = 380, mY = 180;
      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 36px "Segoe UI Variable Display", sans-serif';
      ctx.fillText('GitHub Primer Dark Theme & Modern Performance', mX, mY);
      mY += 28;
      ctx.fillStyle = '#848d97';
      ctx.font = '16px "Segoe UI", sans-serif';
      ctx.fillText('Engineered with .NET 10, C# 13, and WinUI 3 (Windows App SDK) for instant cold startup and minimal memory footprint.', mX, mY);
      mY += 48;

      const metrics = [
        { val: '< 500 ms', lbl: 'Interactive Startup (p95)', sub: 'Native compiled WinUI shell' },
        { val: '100%', lbl: 'Offline Independence', sub: 'Zero external network calls' },
        { val: '< 35 MB', lbl: 'Host Working Set', sub: 'Optimized memory management' },
        { val: 'x64 & ARM64', lbl: 'Native Architecture', sub: 'Optimized for Surface & Copilot+ PCs' }
      ];

      let cardX = mX;
      metrics.forEach(m => {
        ctx.fillStyle = '#161b22';
        ctx.strokeStyle = '#30363d';
        roundRect(ctx, cardX, mY, 235, 110, 8, true, true);
        ctx.fillStyle = '#58a6ff';
        ctx.font = 'bold 24px "Segoe UI", sans-serif';
        ctx.fillText(m.val, cardX + 16, mY + 36);
        ctx.fillStyle = '#ffffff';
        ctx.font = 'bold 13px "Segoe UI", sans-serif';
        ctx.fillText(m.lbl, cardX + 16, mY + 65);
        ctx.fillStyle = '#848d97';
        ctx.font = '11px "Segoe UI", sans-serif';
        ctx.fillText(m.sub, cardX + 16, mY + 88);
        cardX += 248;
      });

      mY += 140;
      const cW = 980, cH = 300;
      ctx.fillStyle = '#161b22';
      roundRect(ctx, mX, mY, cW, cH, 8, true, true);
      
      ctx.fillStyle = '#21262d';
      roundRect(ctx, mX, mY, cW, 36, {tl: 8, tr: 8, br: 0, bl: 0}, true, false);
      ctx.fillStyle = '#848d97';
      ctx.font = '12px "Cascadia Code", monospace';
      ctx.fillText('csharp — Marknexia.Rendering/TemplateEngine.cs', mX + 16, mY + 23);

      const codeLines = [
        { tokens: [{ t: 'public sealed class ', c: '#ff7b72' }, { t: 'TemplateEngine', c: '#ffa657' }] },
        { tokens: [{ t: '{', c: '#e6edf3' }] },
        { tokens: [{ t: '    public string ', c: '#ff7b72' }, { t: 'GenerateHtml', c: '#d2a8ff' }, { t: '(string bodyHtml, RenderContext context)', c: '#e6edf3' }] },
        { tokens: [{ t: '    {', c: '#e6edf3' }] },
        { tokens: [{ t: '        // 100% Offline-first rendering with bundled CSS and Mermaid 11.4', c: '#8b949e' }] },
        { tokens: [{ t: '        var sb = new StringBuilder();', c: '#e6edf3' }] },
        { tokens: [{ t: '        sb.AppendLine("<div class=\\"markdown-body\\">");', c: '#a5d6ff' }] },
        { tokens: [{ t: '        sb.AppendLine(Sanitizer.CleanHtml(bodyHtml));', c: '#e6edf3' }] },
        { tokens: [{ t: '        return sb.ToString();', c: '#ff7b72' }] },
        { tokens: [{ t: '    }', c: '#e6edf3' }] },
        { tokens: [{ t: '}', c: '#e6edf3' }] }
      ];

      let cdY = mY + 62;
      codeLines.forEach(cl => {
        let cdX = mX + 24;
        cl.tokens.forEach(tok => {
          ctx.fillStyle = tok.c;
          ctx.font = '13px "Cascadia Code", Consolas, monospace';
          ctx.fillText(tok.t, cdX, cdY);
          cdX += ctx.measureText(tok.t).width;
        });
        cdY += 21;
      });

      return canvas.toDataURL('image/png');
    };

    window.renderPosterArt = function() {
      canvas.width = 720;
      canvas.height = 1080;

      const grad = ctx.createLinearGradient(0, 0, 720, 1080);
      grad.addColorStop(0, '#060B14');
      grad.addColorStop(0.5, '#0B152B');
      grad.addColorStop(1, '#080E1C');
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, 720, 1080);

      const halo = ctx.createRadialGradient(360, 420, 40, 360, 420, 320);
      halo.addColorStop(0, 'rgba(37, 99, 235, 0.45)');
      halo.addColorStop(0.6, 'rgba(6, 182, 212, 0.15)');
      halo.addColorStop(1, 'rgba(0, 0, 0, 0)');
      ctx.fillStyle = halo;
      ctx.fillRect(0, 0, 720, 1080);

      ctx.drawImage(logoImg, 220, 240, 280, 280);

      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 54px "Segoe UI Variable Display", sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText('Marknexia', 360, 600);

      ctx.fillStyle = '#60a5fa';
      ctx.font = '600 20px "Segoe UI", sans-serif';
      ctx.fillText('GitHub-Style Markdown. Native on Windows.', 360, 642);

      ctx.strokeStyle = 'rgba(255, 255, 255, 0.15)';
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(240, 680); ctx.lineTo(480, 680);
      ctx.stroke();

      const badges = ['100% GFM Parity', 'Offline Mermaid 11.4', 'Native WinUI 3 & .NET 10'];
      let bY = 730;
      badges.forEach(b => {
        ctx.fillStyle = 'rgba(31, 41, 55, 0.85)';
        ctx.strokeStyle = 'rgba(59, 130, 246, 0.4)';
        roundRect(ctx, 210, bY - 24, 300, 42, 21, true, true);
        ctx.fillStyle = '#e2e8f0';
        ctx.font = '600 15px "Segoe UI", sans-serif';
        ctx.fillText(b, 360, bY + 3);
        bY += 60;
      });

      ctx.textAlign = 'left';
      return canvas.toDataURL('image/png');
    };

    window.renderBoxArt = function() {
      canvas.width = 1080;
      canvas.height = 1080;

      const grad = ctx.createLinearGradient(0, 0, 1080, 1080);
      grad.addColorStop(0, '#060B14');
      grad.addColorStop(0.5, '#0E1A38');
      grad.addColorStop(1, '#070D1C');
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, 1080, 1080);

      const halo = ctx.createRadialGradient(540, 460, 60, 540, 460, 480);
      halo.addColorStop(0, 'rgba(37, 99, 235, 0.5)');
      halo.addColorStop(0.7, 'rgba(6, 182, 212, 0.15)');
      halo.addColorStop(1, 'rgba(0, 0, 0, 0)');
      ctx.fillStyle = halo;
      ctx.fillRect(0, 0, 1080, 1080);

      ctx.drawImage(logoImg, 340, 200, 400, 400);

      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 76px "Segoe UI Variable Display", sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText('Marknexia', 540, 710);

      ctx.fillStyle = '#93c5fd';
      ctx.font = '600 26px "Segoe UI", sans-serif';
      ctx.fillText('GitHub-Style Markdown Viewer for Windows', 540, 765);

      ctx.fillStyle = '#94a3b8';
      ctx.font = '400 20px "Segoe UI", sans-serif';
      ctx.fillText('Offline Mermaid Diagrams • Heading Anchors • Fluent UI', 540, 815);

      ctx.textAlign = 'left';
      return canvas.toDataURL('image/png');
    };

    window.renderAppTile = function(size) {
      canvas.width = size;
      canvas.height = size;

      ctx.fillStyle = '#0f172a';
      ctx.fillRect(0, 0, size, size);

      const grad = ctx.createRadialGradient(size/2, size/2, size*0.1, size/2, size/2, size*0.6);
      grad.addColorStop(0, '#2563eb');
      grad.addColorStop(1, '#0f172a');
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, size, size);

      const pad = size * 0.12;
      ctx.drawImage(logoImg, pad, pad, size - pad * 2, size - pad * 2);
      return canvas.toDataURL('image/png');
    };

    window.renderSuperHero = function() {
      canvas.width = 1920;
      canvas.height = 1080;

      const grad = ctx.createLinearGradient(0, 0, 1920, 1080);
      grad.addColorStop(0, '#060B14');
      grad.addColorStop(0.4, '#0C1B3A');
      grad.addColorStop(0.8, '#102A54');
      grad.addColorStop(1, '#081326');
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, 1920, 1080);

      const r1 = ctx.createRadialGradient(960, 540, 100, 960, 540, 800);
      r1.addColorStop(0, 'rgba(37, 99, 235, 0.45)');
      r1.addColorStop(0.5, 'rgba(6, 182, 212, 0.2)');
      r1.addColorStop(1, 'rgba(0, 0, 0, 0)');
      ctx.fillStyle = r1;
      ctx.fillRect(0, 0, 1920, 1080);

      ctx.drawImage(logoImg, 720, 300, 480, 480);

      ctx.strokeStyle = 'rgba(96, 165, 250, 0.2)';
      ctx.lineWidth = 1;
      for (let y = 100; y < 1080; y += 120) {
        ctx.beginPath();
        ctx.moveTo(0, y); ctx.lineTo(1920, y);
        ctx.stroke();
      }
      for (let x = 120; x < 1920; x += 180) {
        ctx.beginPath();
        ctx.moveTo(x, 0); ctx.lineTo(x, 1080);
        ctx.stroke();
      }

      return canvas.toDataURL('image/png');
    };

    window.renderXboxKeyArt = function() {
      canvas.width = 584;
      canvas.height = 800;

      const grad = ctx.createLinearGradient(0, 0, 584, 800);
      grad.addColorStop(0, '#060B14');
      grad.addColorStop(0.5, '#0D1A38');
      grad.addColorStop(1, '#060D1A');
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, 584, 800);

      ctx.drawImage(logoImg, 182, 140, 220, 220);

      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 44px "Segoe UI Variable Display", sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText('Marknexia', 292, 430);

      ctx.fillStyle = '#60a5fa';
      ctx.font = '600 16px "Segoe UI", sans-serif';
      ctx.fillText('Markdown & Repository Viewer', 292, 470);

      ctx.textAlign = 'left';
      return canvas.toDataURL('image/png');
    };

    window.renderXboxTitledHero = function() {
      canvas.width = 1920;
      canvas.height = 1080;

      const grad = ctx.createLinearGradient(0, 0, 1920, 1080);
      grad.addColorStop(0, '#060B14');
      grad.addColorStop(0.5, '#0E1D40');
      grad.addColorStop(1, '#070F20');
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, 1920, 1080);

      ctx.drawImage(logoImg, 320, 360, 360, 360);

      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 96px "Segoe UI Variable Display", sans-serif';
      ctx.fillText('Marknexia', 740, 520);

      ctx.fillStyle = '#60a5fa';
      ctx.font = '600 32px "Segoe UI", sans-serif';
      ctx.fillText('GitHub-Style Markdown. Native on Windows.', 740, 580);

      ctx.fillStyle = '#94a3b8';
      ctx.font = '400 24px "Segoe UI", sans-serif';
      ctx.fillText('Offline Mermaid 11.4 Diagrams • GFM Alert Callouts • WinUI 3', 740, 630);

      return canvas.toDataURL('image/png');
    };

    window.renderXboxPromotionalSquare = function() {
      canvas.width = 1080;
      canvas.height = 1080;

      const grad = ctx.createLinearGradient(0, 0, 1080, 1080);
      grad.addColorStop(0, '#060B14');
      grad.addColorStop(0.5, '#0F2147');
      grad.addColorStop(1, '#070F21');
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, 1080, 1080);

      const halo = ctx.createRadialGradient(540, 540, 80, 540, 540, 500);
      halo.addColorStop(0, 'rgba(37, 99, 235, 0.5)');
      halo.addColorStop(1, 'rgba(0, 0, 0, 0)');
      ctx.fillStyle = halo;
      ctx.fillRect(0, 0, 1080, 1080);

      ctx.drawImage(logoImg, 290, 290, 500, 500);
      return canvas.toDataURL('image/png');
    };
  </script>
</body>
</html>`;

fs.writeFileSync(path.join(__dirname, '../store-submission/assets/generator.html'), htmlContent, 'utf8');
console.log('generator.html created, size:', htmlContent.length);

