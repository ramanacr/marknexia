//! Recorded edits of the bundled page assets (`parity-asset-patch` blocks
//! in `compat/decisions/*.md`), shared by the tests that compare complete
//! pages with frozen .NET output (`parity_v1`, `math_oracle`).
//!
//! The bundled `github-markdown.css` and `bridge.js` are the .NET assets
//! plus the edits these blocks record. Each block names the asset, a `find`
//! text (a JSON string) that must occur exactly once when the blocks are
//! applied in file order, and its `replace` text, and is accepted only in a
//! record with exactly one accepted status line.

use std::{fs, path::PathBuf};

/// Status of a record added after gate G0 and not yet approved.
pub(crate) const PROPOSED_STATUS: &str = "proposed — requires Task 8 approval";
/// Status of a record the owner approved at gate G0 (2026-11-06).
pub(crate) const APPROVED_STATUS: &str = "approved — owner decision 2026-11-06 (G0)";
/// The only statuses under which a parity allowance is accepted.
/// Records approved after G0 by the owner's 2026-11-07 decision.
pub(crate) const APPROVED_LATER_STATUS: &str = "approved — owner decision 2026-11-07";
pub(crate) const ALLOWANCE_STATUSES: [&str; 3] =
    [APPROVED_STATUS, APPROVED_LATER_STATUS, PROPOSED_STATUS];
pub(crate) const ASSET_PATCH_OPEN: &str = "<!-- parity-asset-patch";
/// The bundled assets that the frozen pages embed.
pub(crate) const ASSETS: [&str; 2] = ["github-markdown.css", "bridge.js"];

pub(crate) fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub(crate) fn status_line(text: &str) -> Option<&str> {
    let mut statuses = text
        .lines()
        .filter(|line| line.trim_start().starts_with("Status:"));
    let status = statuses
        .next()?
        .trim()
        .strip_prefix("Status: **")?
        .strip_suffix("**")?;
    statuses.next().is_none().then_some(status)
}

/// The bodies of the comment blocks that start with `open`.
pub(crate) fn comment_blocks<'a>(
    decision: &str,
    text: &'a str,
    open: &str,
) -> Result<Vec<&'a str>, String> {
    let mut bodies = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let body_start = start + open.len();
        let end = rest[body_start..]
            .find("-->")
            .ok_or_else(|| format!("{decision}: unterminated {}", &open[5..]))?;
        bodies.push(&rest[body_start..body_start + end]);
        rest = &rest[body_start + end + 3..];
    }
    Ok(bodies)
}

/// The value of a `name: value` line in a block body.
pub(crate) fn block_field(decision: &str, body: &str, name: &str) -> Result<String, String> {
    body.lines()
        .find_map(|line| line.trim().strip_prefix(&format!("{name}:")))
        .map(|value| value.trim().to_owned())
        .ok_or_else(|| format!("{decision}: block missing `{name}`"))
}

/// Blocks count only in a record with exactly one accepted status line.
pub(crate) fn require_status(decision: &str, text: &str) -> Result<(), String> {
    if status_line(text).is_some_and(|status| ALLOWANCE_STATUSES.contains(&status)) {
        Ok(())
    } else {
        Err(format!(
            "{decision}: parity allowances require the line `Status: **{APPROVED_STATUS}**` or `Status: **{PROPOSED_STATUS}**`"
        ))
    }
}

/// A recorded edit of a bundled asset relative to the .NET asset.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AssetPatch {
    pub(crate) decision: String,
    pub(crate) asset: String,
    pub(crate) find: String,
    pub(crate) replace: String,
}

pub(crate) fn parse_asset_patches(decision: &str, text: &str) -> Result<Vec<AssetPatch>, String> {
    let mut patches = Vec::new();
    for body in comment_blocks(decision, text, ASSET_PATCH_OPEN)? {
        let text_field = |name: &str| -> Result<String, String> {
            serde_json::from_str::<String>(&block_field(decision, body, name)?)
                .map_err(|error| format!("{decision}: `{name}` is not a JSON string: {error}"))
        };
        let asset = block_field(decision, body, "asset")?;
        if !ASSETS.contains(&asset.as_str()) {
            return Err(format!("{decision}: unknown asset `{asset}`"));
        }
        let (find, replace) = (text_field("find")?, text_field("replace")?);
        if find.is_empty() || find == replace {
            return Err(format!(
                "{decision}: empty or no-op asset patch for {asset}"
            ));
        }
        patches.push(AssetPatch {
            decision: decision.to_owned(),
            asset,
            find,
            replace,
        });
    }
    if !patches.is_empty() {
        require_status(decision, text)?;
    }
    Ok(patches)
}

/// Every decision record as (file name, text), in file-name order.
pub(crate) fn decision_files() -> Vec<(String, String)> {
    let mut entries: Vec<_> = fs::read_dir(repository_root().join("compat/decisions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(&path).unwrap())
        })
        .collect()
}

pub(crate) fn decision_asset_patches() -> Vec<AssetPatch> {
    decision_files()
        .iter()
        .flat_map(|(name, text)| parse_asset_patches(name, text).unwrap())
        .collect()
}

/// `source` with the patches for `asset` applied in order. Each `find` must
/// occur exactly once in the text it is applied to, so a stale or ambiguous
/// patch fails.
pub(crate) fn apply_asset_patches(
    asset: &str,
    source: &str,
    patches: &[AssetPatch],
) -> Result<String, String> {
    let mut text = source.to_owned();
    for patch in patches.iter().filter(|patch| patch.asset == asset) {
        match text.matches(patch.find.as_str()).count() {
            1 => text = text.replacen(patch.find.as_str(), &patch.replace, 1),
            count => {
                return Err(format!(
                    "{}: {asset} patch `find` occurs {count} times, not once: {:?}",
                    patch.decision,
                    patch.find.chars().take(80).collect::<String>()
                ));
            }
        }
    }
    Ok(text)
}

/// The .NET oracle asset, newline-normalized.
pub(crate) fn dotnet_asset(name: &str) -> String {
    let path = repository_root()
        .join("src/Marknexia.Rendering/Assets")
        .join(name);
    // Fail loudly: a missing oracle asset must not let a check pass silently.
    let source = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{name}: cannot read the .NET oracle asset {} ({error}); the bundled copy \
             cannot be verified without it",
            path.display()
        )
    });
    source.trim_start_matches('\u{feff}').replace("\r\n", "\n")
}

/// (.NET asset, .NET asset with the recorded patches) for each changed asset.
pub(crate) fn patched_assets() -> Vec<(String, String)> {
    let patches = decision_asset_patches();
    ASSETS
        .iter()
        .map(|name| {
            let source = dotnet_asset(name);
            let patched = apply_asset_patches(name, &source, &patches).unwrap();
            (source, patched)
        })
        .filter(|(source, patched)| source != patched)
        .collect()
}

/// The frozen page with each embedded .NET asset replaced by its patched
/// text, or `None` when the page embeds no changed asset.
pub(crate) fn patch_expected_page(
    case: &str,
    page: &str,
    assets: &[(String, String)],
) -> Option<String> {
    let mut page = page.to_owned();
    let mut changed = false;
    for (source, patched) in assets {
        match page.matches(source.as_str()).count() {
            0 => {}
            1 => {
                page = page.replacen(source.as_str(), patched, 1);
                changed = true;
            }
            count => panic!("{case}: a .NET asset is embedded {count} times"),
        }
    }
    changed.then_some(page)
}
