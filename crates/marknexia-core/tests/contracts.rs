use marknexia_core::contracts::{
    AppTheme, BoundedFragment, BoundedUrl, Diagnostic, DiagnosticSeverity, DocumentId,
    DocumentText, GenerationId, Heading, NavigationIntent, RenderRequest, RenderResult,
    RenderedHtml,
};

#[test]
fn required_core_contracts_are_constructible() {
    let document_id = DocumentId::try_new("docs/readme.md").expect("valid document id");
    let request = RenderRequest {
        document_id: document_id.clone(),
        generation: GenerationId(7),
        source: DocumentText::try_new("# Hello").expect("bounded markdown"),
        theme: AppTheme::Dark,
    };
    let result = RenderResult {
        generation: request.generation,
        html: RenderedHtml::try_new("<h1>Hello</h1>").expect("bounded HTML"),
        headings: vec![Heading {
            text: "Hello".into(),
            level: 1,
            slug: "hello".into(),
            source_line: 1,
        }],
        diagnostics: vec![Diagnostic::warning("demo")],
    };
    let navigation = NavigationIntent::OpenDocument {
        document_id,
        fragment: Some(BoundedFragment::try_new("hello").expect("bounded fragment")),
    };

    assert_eq!(result.generation, GenerationId(7));
    assert_eq!(result.headings[0].slug, "hello");
    assert_eq!(result.diagnostics[0].severity, DiagnosticSeverity::Warning);
    assert!(matches!(navigation, NavigationIntent::OpenDocument { .. }));
}

#[test]
fn bounded_contract_values_reject_oversized_input() {
    assert!(DocumentId::try_new("a".repeat(DocumentId::MAX_BYTES + 1)).is_err());
    assert!(DocumentText::try_new("a".repeat(DocumentText::MAX_BYTES + 1)).is_err());
    assert!(RenderedHtml::try_new("a".repeat(RenderedHtml::MAX_BYTES + 1)).is_err());
}

#[test]
fn bounded_contract_values_deserialize_only_within_byte_limits() {
    let document_id = "a".repeat(DocumentId::MAX_BYTES);
    let document_text = "a".repeat(DocumentText::MAX_BYTES);
    let rendered_html = "a".repeat(RenderedHtml::MAX_BYTES);
    let fragment = "a".repeat(BoundedFragment::MAX_BYTES);
    let url = "a".repeat(BoundedUrl::MAX_BYTES);

    assert_eq!(
        serde_json::from_str::<DocumentId>(&serde_json::to_string(&document_id).unwrap())
            .unwrap()
            .as_str(),
        document_id
    );
    assert_eq!(
        serde_json::from_str::<DocumentText>(&serde_json::to_string(&document_text).unwrap())
            .unwrap()
            .as_str(),
        document_text
    );
    assert_eq!(
        serde_json::from_str::<RenderedHtml>(&serde_json::to_string(&rendered_html).unwrap())
            .unwrap()
            .as_str(),
        rendered_html
    );
    assert_eq!(
        serde_json::from_str::<BoundedFragment>(&serde_json::to_string(&fragment).unwrap())
            .unwrap()
            .as_str(),
        fragment
    );
    assert_eq!(
        serde_json::from_str::<BoundedUrl>(&serde_json::to_string(&url).unwrap())
            .unwrap()
            .as_str(),
        url
    );

    assert!(
        serde_json::from_str::<DocumentId>(
            &serde_json::to_string(&"a".repeat(DocumentId::MAX_BYTES + 1)).unwrap()
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<DocumentText>(
            &serde_json::to_string(&"a".repeat(DocumentText::MAX_BYTES + 1)).unwrap()
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<RenderedHtml>(
            &serde_json::to_string(&"a".repeat(RenderedHtml::MAX_BYTES + 1)).unwrap()
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<BoundedFragment>(
            &serde_json::to_string(&"a".repeat(BoundedFragment::MAX_BYTES + 1)).unwrap()
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<BoundedUrl>(
            &serde_json::to_string(&"a".repeat(BoundedUrl::MAX_BYTES + 1)).unwrap()
        )
        .is_err()
    );
}

#[test]
fn bounded_contract_values_measure_utf8_bytes_not_characters() {
    let accepted = "é".repeat(DocumentId::MAX_BYTES / 2);
    let rejected = "é".repeat(DocumentId::MAX_BYTES / 2 + 1);

    assert!(serde_json::from_str::<DocumentId>(&serde_json::to_string(&accepted).unwrap()).is_ok());
    assert!(
        serde_json::from_str::<DocumentId>(&serde_json::to_string(&rejected).unwrap()).is_err()
    );
}
