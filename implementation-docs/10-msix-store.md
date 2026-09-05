# 10 — MSIX & Microsoft Store Plan

## Packaging decision

Use a **packaged WinUI 3 / Windows App SDK application with MSIX**.

Microsoft currently recommends packaged distribution for new WinUI 3 applications, and packaged WinUI 3 apps are produced as MSIX/MSIXBundle by the standard tooling.

## Package goals

- Store submission-ready package.
- x64 and ARM64 variants/bundle as appropriate.
- File associations for Markdown extensions.
- Clean uninstall.
- Store-managed updates.
- Minimal capabilities.

## File associations

Register:

```text
.md
.markdown
.mdown
.mkdn
```

The app should be discoverable as an “Open with” target.

## Capabilities

Request the minimum capabilities required by the actual implementation.

Do not declare broad network or file-system capabilities merely for convenience.

## Store workflow

1. Reserve product name in Partner Center.
2. Configure package identity.
3. Build Release MSIX/MSIXBundle.
4. Validate package.
5. Run Windows App Certification Kit.
6. Test clean install / update / uninstall.
7. Prepare Store listing.
8. Upload packages.
9. Configure pricing/availability and age rating.
10. Submit for certification.
11. Monitor certification feedback.

Microsoft documents MSIX upload and Store submission as the normal path for packaged applications.

## Versioning

Keep semantic product versioning separate from package version constraints.

Use monotonically increasing package versions suitable for Store update ordering.

## Signing

For Microsoft Store distribution, the Store handles package signing/re-signing as part of certification/distribution. Local sideloading/development still requires the normal development certificate/install trust flow.

## Store assets

Prepare:

- App icon set.
- Store logo.
- Screenshots.
- Feature descriptions.
- Privacy statement.
- Support URL.
- Release notes.
- Age-rating questionnaire.

## Microsoft Store references

- Packaging overview: https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/packaging/
- Get started with Microsoft Store: https://learn.microsoft.com/en-us/windows/apps/publish/get-started
- Create an app submission for MSIX: https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/create-app-submission
- Upload MSIX packages: https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/upload-app-packages
