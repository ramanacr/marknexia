//! URI categories matched to the releasable .NET navigation oracle.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UriClassification {
    Empty,
    FragmentOnly,
    Unsupported,
    Http,
    Https,
    OtherExternal,
    RepositoryRootPath,
    AbsoluteLocalPath,
    RelativePath,
}

pub fn classify(destination: Option<&str>) -> UriClassification {
    let Some(destination) = destination.map(str::trim).filter(|value| !value.is_empty()) else {
        return UriClassification::Empty;
    };
    if destination.starts_with('#') {
        return UriClassification::FragmentOnly;
    }
    let lower = destination.to_ascii_lowercase();
    if ["javascript:", "vbscript:", "data:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        return UriClassification::Unsupported;
    }
    if lower.starts_with("http://") {
        return UriClassification::Http;
    }
    if lower.starts_with("https://") {
        return UriClassification::Https;
    }
    if ["mailto:", "ftp://", "tel:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        return UriClassification::OtherExternal;
    }
    if destination.starts_with(['/', '\\']) {
        return UriClassification::RepositoryRootPath;
    }
    let source = destination.as_bytes();
    if (source.len() >= 2 && source[0].is_ascii_alphabetic() && source[1] == b':')
        || lower.starts_with("file://")
    {
        return UriClassification::AbsoluteLocalPath;
    }
    UriClassification::RelativePath
}
