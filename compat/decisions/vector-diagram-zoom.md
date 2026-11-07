# Vector diagram zoom: zoom diagrams by re-layout, not by a bitmap scale (Rust edition)

Status: **approved — owner decision 2026-11-07**

Scope: the bundled page assets `crates/marknexia-rendering/assets/bridge.js` and `crates/marknexia-rendering/assets/github-markdown.css`, and the page-parity tests `crates/marknexia-rendering/tests/parity_v1.rs` and `math_oracle.rs` with the shared `tests/support/asset_patches.rs` (new `parity-asset-patch` blocks). Owner report 2026-11-07: zooming a Mermaid diagram or a text diagram makes it blurry; diagrams must stay vector-sharp at every zoom level. No .NET source and no frozen fixture is changed.

Evidence class: implemented and headless-tested in Chrome (the WebView2 engine) through the bridge functions and real compositor frames. **Not run:** the native shell and WebView2 itself, high-DPI displays other than 100%, a screen reader.

## Cause

The .NET bridge zooms with `canvas.style.transform = translate(...) scale(zoom)`, and the stylesheet gives the canvas `will-change: transform`. Chromium rasterises a `will-change: transform` layer once, at 100%, and then only scales those pixels when the transform changes. The SVG paths and the text are therefore magnified bitmaps at 200%, 300% and 400%. The .NET edition has the same blur, because it ships the same two assets.

## Observable differences

| ID | Condition | .NET oracle | Rust edition |
| --- | --- | --- | --- |
| VZ-1 | A diagram (Mermaid or text diagram) zoomed away from 100% | `transform: translate(x, y) scale(z)` on a permanent compositor layer: the content is blurred above 100% and soft below it. | CSS `zoom: z` on `.marknexia-diagram-canvas`. Chromium lays out the canvas again and rasterises the SVG and the text at the new size, so edges stay sharp at every level from 25% to 400%. The canvas gets a fixed `width` (its 100% layout width) and `min-width: 0` while zoomed, so the Mermaid SVG (`width: 100%` with a `max-width`) scales by exactly `z` and does not just fill the viewport. At 100% the bridge removes all of these styles. |
| VZ-2 | Pan (drag) | Part of the same transform. | A translate only, without scale. The pan is kept in screen pixels and written as `translate(panX / z, panY / z)`, because CSS zoom also scales the canvas's own transform lengths. |
| VZ-3 | Compositor layer | `will-change: transform` always. | Only while dragging (`.marknexia-diagram-viewport.is-panning .marknexia-diagram-canvas`), removed on `pointerup` and `pointercancel`; the canvas at rest is painted at the current zoom. |
| VZ-4 | Viewport size while zoomed | The transform does not change layout, so the viewport keeps its height and the zoomed canvas scrolls inside it. | The same: the bridge fixes the viewport `height` at its 100% height while zoomed (not in full-window mode, where the viewport fills the window). The scroll area now includes the bottom padding of the viewport. |
| VZ-5 | Mouse-wheel zoom anchor | Computed from the viewport origin, without its padding and scroll offset. | Computed from the canvas's on-screen origin, then corrected once after the re-layout (for example when the viewport's scroll position is clamped as the content shrinks). Measured drift of the point under the pointer: 0 px horizontally and at most 0.04 diagram pixels vertically over four wheel steps. |
| VZ-6 | Full window, Mermaid re-render, hidden viewport and window resize | No re-layout needed. | Opening or closing full window, and a Mermaid re-render (first render or system theme change) while zoomed, lay the canvas out again for the new viewport or drawing. While the source is shown, the viewport is hidden and cannot be measured: a zoom change (the zoom buttons stay visible) or a re-render then leaves the layout pending, and showing the diagram again (`toggleSource`) lays it out. A window resize lays every zoomed diagram out again for the new width, 150 ms after the last resize event. |

The zoom status, the reset button text, the `is-zoomed` class, the zoom limits and step (25% to 400% in steps of 25%), the keyboard controls (`+`, `=`, `-`, `_`, `0`, Escape), the full-window mode and every aria label are unchanged.

## Content Security Policy

The page CSP is unchanged: `script-src 'nonce-…' https://marknexia.assets` and `style-src 'unsafe-inline'`. The bridge sets styles only through the CSSOM (`element.style.zoom`, `width`, `minWidth`, `height`, `transform`), the same mechanism the .NET bridge already uses for `transform`. No `<style>` element, `style` attribute in markup, `eval` or new script source is added, and the template, nonce model and sanitizer are not changed.

## Verification (headless Chrome)

The page was rendered with `cargo run -p marknexia-rendering --example text_diagram_preview` from a Markdown file with one Mermaid flowchart and one text diagram, loaded in headless Chrome with the real CSP; the pinned Mermaid runtime (`src/Marknexia.Rendering/Assets/mermaid.min.js`) was served for `https://marknexia.assets/mermaid.min.js` by DevTools request interception. Zoom was set to 300% through the bridge function `adjustDiagramZoom` in eight steps, one frame apart. Frames were taken from the compositor (`Page.startScreencast`): `Page.captureScreenshot` repaints the page and hides the blur.

| Measure at 300% (viewport crop, 978 px wide) | Old transform | CSS zoom |
| --- | --- | --- |
| Mermaid: mean squared gradient | 131.8 | 461.0 |
| Mermaid: largest one-pixel luminance step | 76 | 204 |
| Text diagram: mean squared gradient | 411.4 | 1539.8 |
| Text diagram: largest one-pixel luminance step | 79 | 232 |
| Mermaid SVG width (100% is 754.97 px) | 2264.91 px | 2264.91 px |

Text layer against the SVG grid of the text diagram (36 characters, each compared with its `1ch` by `2ch` cell): the SVG box equals the figure box exactly at 50%, 100%, 200% and 400%. The largest horizontal offset of a character from its cell is 0.012 px; the largest vertical offset of a glyph centre from its cell centre is 0.8 px at 50% (0.06 px with the old transform; the font is 6.5 px there and the browser rounds its line metrics), and 0.12 px, 0.23 px and 0.47 px at 100%, 200% and 400%, the same as the old transform.

Other checks: a 100 px by 40 px drag at 300% moves the canvas by exactly 100 px by 40 px, with `will-change: transform` during the drag and `auto` after it; the viewport height is the same at 50%, 100% and 300%; the reset (`0`) removes every style the bridge set. With the source shown, zooming both diagrams to 300% with the buttons leaves the layout pending, and showing the diagram again gives the canvas its 100% width (946 px) and the Mermaid SVG exactly 3 × 754.97 px; a system theme change (Mermaid re-render) while hidden at 200% behaves the same. After a resize from 1200 px to 760 px wide at 200%, the canvas width becomes the new 100% width (630 px) and the Mermaid SVG is 1196 px, exactly 2 × its new 100% width (598 px).

## Parity gate

Every frozen rendered page embeds both assets, so no rendered page keeps the .NET bytes. Instead of whole-page allowances, the gate now reads `parity-asset-patch` blocks: each names an asset, a `find` text (a JSON string that must occur exactly once when the blocks are applied in order) and its `replace` text, and needs one of the accepted status lines. `bundled_assets_match_the_dotnet_sources` checks that each bundled asset equals the .NET asset with exactly these edits, and `parity_v1` (5 of the 7 frozen rendering cases) and `math_oracle` (its one complete-page case) replace the embedded .NET asset in each frozen page with that patched text before they compare the page. The shared code is `crates/marknexia-rendering/tests/support/asset_patches.rs`. Any other difference still fails, and a stale or ambiguous edit fails too. If the owner rejects this record, removing it and restoring the two assets returns the gate to byte-identical .NET pages.

## Recorded asset edits

The blocks below are generated from the .NET assets and the bundled assets, in file order.

<!-- parity-asset-patch
asset: github-markdown.css
find: "  transform-origin: 0 0;\n  will-change: transform;\n"
replace: "  transform-origin: 0 0;\n}\n\n/* A composited layer only while dragging; at rest the canvas is painted\n   (and its vector content rasterised) at the current zoom. */\n.marknexia-diagram-viewport.is-panning .marknexia-diagram-canvas {\n  will-change: transform;\n"
-->

<!-- parity-asset-patch
asset: bridge.js
find: "        viewEl.style.display = 'block';\n      }\n"
replace: "        viewEl.style.display = 'block';\n        // A zoom changed while the source was shown is laid out now.\n        const diagram = document.getElementById(diagramId);\n        if (diagram && diagram._marknexiaZoomState) updateDiagramZoomUi(diagram);\n      }\n"
-->

<!-- parity-asset-patch
asset: bridge.js
find: "  document.querySelectorAll('.marknexia-mermaid').forEach(installDiagramInteractions);\n  if (typeof mermaid !== 'undefined') {\n"
replace: "  document.querySelectorAll('.marknexia-mermaid').forEach(installDiagramInteractions);\n  // A zoomed canvas is fixed at its 100% width: lay it out again for the\n  // new viewport width after the window is resized.\n  let diagramResizeTimer = 0;\n  window.addEventListener('resize', () => {\n    clearTimeout(diagramResizeTimer);\n    diagramResizeTimer = setTimeout(() => {\n      document.querySelectorAll('.marknexia-mermaid').forEach(diagram => {\n        if (diagramState(diagram).zoom !== 1) updateDiagramZoomUi(diagram, true);\n      });\n    }, 150);\n  });\n  if (typeof mermaid !== 'undefined') {\n"
-->

<!-- parity-asset-patch
asset: bridge.js
find: "            mermaid.render(renderId, code).then(res => {\n              if (container.dataset.marknexiaMermaidRenderId === renderId) preEl.innerHTML = res.svg;\n            }).catch(err => {\n"
replace: "            mermaid.render(renderId, code).then(res => {\n              if (container.dataset.marknexiaMermaidRenderId !== renderId) return;\n              preEl.innerHTML = res.svg;\n              // The drawing replaced the source text: size a zoomed canvas again.\n              if (diagramState(container).zoom !== 1) updateDiagramZoomUi(container, true);\n            }).catch(err => {\n"
-->

<!-- parity-asset-patch
asset: bridge.js
find: "  if (!diagram._marknexiaZoomState) {\n    diagram._marknexiaZoomState = { zoom: 1, panX: 0, panY: 0, pointerId: null, lastX: 0, lastY: 0 };\n  }\n"
replace: "  if (!diagram._marknexiaZoomState) {\n    diagram._marknexiaZoomState = { zoom: 1, layoutZoom: 1, panX: 0, panY: 0, pointerId: null, lastX: 0, lastY: 0 };\n  }\n"
-->

<!-- parity-asset-patch
asset: bridge.js
find: "\nfunction updateDiagramZoomUi(diagram) {\n  const state = diagramState(diagram);\n"
replace: "\n// Diagrams zoom by re-layout, not by a bitmap scale: CSS `zoom` makes\n// Chromium lay out and rasterise the SVG and its text again at the new size,\n// so they stay sharp. A `scale()` transform on a composited layer only\n// magnifies the pixels rasterised at 100%.\n// Returns false when the viewport is hidden (the source is shown): nothing\n// can be measured then, so the layout is left for when it is shown again.\nfunction layoutDiagramCanvas(diagram, canvas, viewport, zoom) {\n  canvas.style.zoom = '';\n  canvas.style.width = '';\n  canvas.style.minWidth = '';\n  if (viewport) viewport.style.height = '';\n  if (zoom === 1) return true;\n  if (canvas.getClientRects().length === 0) return false;\n  // Fix the canvas at its 100% layout width, so content sized in percent\n  // (the Mermaid SVG is `width: 100%` with a `max-width`) scales with the\n  // zoom instead of filling the viewport.\n  canvas.style.width = `${canvas.getBoundingClientRect().width}px`;\n  canvas.style.minWidth = '0';\n  // Keep the viewport at its 100% height, as the transform did: the zoomed\n  // canvas scrolls inside it instead of moving the rest of the page.\n  if (viewport && !diagram.classList.contains('is-expanded')) {\n    viewport.style.height = getComputedStyle(viewport).height;\n  }\n  canvas.style.zoom = String(zoom);\n  return true;\n}\n\nfunction applyDiagramPan(canvas, state) {\n  // The pan is kept in screen pixels. CSS zoom also scales the canvas's own\n  // transform lengths, so it is given in unzoomed pixels.\n  canvas.style.transform = state.panX === 0 && state.panY === 0\n    ? ''\n    : `translate(${state.panX / state.zoom}px, ${state.panY / state.zoom}px)`;\n}\n\nfunction updateDiagramZoomUi(diagram, relayout) {\n  const state = diagramState(diagram);\n"
-->

<!-- parity-asset-patch
asset: bridge.js
find: "  const status = diagram.querySelector('[data-marknexia-zoom-status]');\n  if (canvas) canvas.style.transform = `translate(${state.panX}px, ${state.panY}px) scale(${state.zoom})`;\n  const percent = `${Math.round(state.zoom * 100)}%`;\n"
replace: "  const status = diagram.querySelector('[data-marknexia-zoom-status]');\n  if (canvas) {\n    if (relayout || state.layoutZoom !== state.zoom) {\n      // `null` marks the layout as pending: the next update lays it out.\n      state.layoutZoom = layoutDiagramCanvas(diagram, canvas, viewport, state.zoom) ? state.zoom : null;\n    }\n    applyDiagramPan(canvas, state);\n  }\n  const percent = `${Math.round(state.zoom * 100)}%`;\n"
-->

<!-- parity-asset-patch
asset: bridge.js
find: "    state.previousFocus = null;\n  }\n}\n\n"
replace: "    state.previousFocus = null;\n  }\n  // The viewport changed size: lay the zoomed canvas out again.\n  updateDiagramZoomUi(diagram, true);\n}\n\n"
-->

<!-- parity-asset-patch
asset: bridge.js
find: "    if (nextZoom === oldZoom) return;\n    const bounds = viewport.getBoundingClientRect();\n    const anchorX = event.clientX - bounds.left - state.panX;\n    const anchorY = event.clientY - bounds.top - state.panY;\n    const ratio = nextZoom / oldZoom;\n"
replace: "    if (nextZoom === oldZoom) return;\n    const canvas = diagram.querySelector('.marknexia-diagram-canvas');\n    if (!canvas) return;\n    // Keep the diagram point under the pointer in place. Offsets are in\n    // screen pixels from the canvas's on-screen origin.\n    const origin = canvas.getBoundingClientRect();\n    const anchorX = event.clientX - origin.left;\n    const anchorY = event.clientY - origin.top;\n    const ratio = nextZoom / oldZoom;\n"
-->

<!-- parity-asset-patch
asset: bridge.js
find: "    updateDiagramZoomUi(diagram);\n  }, { passive: false });\n"
replace: "    updateDiagramZoomUi(diagram);\n    // The re-layout can move the canvas (the viewport's scroll position is\n    // clamped when the content shrinks): correct the pan by what it moved.\n    const moved = canvas.getBoundingClientRect();\n    const errorX = event.clientX - anchorX * ratio - moved.left;\n    const errorY = event.clientY - anchorY * ratio - moved.top;\n    if (Math.abs(errorX) >= 0.5 || Math.abs(errorY) >= 0.5) {\n      state.panX += errorX;\n      state.panY += errorY;\n      updateDiagramZoomUi(diagram);\n    }\n  }, { passive: false });\n"
-->
