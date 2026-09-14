use marknexia_core::contracts::{
    AppTheme, BoundedFragment, Diagnostic, DiagnosticSeverity, DocumentId, DocumentText,
    GenerationId, Heading, NavigationIntent, RenderRequest, RenderResult, RenderedHtml,
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
