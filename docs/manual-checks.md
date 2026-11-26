# Manual checks for the Rust edition (owner)

These checks need real hardware or a clean machine, so CI cannot run them. They close the
remaining items of Phase 4 (accessibility) and Phase 5 (WebView2 distribution) in
`docs/superpowers/plans/2026-11-06-rust-win32-production.md`.

Use the signed pre-release
[`rust-preview-0.1.0-signed.1`](https://github.com/ramanacr/marknexia/releases/tag/rust-preview-0.1.0-signed.1):
`marknexia-rust-preview-setup-0.1.0-x64.exe` (or `-arm64.exe` on an ARM64 PC).

For each step, write **Pass**, **Fail** or **N/A** in the Result column. For a failure, add a
screenshot and one sentence about what you saw. Send the filled-in file back, or paste the failed
rows into the chat.

Already covered by CI on every change (no need to repeat): runtime high contrast on and off, the
Tab and Shift+Tab order of every control, control names for screen readers, and keyboard focus.

## A. Before you start

| # | Step | Result |
|---|---|---|
| A1 | Install the setup exe. Windows shows the publisher as **Ramana Reddy Chamakura**, not "Unknown publisher". | |
| A2 | Start Marknexia (Rust preview). Open a folder of Markdown files with relative links (for example your wiki, `D:\SCIMAX_GIT\Publications_With_NewUI\Project_Docs\wiki`). | |

## B. Two monitors with different scaling

Setup: two monitors, one at 100 % and one at 150 % or more (Settings > System > Display > Scale).
Change the scaling only for this test and set it back afterwards.

| # | Step | Expected | Result |
|---|---|---|---|
| B1 | Start Marknexia on the 100 % monitor. | Toolbar, tabs, sidebar, status bar and document are sharp and correctly sized. | |
| B2 | Drag the window to the 150 % monitor and release it. | Within a second, the window, fonts, icons, tab strip and sidebar resize to the new scale. No blurry text, no clipped buttons, no overlapping controls. | |
| B3 | On the 150 % monitor, open the sidebar, open a second tab, open the find bar (Ctrl+F). | All fit, nothing is cut off. | |
| B4 | Drag the window so it sits half on each monitor, then fully back to the 100 % monitor. | It ends up sized for 100 % again, with no leftover large controls. | |
| B5 | Maximize on the 150 % monitor, restore, close. Start again. | It opens where it was, at the right size for that monitor. | |
| B6 | Change the scale of the monitor the window is on while Marknexia runs (for example 150 % to 125 %). | Layout updates without a restart. | |
| B7 | Zoom a Mermaid or ASCII diagram (Ctrl+wheel on the diagram) on the 150 % monitor. | The diagram stays sharp when zoomed. | |

## C. Narrator listening pass

Setup: start Narrator with **Ctrl+Win+Enter**; stop it the same way. Use only the keyboard.

| # | Step | Expected (what Narrator says) | Result |
|---|---|---|---|
| C1 | Focus the Marknexia window (Alt+Tab). | "Marknexia" and the window type. | |
| C2 | Press Tab through the toolbar. | Each button's name and "button" (for example "Open Markdown file, button"). Nothing is silent or read as "unknown". | |
| C3 | Tab to the document tabs, then use Left and Right arrows. | "<file name>, tab item" (position such as "2 of 3" if Narrator reads it), and selecting a tab announces the new selection. | |
| C4 | Tab to the sidebar view selector and the outline or repository tree; move with the arrow keys. | Each heading or file name, its level or expanded/collapsed state. | |
| C5 | Tab into the document; read it with Narrator's reading keys (Caps Lock+Down arrow or Insert+Down arrow). | Headings, paragraphs, lists, tables and code blocks are read in order. Links are read as "link". | |
| C6 | In the document, Tab to a relative link and press Enter. | Narrator announces the new document. Back (Alt+Left) announces the previous one. | |
| C7 | Press Ctrl+F, type a word, press Enter. | "Find in document, edit" and the match count. | |
| C8 | Zoom a diagram (Ctrl+mouse wheel on it). | The zoom level is announced (it is not shown on screen). | |
| C9 | Open the Theme selector and pick another theme. | The selector and the new value are read. | |
| C10 | Anything Narrator reads wrongly, twice, or not at all. | Note it with the step number. | |

## D. Clean machine without WebView2

The Rust edition shows the document with Microsoft WebView2. Windows 11 always has WebView2, so
this check needs a machine without it, for example a fresh **Windows 10** virtual machine that has
not yet installed Microsoft Edge updates. Check first: in the VM, *Settings > Apps* must not list
"Microsoft Edge WebView2 Runtime". (Assumption: such an image still lacks WebView2; if it has it,
note N/A for D2–D6.)

| # | Step | Expected | Result |
|---|---|---|---|
| D1 | Copy the setup exe to the VM and install it. | Installs; the publisher is Ramana Reddy Chamakura. | |
| D2 | Start Marknexia **offline** (disconnect the VM network). | The window opens and stays usable. Instead of a document it shows that WebView2 is missing, a button **Install WebView2 Runtime** and a link **Get Microsoft's offline installer**. No crash, no error dialog loop. | |
| D3 | Still offline, click **Install WebView2 Runtime**. | A clear failure message (no download possible); the app stays usable; you can try again. | |
| D4 | Connect the network and click **Install WebView2 Runtime** again. | It downloads Microsoft's installer, Windows may ask for permission, the install finishes, and Marknexia says "WebView2 Runtime <version> is installed." | |
| D5 | After the install, open a Markdown file. | The document renders without restarting the app, or after one restart; note which. | |
| D6 | Close and restart Marknexia. | It starts normally and shows the document directly. | |
| D7 | Uninstall Marknexia from *Settings > Apps*. | It is removed; no Marknexia install folder is left behind (your settings folder may stay). | |

## E. Sign-off

| Item | Done by | Date |
|---|---|---|
| B. Mixed scaling | | |
| C. Narrator | | |
| D. Clean machine | | |
