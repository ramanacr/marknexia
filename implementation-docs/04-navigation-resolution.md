# 04 — Navigation & URI Resolution

## Why this is a subsystem

Navigation is more complex than assigning a WebView `href`. The application must resolve local Markdown paths, repository-root paths, fragments, custom anchors, images, external URLs, and missing targets while maintaining history.

## URI classifications

Classify destinations into:

```text
Empty
FragmentOnly
RelativePath
RepositoryRootPath
AbsoluteLocalPath
Http
Https
OtherExternal
Unsupported
```

Unknown or dangerous protocols are rejected by default.

## Resolution context

```csharp
public sealed record ResolutionContext(
    string CurrentFile,
    string? RepositoryRoot,
    string? CurrentDocumentUri,
    NavigationPolicy Policy);
```

## Path resolution algorithm

### Fragment-only

Example:

```text
#authentication
```

Resolution:

```text
Current document
        ↓
Anchor index
        ↓
#authentication
        ↓
Navigate in current document
```

### Relative Markdown path

Example:

```text
../api/authentication.md#login
```

Resolve relative to the current Markdown file's directory.

### Repository-root path

Example:

```text
/docs/api.md#authentication
```

When repository context exists, resolve from repository root.

If no repository context exists, do not guess an arbitrary Windows drive-root interpretation. Show a diagnostic and preserve the link as unresolved.

### External

`http` and `https` are external navigation candidates. Default policy should open them through the system/browser rather than granting embedded documents unrestricted network access.

## Fragment lookup

The rendered document must create an anchor index:

```text
fragment string -> AnchorTarget
```

An `AnchorTarget` should contain:

- Element identifier.
- Document location.
- Optional source line/offset.
- Whether it is heading-generated or custom.

## Custom anchors

Support HTML anchor patterns such as:

```html
<a name="my-custom-anchor"></a>
```

subject to the sanitizer preserving the safe anchor semantics.

Support `id="..."` anchors if present and safe.

## Cross-document fragment navigation

Required sequence:

1. Classify destination.
2. Resolve target path.
3. Verify target exists and is an allowed Markdown document.
4. Open target tab or reuse existing tab according to tab policy.
5. Read source.
6. Parse/render.
7. Build anchor index.
8. Resolve fragment.
9. Scroll/focus target.
10. Commit navigation-history entry.

Never try to scroll the old document before the target document has rendered.

## History

History entry:

```csharp
public sealed record HistoryEntry(
    DocumentUri Document,
    string? Fragment,
    double ScrollTop,
    DateTimeOffset CreatedAt);
```

The history model should support back/forward through document and fragment changes.

## Heading anchor behavior

A heading link/copy-anchor command should expose the final generated anchor.

Example:

```text
Heading: Database Architecture
Anchor: #database-architecture
```

## Broken links

Broken navigation should be diagnostic, not catastrophic:

- Keep document visible.
- Show lightweight notification/toast.
- Optionally annotate the offending link temporarily.
- Provide a diagnostic in a debug/developer panel.

## Security invariants

Path traversal cannot escape an explicitly defined root when repository sandboxing is enabled.

Example:

```text
Repository root = C:epo
Current file    = C:epo\docspi.md
Target          = ../../secret.md
```

The resolver must detect that the canonical target is outside `C:epo` and reject it in repository sandbox mode.

## Test vectors

Must include:

```text
#foo
./other.md
../README.md
other.md#foo
./docs/other.md#foo
/docs/other.md
/docs/other.md#foo
https://example.com
mailto:user@example.com
javascript:alert(1)
```
