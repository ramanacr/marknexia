use marknexia_core::contracts::{AppTheme, RenderedHtml};
use marknexia_webview::protocol::{HostToPage, serialize_host_message};
use marknexia_webview::protocol::{MessageError, PageToHost, ProtocolContext, parse_page_message};

const SOURCE: &str = "https://tab-7.marknexia.invalid/document.html";
const ORIGIN: &str = "https://tab-7.marknexia.invalid";

fn context() -> ProtocolContext<'static> {
    ProtocolContext {
        expected_source_uri: SOURCE,
        expected_origin: ORIGIN,
        protocol: 1,
        tab_id: 7,
        document_epoch: 42,
    }
}

#[test]
fn accepts_ready_only_for_current_document() {
    let json = br#"{"type":"ready","payload":{"protocol":1,"tabId":7,"documentEpoch":42}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Ok(PageToHost::Ready {
            protocol: 1,
            tab_id: 7,
            document_epoch: 42,
        })
    );
}

#[test]
fn rejects_messages_from_a_different_origin() {
    let json = br#"{"type":"ready","payload":{"protocol":1,"tabId":7,"documentEpoch":42}}"#;
    assert_eq!(
        parse_page_message(json, "https://attacker.invalid/document.html", &context()),
        Err(MessageError::WrongOrigin)
    );
}

#[test]
fn rejects_same_origin_but_unexpected_document_source() {
    let json = br#"{"type":"ready","payload":{"protocol":1,"tabId":7,"documentEpoch":42}}"#;
    assert_eq!(
        parse_page_message(
            json,
            "https://tab-7.marknexia.invalid/other.html",
            &context()
        ),
        Err(MessageError::WrongSource)
    );
}

#[test]
fn rejects_a_message_claiming_another_tab() {
    let json = br#"{"type":"ready","payload":{"protocol":1,"tabId":8,"documentEpoch":42}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Err(MessageError::CrossTab)
    );
}

#[test]
fn rejects_old_document_epoch() {
    let json = br##"{"type":"openLink","payload":{"protocol":1,"tabId":7,"documentEpoch":41,"href":"#heading"}}"##;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Err(MessageError::StaleEpoch)
    );
}

#[test]
fn rejects_wrong_protocol() {
    let json = br#"{"type":"copyText","payload":{"protocol":2,"tabId":7,"documentEpoch":42,"text":"hello"}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Err(MessageError::WrongProtocol)
    );
}

#[test]
fn rejects_unknown_payload_field() {
    let json = br#"{"type":"ready","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"command":"erase"}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Err(MessageError::InvalidPayload)
    );
}

#[test]
fn rejects_unknown_message_type() {
    let json = br#"{"type":"runCommand","payload":{"protocol":1,"tabId":7,"documentEpoch":42}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Err(MessageError::InvalidPayload)
    );
}

#[test]
fn rejects_unexpected_top_level_json_field() {
    let json = br#"{"type":"ready","payload":{"protocol":1,"tabId":7,"documentEpoch":42},"source":"https://tab-7.marknexia.invalid/document.html"}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Err(MessageError::InvalidPayload)
    );
}

#[test]
fn rejects_origin_prefix_spoofing() {
    let json = br#"{"type":"ready","payload":{"protocol":1,"tabId":7,"documentEpoch":42}}"#;
    assert_eq!(
        parse_page_message(
            json,
            "https://tab-7.marknexia.invalid.evil/document.html",
            &context()
        ),
        Err(MessageError::WrongOrigin)
    );
}

#[test]
fn accepts_typed_focus_change_for_current_document() {
    let json = br#"{"type":"focusChanged","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"focused":true}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Ok(PageToHost::FocusChanged {
            protocol: 1,
            tab_id: 7,
            document_epoch: 42,
            focused: true,
        })
    );
}

#[test]
fn rejects_messages_over_64_kib_before_json_parsing() {
    let oversized = vec![b'x'; 64 * 1024 + 1];
    assert_eq!(
        parse_page_message(&oversized, SOURCE, &context()),
        Err(MessageError::TooLarge)
    );
}

#[test]
fn accepts_typed_open_link_message() {
    let json = br##"{"type":"openLink","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"href":"#heading"}}"##;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Ok(PageToHost::OpenLink {
            protocol: 1,
            tab_id: 7,
            document_epoch: 42,
            href: "#heading".to_owned(),
        })
    );
}

#[test]
fn rejects_link_destination_beyond_core_url_contract() {
    let href = "x".repeat(8 * 1024 + 1);
    let json = format!(
        "{{\"type\":\"openLink\",\"payload\":{{\"protocol\":1,\"tabId\":7,\"documentEpoch\":42,\"href\":\"{href}\"}}}}"
    );
    assert!(matches!(
        parse_page_message(json.as_bytes(), SOURCE, &context()),
        Err(MessageError::InvalidPayload)
    ));
}

#[test]
fn serializes_render_content_as_json_data_not_interpolated_script() {
    let html = RenderedHtml::try_new("<p title=\"x\">a</p>").unwrap();
    let message = HostToPage::RenderDocument {
        protocol: 1,
        tab_id: 7,
        document_epoch: 42,
        html,
    };
    assert_eq!(message.identity(), (1, 7, 42));
    assert_eq!(
        serialize_host_message(&message),
        Ok("{\"type\":\"renderDocument\",\"payload\":{\"protocol\":1,\"tabId\":7,\"documentEpoch\":42,\"html\":\"<p title=\\\"x\\\">a</p>\"}}".to_owned())
    );
}

#[test]
fn serializes_a_typed_theme_message_for_the_current_document() {
    let message = HostToPage::SetTheme {
        protocol: 1,
        tab_id: 7,
        document_epoch: 42,
        theme: AppTheme::Dark,
    };
    assert_eq!(
        serialize_host_message(&message),
        Ok("{\"type\":\"setTheme\",\"payload\":{\"protocol\":1,\"tabId\":7,\"documentEpoch\":42,\"theme\":\"Dark\"}}".to_owned())
    );
}

#[test]
fn rejects_host_json_that_expands_past_output_cap() {
    let html = RenderedHtml::try_new("\"".repeat(5 * 1024 * 1024)).unwrap();
    let message = HostToPage::RenderDocument {
        protocol: 1,
        tab_id: 7,
        document_epoch: 42,
        html,
    };
    assert!(matches!(
        serialize_host_message(&message),
        Err(MessageError::TooLarge)
    ));
}
