#![forbid(unsafe_code)]

pub mod classify;
pub mod history;

pub use marknexia_files::virtual_fs::VirtualFileSystem;

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
        if ["http://", "https://", "ftp://"]
            .iter()
            .any(|scheme| lower.starts_with(scheme))
            && !destination.split_once("://").is_some_and(|(_, rest)| {
                !rest
                    .split(['/', '?', '#'])
                    .next()
                    .unwrap_or_default()
                    .is_empty()
            })
        {
            return blocked(None, "Invalid external URI.");
        }
        let uri = if (lower.starts_with("http://") || lower.starts_with("https://"))
            && !destination
                .split_once("://")
                .is_some_and(|(_, tail)| tail.contains('/'))
        {
            format!("{destination}/")
        } else {
            destination.into()
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
    if lower.starts_with("file:") || destination.starts_with("\\\\") {
        return blocked(
            fragment,
            "Access blocked: Network file paths and invalid local paths are not supported.",
        );
    }
    let Some(decoded) = decode_once(path) else {
        return blocked(
            fragment,
            "Access blocked: Network file paths and invalid local paths are not supported.",
        );
    };
    if decoded.replace('\\', "/").starts_with("//") || decoded.contains(':') {
        return blocked(
            fragment,
            "Access blocked: Network file paths and invalid local paths are not supported.",
        );
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
    let mut parts: Vec<String> = if rooted {
        Vec::new()
    } else {
        let mut current: Vec<String> = context
            .current_file
            .replace('\\', "/")
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();
        current.pop();
        current
    };
    for segment in decoded
        .trim_start_matches(['/', '\\'])
        .replace('\\', "/")
        .split('/')
    {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return blocked(
                        fragment,
                        "Access blocked: Relative path escapes repository root sandbox.",
                    );
                }
            }
            _ => parts.push(segment.into()),
        }
    }
    let relative = parts.join("/");
    if context.enforce_repository_sandbox
        && context.repository_root.is_some()
        && relative.is_empty()
    {
        return blocked(
            fragment,
            "Access blocked: Relative path escapes repository root sandbox.",
        );
    }
    let mut probes = 0;
    let exists = vfs.file_exists(&relative, &mut probes);
    let target = format!(
        "{relative}{}",
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
            target_document: Some(target),
            fragment,
            external_uri: None,
            is_safe: true,
            diagnostic: if exists {
                None
            } else {
                Some(format!("Target file not found: {relative}"))
            },
        },
        file_system_probe_count: probes,
    }
}
