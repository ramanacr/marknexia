#![forbid(unsafe_code)]

pub mod classify;
pub mod history;

pub use marknexia_files::virtual_fs::VirtualFileSystem;

use marknexia_files::path::{CanonicalPath, RepositoryScope};
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

fn canonical_scope(context: &ResolutionContext) -> Option<RepositoryScope> {
    context
        .repository_root
        .as_deref()
        .and_then(|root| RepositoryScope::new(root).ok())
}

fn join_canonical(base: &CanonicalPath, path: &str) -> Result<CanonicalPath, ()> {
    CanonicalPath::new(&format!(
        "{}/{}",
        base.as_str().trim_end_matches('/'),
        path.trim_start_matches(['/', '\\'])
    ))
    .map_err(|_| ())
}

fn parent_canonical(path: &CanonicalPath) -> Result<CanonicalPath, ()> {
    let value = path.as_str();
    let parent = value.rsplit_once('/').map_or(value, |(parent, _)| parent);
    CanonicalPath::new(parent).map_err(|_| ())
}

fn complete_local_target(
    target: CanonicalPath,
    fragment: Option<String>,
    scope: Option<&RepositoryScope>,
    context: &ResolutionContext,
    vfs: &VirtualFileSystem,
    outside_message: &'static str,
) -> ResolutionResult {
    if context.enforce_repository_sandbox && scope.is_some_and(|scope| !scope.contains(&target)) {
        return blocked(fragment, outside_message);
    }

    let mut probes = 0;
    let exists = vfs.file_exists(target.as_str(), &mut probes);
    let target_document = format!(
        "{}{}",
        target.as_str(),
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
            diagnostic: (!exists).then(|| format!("Target file not found: {}", target.as_str())),
        },
        file_system_probe_count: probes,
    }
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
    let absolute_destination = CanonicalPath::new(&decoded).ok();
    let scope = canonical_scope(context);

    if context.enforce_repository_sandbox && context.repository_root.is_some() && scope.is_none() {
        return blocked(
            fragment,
            "Access blocked: Repository root is invalid; cannot enforce repository sandbox.",
        );
    }

    if let Some(target) = absolute_destination {
        return complete_local_target(
            target,
            fragment,
            scope.as_ref(),
            context,
            vfs,
            "Access blocked: Absolute path is outside repository sandbox.",
        );
    }
    if absolute_input {
        return invalid_local_path(fragment);
    }

    if rooted && context.repository_root.is_none() {
        return broken_root_path(fragment);
    }

    if let Some(scope) = scope.as_ref() {
        let base = if rooted {
            CanonicalPath::new(context.repository_root.as_deref().unwrap_or_default()).ok()
        } else {
            CanonicalPath::new(&context.current_file)
                .ok()
                .and_then(|current| parent_canonical(&current).ok())
        };
        if base.is_none() && context.enforce_repository_sandbox {
            return blocked(
                fragment,
                "Access blocked: Current file is invalid; cannot enforce repository sandbox.",
            );
        }
        if let Some(base) = base {
            let Ok(target) = join_canonical(&base, &decoded) else {
                return blocked(
                    fragment,
                    "Access blocked: Relative path escapes repository root sandbox.",
                );
            };
            return complete_local_target(
                target,
                fragment,
                Some(scope),
                context,
                vfs,
                "Access blocked: Relative path escapes repository root sandbox.",
            );
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
