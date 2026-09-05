# Marknexia — Microsoft Store Listing Details

Use the following metadata when submitting Marknexia in Microsoft Partner Center (https://partner.microsoft.com/dashboard).

---

## 1. Product Identification
* **Product Name**: Marknexia
* **Category**: Developer tools > Documentation & Utilities
* **Pricing & Availability**: Free / Worldwide (or select target markets)
* **Visibility**: Public (Discoverable in Store search)

---

## 2. Store Listing Copy

### Product Title
```text
Marknexia — GitHub-Style Markdown & Repository Viewer
```

### Tagline / Short Description (100 characters max)
```text
GitHub-style Markdown. Native on Windows. Read, explore, and understand documentation repositories.
```

### Full Description
```markdown
Marknexia is a high-performance, offline-first native Windows Markdown viewer and documentation repository browser built with .NET 10, C# 13, and WinUI 3 (Windows App SDK).

Engineered specifically for developers, architects, and engineering teams, Marknexia resolves the common frustrations of reading documentation repositories locally: broken relative links, non-functional repository-root `/` paths, unresponsive cross-file anchors, missing offline diagrams, and sluggish web-wrapper rendering.

KEY FEATURES:

⚡ 100% GITHUB-STYLE MARKDOWN (GFM)
- GitHub Alert Callouts: Native rendering of [!NOTE], [!TIP], [!IMPORTANT], [!WARNING], and [!CAUTION] callouts with authentic GitHub Primer iconography and colors.
- Deterministic Heading Anchors: GitHub-identical heading slug generation for full compatibility with existing GitHub READMEs and wikis.
- Extended GFM elements: Tables, task lists with checkboxes, strikethrough, footnotes, and autolinks.
- GitHub Primer Themes: True GitHub Light and Dark theme tokens.

📊 OFFLINE-FIRST DIAGRAMS & MATH
- Bundled Mermaid 11.4 Runtime: Flowcharts, Sequence Diagrams, State Diagrams, Class Diagrams, ER Diagrams, and Gantt charts without internet access.
- Interactive Diagram Containers: Built-in "Copy Source" and "Toggle Source / Diagram" controls with isolated error fallback boundaries.
- LaTeX / KaTeX math support.

🧭 RESILIENT REPOSITORY NAVIGATION
- Intra-document smooth scrolling to internal heading anchors.
- Relative cross-file link resolution (./docs/setup.md, ../architecture/adr.md).
- Cross-document anchor targeting (./api.md#auth).
- Repository-root paths: Absolute links prefixed with / automatically resolve relative to the detected repository root (.git boundary marker).
- Full browser-grade Back and Forward navigation history.
- Directory traversal sandboxing protecting against unauthorized filesystem escapes.

📑 MODERN WINUI 3 DESKTOP SHELL
- Multi-document tabs (TabView) for concurrent reading.
- Hierarchical document outline sidebar (TreeView) extracted from headings (H1–H6).
- Repository workspace explorer for quick file browsing.
- In-page search (Ctrl+F) with match highlighting and match counters.
- Native Windows file associations for .md, .markdown, .mdown, and .mkdn.

🛡️ ENTERPRISE-GRADE SECURITY
- Sanitized DOM execution powered by Ganss.Xss sanitizer.
- Blocks scripts, inline event handlers, and malicious SVGs.
- Whitelisted protocol support (file:, http:, https:, mailto:).
```

### Feature Bullets (Up to 20)
1. 100% GitHub-style Markdown (GFM) parity with native GitHub Alert callouts
2. Bundled offline Mermaid 11.4 diagram rendering (Flowcharts, Sequence, Class, ER, Gantt)
3. Repository-aware link navigation resolving relative links and repo-root `/` paths
4. Cross-document anchor jump navigation with full history back/forward
5. Multi-document tabs with Mica and Fluent design
6. Hierarchical outline tree sidebar auto-generated from document headings
7. In-page instant search with match highlighting and keyboard navigation
8. Fast, offline-first execution with zero external runtime network dependencies
9. Enterprise-grade DOM sanitization and directory traversal sandboxing
10. Native Windows shell file associations for .md, .markdown, .mdown, .mkdn

### Search Keywords (7 terms max)
1. markdown
2. markdown viewer
3. github markdown
4. mermaid
5. gfm
6. documentation
7. repository browser

---

## 3. URLs & Support
* **Website / Documentation**: https://ramanacr.github.io/marknexia/
* **Support Contact / Issues**: https://github.com/ramanacr/marknexia/issues
* **Privacy Policy URL**: https://ramanacr.github.io/marknexia/#privacy
* **Copyright**: Copyright (c) 2026 Marknexia. Released under the MIT License.

---

## 4. Required Package & Visual Assets
* **Package File**: `store-submission/Marknexia-v1.0.0-win-x64.msix`
* **Store Logo**: `store-submission/assets/StoreLogo.png` (50x50)
* **Square 150x150 Logo**: `store-submission/assets/Square150x150Logo.png`
* **Square 44x44 Logo**: `store-submission/assets/Square44x44Logo.png`
* **Splash Screen**: `store-submission/assets/SplashScreen.png` (620x300)
* **Hero / Banner Art**: `store-submission/assets/marknexia-store-hero.png` (1920x1080)
