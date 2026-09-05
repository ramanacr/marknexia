# 05 — File System & Repository Model

## File identity

Use canonical absolute paths plus a normalized case strategy appropriate to Windows.

A document identity must not depend only on the display name.

## Encoding

Support common Unicode encodings. Prefer UTF-8. Handle BOMs correctly. If encoding is ambiguous, expose a diagnostic rather than silently corrupting content.

## Repository context

```csharp
public sealed record RepositoryContext(
    string RootPath,
    RepositoryKind Kind,
    bool EnforceRootSandbox);
```

`RepositoryKind` can initially be `Folder` and later expand to `GitRepository` without changing the renderer.

## Opening a file

When a user opens `C:epo\README.md`:

1. Canonicalize the file path.
2. Load the file.
3. Detect a plausible repository root if repository detection is enabled.
4. Create a document context.
5. Render.

Repository detection must never perform network access.

## Folder mode

Opening a folder creates a workspace root and exposes a file tree.

Recommended default filters:

- Show Markdown files.
- Show common image/resource assets.
- Hide `.git` by default.
- Hide build/output directories by default.
- Allow users to reveal ignored/hidden files.

## Symlinks and junctions

Windows reparse points require explicit security handling. In sandboxed repository mode:

- Canonicalize the final path.
- Check the canonical path is inside the permitted root.
- Decide whether reparse-point traversal is allowed.
- Log/reveal a diagnostic when blocked.

## File watching

Future-friendly design:

```text
FileChanged event
    ↓
Debounce
    ↓
Invalidate source cache
    ↓
Reparse
    ↓
Preserve fragment/scroll if possible
```

Do not auto-reload aggressively in v1 without a user-visible setting.
