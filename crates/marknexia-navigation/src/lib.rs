#![forbid(unsafe_code)]

pub mod classify;
pub mod history;

pub use marknexia_files::virtual_fs::VirtualFileSystem;

use marknexia_files::path::{CanonicalPath, PathError, RepositoryScope};
use serde::Serialize;
use serde_json::Value;

const MAX_DESTINATION_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug)]
pub struct ResolutionContext {
    pub current_file: String,
    pub repository_root: Option<String>,
    pub allow_external_links: bool,
    pub enforce_repository_sandbox: bool,
}

impl ResolutionContext {
    pub fn from_case(input: &Value) -> Self {
        Self {
            current_file: input["currentFile"].as_str().unwrap_or_default().to_owned(),
            repository_root: input["repositoryRoot"].as_str().map(str::to_owned),
            allow_external_links: input["policy"]["allowExternalLinks"]
                .as_bool()
                .unwrap_or(false),
            enforce_repository_sandbox: input["policy"]["enforceRepositorySandbox"]
                .as_bool()
                .unwrap_or(true),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigationOutcome {
    pub kind: String,
    pub target_document: Option<String>,
    pub fragment: Option<String>,
    pub external_uri: Option<String>,
    pub is_safe: bool,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ResolutionResult {
    pub intent: NavigationOutcome,
    pub file_system_probe_count: u64,
}

fn blocked(fragment: Option<String>, message: &str) -> ResolutionResult {
    ResolutionResult {
        intent: NavigationOutcome {
            kind: "BlockedOrInvalid".into(),
            target_document: None,
            fragment,
            external_uri: None,
            is_safe: false,
            diagnostic: Some(message.into()),
        },
        file_system_probe_count: 0,
    }
}

fn decode_once(input: &str) -> Option<String> {
    let source = input.as_bytes();
    let mut bytes = Vec::with_capacity(source.len());
    let mut i = 0;
    while i < source.len() {
        if source[i] == b'%' && i + 2 < source.len() {
            let hi = (source[i + 1] as char).to_digit(16);
            let lo = (source[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                bytes.push(((hi << 4) | lo) as u8);
                i += 3;
                continue;
            }
        }
        bytes.push(source[i]);
        i += 1;
    }
    let result = String::from_utf8(bytes).ok()?;
    if result.chars().any(char::is_control) {
        None
    } else {
        Some(result)
    }
}

fn contains_encoded_traversal(input: &str) -> bool {
    input.contains('%')
        && decode_once(input).is_some_and(|decoded| {
            decoded
                .replace('\\', "/")
                .split('/')
                .any(|component| component == "..")
        })
}

fn invalid_local_path(fragment: Option<String>) -> ResolutionResult {
    blocked(
        fragment,
        "Access blocked: Network file paths and invalid local paths are not supported.",
    )
}

fn broken_root_path(fragment: Option<String>) -> ResolutionResult {
    ResolutionResult {
        intent: NavigationOutcome {
            kind: "BrokenTarget".into(),
            target_document: None,
            fragment,
            external_uri: None,
            is_safe: false,
            diagnostic: Some(
                "Cannot resolve repository-root path ('/...'): No active repository context."
                    .into(),
            ),
        },
        file_system_probe_count: 0,
    }
}

fn local_file_uri_path(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    if !lower.starts_with("file:") {
        return decode_once(path);
    }

    // `file:///C:/...` is a local Windows file URI. Anything with an authority
    // component is a network path and is rejected before a filesystem probe.
    let remainder = &path[5..];
    let local_path = remainder.strip_prefix("///")?;
    decode_once(local_path)
}

fn is_network_or_device_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    normalized.starts_with("//")
        || lower.starts_with("/??/")
        || lower.starts_with("/?/")
        || lower.starts_with("/device/")
}

fn has_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn has_alternate_data_stream(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized
        .split('/')
        .enumerate()
        .any(|(index, component)| index != 0 && component.contains(':'))
}

fn virtual_relative_target(
    path: &str,
    context: &ResolutionContext,
    rooted: bool,
) -> Result<String, &'static str> {
    let mut components: Vec<String> = if rooted {
        Vec::new()
    } else {
        let mut current: Vec<String> = context
            .current_file
            .replace('\\', "/")
            .split('/')
            .filter(|component| !component.is_empty())
            .map(str::to_owned)
            .collect();
        current.pop();
        current
    };

    for component in path
        .trim_start_matches(['/', '\\'])
        .replace('\\', "/")
        .split('/')
    {
        match component {
            "" | "." => {}
            ".." => {
                components
                    .pop()
                    .ok_or("Access blocked: Relative path escapes repository root sandbox.")?;
            }
            _ => components.push(component.into()),
        }
    }
    Ok(components.join("/"))
}

/// How the repository root of a resolution context can be enforced.
enum RootMode {
    /// No repository root was supplied.
    Absent,
    /// A canonical Windows root; containment is checked component-wise.
    Canonical(RepositoryScope),
    /// A lexically safe relative root naming a virtual (fixture) repository.
    /// Containment is enforced by `virtual_relative_target`, which refuses to
    /// pop above the virtual root.
    Virtual,
    /// A root that is neither canonical nor a safe virtual name. With sandbox
    /// enforcement on, resolution must fail closed.
    Invalid,
}

/// A relative, drive-less, non-rooted path whose every component is a plain
/// file name: no traversal, no stream/drive separators, no reserved names.
fn is_safe_virtual_relative_path(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with(['/', '\\'])
        || path.chars().any(char::is_control)
        || path.contains(':')
    {
        return false;
    }
    path.split(['/', '\\']).all(|component| {
        !component.is_empty()
            && component != "."
            && component != ".."
            && !component.contains(['<', '>', '"', '|', '?', '*'])
            && !component.ends_with([' ', '.'])
    })
}

fn root_mode(context: &ResolutionContext) -> RootMode {
    let Some(root) = context.repository_root.as_deref() else {
        return RootMode::Absent;
    };
    match RepositoryScope::new(root) {
        Ok(scope) => RootMode::Canonical(scope),
        Err(PathError::NotAbsolute) if is_safe_virtual_relative_path(root) => RootMode::Virtual,
        Err(_) => RootMode::Invalid,
    }
}

/// A lexically validated absolute Windows path in two forms.
///
/// `canonical` is case-folded and decides repository containment, so a case
/// change can never slip past the sandbox. `display` is the same path with the
/// caller's case kept, which is what .NET `Path.GetFullPath` returns (and so
/// what the oracle reports as the target). Both come from the same segment
/// walk, so they name the same path. `display` uses `/` separators.
#[derive(Clone, Debug)]
struct LocalPath {
    canonical: CanonicalPath,
    display: String,
}

impl LocalPath {
    fn new(input: &str) -> Result<Self, PathError> {
        // Validation (network/device, drive form, reserved names, traversal
        // above the drive) is owned by `CanonicalPath`.
        let canonical = CanonicalPath::new(input)?;
        // `CanonicalPath` succeeded, so bytes 0..3 are an ASCII drive letter,
        // ':' and a separator.
        let normalized = input.replace('\\', "/");
        let mut segments: Vec<&str> = Vec::new();
        for segment in normalized[3..].split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                _ => segments.push(segment),
            }
        }
        let drive = &normalized[..1];
        let display = if segments.is_empty() {
            format!("{drive}:/")
        } else {
            format!("{drive}:/{}", segments.join("/"))
        };
        Ok(Self { canonical, display })
    }

    fn join(&self, path: &str) -> Result<Self, PathError> {
        Self::new(&format!(
            "{}/{}",
            self.display.trim_end_matches('/'),
            path.trim_start_matches(['/', '\\'])
        ))
    }

    /// The containing directory. A drive root has none (as .NET
    /// `Path.GetDirectoryName` returns null), so it is an error here.
    fn parent(&self) -> Result<Self, PathError> {
        match self.display.rsplit_once('/') {
            Some((drive, "")) if drive.len() == 2 => Err(PathError::NotAbsolute),
            Some((drive, _)) if drive.len() == 2 => Self::new(&format!("{drive}/")),
            Some((parent, _)) => Self::new(parent),
            None => Err(PathError::NotAbsolute),
        }
    }

    /// The path as .NET `Path.GetFullPath` spells it, for diagnostics.
    fn windows(&self) -> String {
        self.display.replace('/', "\\")
    }
}

/// .NET `NavigationResolver` diagnostic prefixes for a missing target.
const RELATIVE_NOT_FOUND: &str = "Target file not found: ";
const REPOSITORY_NOT_FOUND: &str = "Repository-relative file not found: ";
const ABSOLUTE_NOT_FOUND: &str = "Absolute target file not found: ";
const RELATIVE_ESCAPE: &str = "Access blocked: Relative path escapes repository root sandbox.";

fn complete_local_target(
    target: LocalPath,
    fragment: Option<String>,
    scope: Option<&RepositoryScope>,
    context: &ResolutionContext,
    vfs: &VirtualFileSystem,
    outside_message: &'static str,
    not_found_prefix: &'static str,
) -> ResolutionResult {
    if context.enforce_repository_sandbox
        && scope.is_some_and(|scope| !scope.contains(&target.canonical))
    {
        return blocked(fragment, outside_message);
    }

    let mut probes = 0;
    let exists = vfs.file_exists(&target.display, &mut probes);
    let target_document = format!(
        "{}{}",
        target.display,
        fragment
            .as_ref()
            .map_or(String::new(), |value| format!("#{value}"))
    );
    ResolutionResult {
        intent: NavigationOutcome {
            kind: if exists {
                if fragment.is_some() {
                    "CrossDocumentWithAnchor"
                } else {
                    "CrossDocument"
                }
            } else {
                "BrokenTarget"
            }
            .into(),
            target_document: Some(target_document),
            fragment,
            external_uri: None,
            is_safe: true,
            diagnostic: (!exists).then(|| format!("{not_found_prefix}{}", target.windows())),
        },
        file_system_probe_count: probes,
    }
}

/// Resolve a relative destination against an absolute current file, in the
/// .NET order: probe the current file; if it exists the base is its directory,
/// otherwise the current path itself is the base.
///
/// Decision NAV-1 (`compat/decisions/navigation-probe-order.md`): with the
/// sandbox enforced, the directory-based target is checked for containment
/// before the current-file probe, so an escaping destination is rejected with
/// zero probes. .NET probes the current file first.
fn resolve_relative_to_current(
    current: &LocalPath,
    decoded: &str,
    fragment: Option<String>,
    scope: &RepositoryScope,
    context: &ResolutionContext,
    vfs: &VirtualFileSystem,
) -> ResolutionResult {
    let from_directory = current.parent().and_then(|parent| parent.join(decoded));
    if context.enforce_repository_sandbox
        && !from_directory
            .as_ref()
            .is_ok_and(|target| scope.contains(&target.canonical))
    {
        return blocked(fragment, RELATIVE_ESCAPE);
    }

    let mut probes = 0;
    let target = if vfs.file_exists(&current.display, &mut probes) {
        from_directory
    } else {
        current.join(decoded)
    };
    let mut result = match target {
        Ok(target) => complete_local_target(
            target,
            fragment,
            Some(scope),
            context,
            vfs,
            RELATIVE_ESCAPE,
            RELATIVE_NOT_FOUND,
        ),
        Err(_) => blocked(fragment, RELATIVE_ESCAPE),
    };
    // Only the trusted current file can have been probed before a rejection.
    result.file_system_probe_count += probes;
    result
}

fn complete_virtual_target(
    target: String,
    fragment: Option<String>,
    vfs: &VirtualFileSystem,
) -> ResolutionResult {
    let mut probes = 0;
    let exists = vfs.file_exists(&target, &mut probes);
    let target_document = format!(
        "{target}{}",
        fragment
            .as_ref()
            .map_or(String::new(), |value| format!("#{value}"))
    );
    ResolutionResult {
        intent: NavigationOutcome {
            kind: if exists {
                if fragment.is_some() {
                    "CrossDocumentWithAnchor"
                } else {
                    "CrossDocument"
                }
            } else {
                "BrokenTarget"
            }
            .into(),
            target_document: Some(target_document),
            fragment,
            external_uri: None,
            is_safe: true,
            diagnostic: (!exists).then(|| format!("Target file not found: {target}")),
        },
        file_system_probe_count: probes,
    }
}

fn normalize_external_uri(destination: &str) -> Option<String> {
    if destination
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return None;
    }
    let lower = destination.to_ascii_lowercase();
    if let Some((scheme, remainder)) = destination.split_once("://") {
        let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() || authority.contains('\\') {
            return None;
        }
        if ["http", "https", "ftp"].contains(&scheme.to_ascii_lowercase().as_str())
            && remainder.starts_with(['?', '#'])
        {
            return Some(format!(
                "{scheme}://{authority}/{}",
                &remainder[authority.len()..]
            ));
        }
        if ["http", "https", "ftp"].contains(&scheme.to_ascii_lowercase().as_str())
            && !remainder[authority.len()..].starts_with('/')
        {
            return Some(format!(
                "{scheme}://{authority}/{}",
                &remainder[authority.len()..]
            ));
        }
        return Some(destination.into());
    }
    if ["mailto:", "tel:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        && destination
            .split_once(':')
            .is_some_and(|(_, value)| !value.is_empty())
    {
        return Some(destination.into());
    }
    None
}

pub fn resolve(
    destination: Option<&str>,
    context: &ResolutionContext,
    vfs: &VirtualFileSystem,
) -> ResolutionResult {
    let destination = destination.unwrap_or_default().trim();
    if destination.is_empty() {
        return blocked(None, "Empty destination.");
    }
    if destination.len() > MAX_DESTINATION_BYTES {
        return blocked(None, "Destination exceeds configured size limit.");
    }
    let lower = destination.to_ascii_lowercase();
    if ["javascript:", "vbscript:", "data:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return blocked(None, "Unsupported or unsafe protocol.");
    }
    if ["http://", "https://", "mailto:", "ftp://", "tel:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        if !context.allow_external_links {
            return blocked(None, "External links are disabled by policy.");
        }
        let Some(uri) = normalize_external_uri(destination) else {
            return blocked(None, "Invalid external URI.");
        };
        return ResolutionResult {
            intent: NavigationOutcome {
                kind: "ExternalBrowser".into(),
                target_document: None,
                fragment: None,
                external_uri: Some(uri),
                is_safe: true,
                diagnostic: None,
            },
            file_system_probe_count: 0,
        };
    }
    let (path, encoded_fragment) = destination.split_once('#').unwrap_or((destination, ""));
    let fragment = if encoded_fragment.is_empty() {
        None
    } else {
        decode_once(encoded_fragment)
    };
    if contains_encoded_traversal(path) {
        return blocked(
            fragment,
            "Access blocked: Relative path escapes repository root sandbox.",
        );
    }
    let Some(decoded) = local_file_uri_path(path) else {
        return invalid_local_path(fragment);
    };
    if is_network_or_device_path(&decoded) || has_alternate_data_stream(&decoded) {
        return invalid_local_path(fragment);
    }
    if path.is_empty() {
        return ResolutionResult {
            intent: NavigationOutcome {
                kind: "SameDocumentAnchor".into(),
                target_document: Some(format!(
                    "{}#{}",
                    context.current_file,
                    fragment.clone().unwrap_or_default()
                )),
                fragment,
                external_uri: None,
                is_safe: true,
                diagnostic: None,
            },
            file_system_probe_count: 0,
        };
    }
    let rooted = decoded.starts_with('/') || decoded.starts_with('\\');
    let absolute_input = lower.starts_with("file:") || has_drive_prefix(&decoded);
    let absolute_destination = LocalPath::new(&decoded).ok();
    let mode = root_mode(context);

    if context.enforce_repository_sandbox {
        match mode {
            RootMode::Invalid => {
                return blocked(
                    fragment,
                    "Access blocked: Repository root is invalid; cannot enforce repository sandbox.",
                );
            }
            RootMode::Virtual => {
                // A virtual root only sandboxes virtual relative paths. An
                // unparseable current file or any absolute target cannot be
                // proven contained, so fail closed before any probe.
                if !is_safe_virtual_relative_path(&context.current_file) {
                    return blocked(
                        fragment,
                        "Access blocked: Current file is invalid; cannot enforce repository sandbox.",
                    );
                }
                if absolute_destination.is_some() || absolute_input {
                    return blocked(
                        fragment,
                        "Access blocked: Absolute path is outside repository sandbox.",
                    );
                }
            }
            RootMode::Absent | RootMode::Canonical(_) => {}
        }
    }
    let scope = match mode {
        RootMode::Canonical(scope) => Some(scope),
        _ => None,
    };

    if let Some(target) = absolute_destination {
        return complete_local_target(
            target,
            fragment,
            scope.as_ref(),
            context,
            vfs,
            "Access blocked: Absolute path is outside repository sandbox.",
            ABSOLUTE_NOT_FOUND,
        );
    }
    if absolute_input {
        return invalid_local_path(fragment);
    }

    if rooted && context.repository_root.is_none() {
        return broken_root_path(fragment);
    }

    if let Some(scope) = scope.as_ref() {
        if rooted {
            const ROOT_TRAVERSAL: &str =
                "Access blocked: Repository root traversal outside sandbox boundary.";
            // A canonical scope implies the root parses; fail closed regardless.
            let Ok(root) = LocalPath::new(context.repository_root.as_deref().unwrap_or_default())
            else {
                return blocked(
                    fragment,
                    "Access blocked: Repository root is invalid; cannot enforce repository sandbox.",
                );
            };
            let Ok(target) = root.join(&decoded) else {
                return blocked(fragment, ROOT_TRAVERSAL);
            };
            return complete_local_target(
                target,
                fragment,
                Some(scope),
                context,
                vfs,
                ROOT_TRAVERSAL,
                REPOSITORY_NOT_FOUND,
            );
        } else {
            let current = LocalPath::new(&context.current_file)
                .ok()
                .filter(|current| current.parent().is_ok());
            match current {
                Some(current) => {
                    return resolve_relative_to_current(
                        &current, &decoded, fragment, scope, context, vfs,
                    );
                }
                None if context.enforce_repository_sandbox => {
                    return blocked(
                        fragment,
                        "Access blocked: Current file is invalid; cannot enforce repository sandbox.",
                    );
                }
                None => {}
            }
        }
    }

    let Ok(target) = virtual_relative_target(&decoded, context, rooted) else {
        return blocked(
            fragment,
            "Access blocked: Relative path escapes repository root sandbox.",
        );
    };
    if context.enforce_repository_sandbox && context.repository_root.is_some() && target.is_empty()
    {
        return blocked(
            fragment,
            "Access blocked: Relative path escapes repository root sandbox.",
        );
    }
    complete_virtual_target(target, fragment, vfs)
}
