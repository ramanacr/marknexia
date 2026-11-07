use marknexia_core::contracts::AppTheme;
use marknexia_security::{ContentPolicy, HtmlPolicy};
use marknexia_webview::protocol::{HostToPage, serialize_host_message};
use marknexia_webview::protocol::{
    MAX_HOST_MESSAGE_BYTES, MessageError, PageToHost, ProtocolContext, parse_page_message,
};

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
    let html = HtmlPolicy::new(ContentPolicy::default())
        .encode_text("<p title=\"x\">a</p>")
        .unwrap();
    let message = HostToPage::RenderDocument {
        protocol: 1,
        tab_id: 7,
        document_epoch: 42,
        html,
    };
    assert_eq!(message.identity(), (1, 7, 42));
    assert_eq!(
        serialize_host_message(&message),
        Ok("{\"type\":\"renderDocument\",\"payload\":{\"protocol\":1,\"tabId\":7,\"documentEpoch\":42,\"html\":\"&lt;p title=&quot;x&quot;&gt;a&lt;/p&gt;\"}}".to_owned())
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
fn rejects_safely_created_content_when_json_escaping_crosses_host_cap() {
    // A control character is inert text after HtmlPolicy encoding but expands
    // to six JSON bytes. This exercises the real capped serializer without a
    // public escape hatch for constructing SanitizedFragment.
    let characters = MAX_HOST_MESSAGE_BYTES / 6 + 1;
    let html = HtmlPolicy::new(ContentPolicy::default())
        .encode_text(&"\u{0001}".repeat(characters))
        .expect("input remains under the absolute HTML input/output caps");
    let message = HostToPage::RenderDocument {
        protocol: 1,
        tab_id: 7,
        document_epoch: 42,
        html,
    };
    assert_eq!(
        serialize_host_message(&message),
        Err(MessageError::TooLarge)
    );
}

fn find_result(payload: &str) -> String {
    format!(
        "{{\"type\":\"findResult\",\"payload\":{{\"protocol\":1,\"tabId\":7,\"documentEpoch\":42,{payload}}}}}"
    )
}

#[test]
fn accepts_a_consistent_find_result_for_the_current_document() {
    let json = find_result(r#""requestId":3,"query":"Mark","matchCount":4,"currentMatch":2"#);
    assert_eq!(
        parse_page_message(json.as_bytes(), SOURCE, &context()),
        Ok(PageToHost::FindResult {
            protocol: 1,
            document_epoch: 42,
            tab_id: 7,
            request_id: 3,
            query: "Mark".to_owned(),
            match_count: 4,
            current_match: 2,
        })
    );
    let none = find_result(r#""requestId":3,"query":"Mark","matchCount":0,"currentMatch":0"#);
    assert!(parse_page_message(none.as_bytes(), SOURCE, &context()).is_ok());
    let empty = find_result(r#""requestId":3,"query":"","matchCount":0,"currentMatch":0"#);
    assert!(parse_page_message(empty.as_bytes(), SOURCE, &context()).is_ok());
}

#[test]
fn rejects_inconsistent_or_unbounded_find_results() {
    for payload in [
        // Position past the count, no position with matches, matches for an
        // empty query, request id 0.
        r#""requestId":3,"query":"a","matchCount":2,"currentMatch":3"#,
        r#""requestId":3,"query":"a","matchCount":2,"currentMatch":0"#,
        r#""requestId":3,"query":"","matchCount":1,"currentMatch":1"#,
        r#""requestId":0,"query":"a","matchCount":0,"currentMatch":0"#,
        // Negative, fractional and oversized numbers.
        r#""requestId":3,"query":"a","matchCount":-1,"currentMatch":0"#,
        r#""requestId":3,"query":"a","matchCount":1.5,"currentMatch":1"#,
        r#""requestId":3,"query":"a","matchCount":1000001,"currentMatch":1"#,
        // Missing and unknown fields, wrong types.
        r#""requestId":3,"query":"a","matchCount":1"#,
        r#""requestId":3,"query":"a","matchCount":1,"currentMatch":1,"extra":true"#,
        r#""requestId":"3","query":"a","matchCount":1,"currentMatch":1"#,
        r#""requestId":3,"query":null,"matchCount":0,"currentMatch":0"#,
    ] {
        assert_eq!(
            parse_page_message(find_result(payload).as_bytes(), SOURCE, &context()),
            Err(MessageError::InvalidPayload),
            "{payload}"
        );
    }
    let long = "q".repeat(1025);
    let json = find_result(&format!(
        r#""requestId":3,"query":"{long}","matchCount":0,"currentMatch":0"#
    ));
    assert_eq!(
        parse_page_message(json.as_bytes(), SOURCE, &context()),
        Err(MessageError::InvalidPayload)
    );
}

#[test]
fn find_results_are_bound_to_the_document_identity() {
    let json = br#"{"type":"findResult","payload":{"protocol":1,"tabId":8,"documentEpoch":42,"requestId":3,"query":"a","matchCount":1,"currentMatch":1}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Err(MessageError::CrossTab)
    );
    let json = br#"{"type":"findUnavailable","payload":{"protocol":1,"tabId":7,"documentEpoch":41,"requestId":3}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Err(MessageError::StaleEpoch)
    );
    let json = br#"{"type":"findUnavailable","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"requestId":0}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Err(MessageError::InvalidPayload)
    );
    let json = br#"{"type":"findUnavailable","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"requestId":5}}"#;
    assert_eq!(
        parse_page_message(json, SOURCE, &context()),
        Ok(PageToHost::FindUnavailable {
            protocol: 1,
            document_epoch: 42,
            tab_id: 7,
            request_id: 5,
        })
    );
}

#[test]
fn serializes_find_requests_as_json_data() {
    let message = HostToPage::Find {
        protocol: 1,
        tab_id: 7,
        document_epoch: 42,
        request_id: 9,
        query: "a\"</script>".to_owned(),
        backwards: true,
    };
    assert_eq!(message.identity(), (1, 7, 42));
    assert_eq!(
        serialize_host_message(&message),
        Ok("{\"type\":\"find\",\"payload\":{\"protocol\":1,\"tabId\":7,\"documentEpoch\":42,\"requestId\":9,\"query\":\"a\\\"</script>\",\"backwards\":true}}".to_owned())
    );
    let clear = HostToPage::ClearFind {
        protocol: 1,
        tab_id: 7,
        document_epoch: 42,
    };
    assert_eq!(
        serialize_host_message(&clear),
        Ok("{\"type\":\"clearFind\",\"payload\":{\"protocol\":1,\"tabId\":7,\"documentEpoch\":42}}".to_owned())
    );
}

#[test]
fn refuses_to_serialize_oversized_or_unnumbered_find_requests() {
    let find = |request_id: u64, query: String| HostToPage::Find {
        protocol: 1,
        tab_id: 7,
        document_epoch: 42,
        request_id,
        query,
        backwards: false,
    };
    // 1,024 UTF-16 units fit; a supplementary character counts as two.
    assert!(serialize_host_message(&find(1, "q".repeat(1024))).is_ok());
    assert_eq!(
        serialize_host_message(&find(1, "q".repeat(1025))),
        Err(MessageError::TooLarge)
    );
    assert_eq!(
        serialize_host_message(&find(1, "\u{1F600}".repeat(513))),
        Err(MessageError::TooLarge)
    );
    assert_eq!(
        serialize_host_message(&find(0, "q".to_owned())),
        Err(MessageError::InvalidPayload)
    );
}

// The link adapter's messages (`link_adapter.js`).

#[test]
fn accepts_the_link_adapters_typed_messages() {
    let open = br##"{"type":"openLink","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"href":"00-overview/product-overview.md#goals"}}"##;
    assert_eq!(
        parse_page_message(open, SOURCE, &context()),
        Ok(PageToHost::OpenLink {
            protocol: 1,
            tab_id: 7,
            document_epoch: 42,
            href: "00-overview/product-overview.md#goals".to_owned(),
        })
    );
    let hover = br##"{"type":"linkHover","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"href":"#intro"}}"##;
    assert!(matches!(
        parse_page_message(hover, SOURCE, &context()),
        Ok(PageToHost::LinkHover { href: Some(href), .. }) if href == "#intro"
    ));
    let leave = br#"{"type":"linkHover","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"href":null}}"#;
    assert!(matches!(
        parse_page_message(leave, SOURCE, &context()),
        Ok(PageToHost::LinkHover { href: None, .. })
    ));
    let scroll = br#"{"type":"scrollChanged","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"top":1280}}"#;
    assert!(matches!(
        parse_page_message(scroll, SOURCE, &context()),
        Ok(PageToHost::ScrollChanged { top: 1280, .. })
    ));
    let step = br#"{"type":"historyStep","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"back":true}}"#;
    assert!(matches!(
        parse_page_message(step, SOURCE, &context()),
        Ok(PageToHost::HistoryStep { back: true, .. })
    ));
}

#[test]
fn rejects_malformed_or_foreign_link_messages() {
    for (json, error) in [
        // The frozen .NET bridge's flat message.
        (
            br#"{"type":"openLink","href":"a.md"}"#.as_slice(),
            MessageError::InvalidPayload,
        ),
        // A blank destination (.NET returns before resolving it).
        (
            br#"{"type":"openLink","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"href":"  "}}"#,
            MessageError::InvalidPayload,
        ),
        // Unknown fields, wrong types, negative offsets.
        (
            br#"{"type":"openLink","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"href":"a.md","newTab":true}}"#,
            MessageError::InvalidPayload,
        ),
        (
            br#"{"type":"openLink","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"href":5}}"#,
            MessageError::InvalidPayload,
        ),
        (
            br#"{"type":"scrollChanged","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"top":-1}}"#,
            MessageError::InvalidPayload,
        ),
        (
            br#"{"type":"historyStep","payload":{"protocol":1,"tabId":7,"documentEpoch":42,"back":"yes"}}"#,
            MessageError::InvalidPayload,
        ),
        // Another tab's or an older document's link.
        (
            br#"{"type":"openLink","payload":{"protocol":1,"tabId":8,"documentEpoch":42,"href":"a.md"}}"#,
            MessageError::CrossTab,
        ),
        (
            br#"{"type":"openLink","payload":{"protocol":1,"tabId":7,"documentEpoch":41,"href":"a.md"}}"#,
            MessageError::StaleEpoch,
        ),
        (
            br#"{"type":"openLink","payload":{"protocol":2,"tabId":7,"documentEpoch":42,"href":"a.md"}}"#,
            MessageError::WrongProtocol,
        ),
    ] {
        assert_eq!(
            parse_page_message(json, SOURCE, &context()),
            Err(error),
            "{}",
            String::from_utf8_lossy(json)
        );
    }
    let long = "a".repeat(8 * 1024 + 1);
    for kind in ["openLink", "linkHover"] {
        let json = format!(
            r#"{{"type":"{kind}","payload":{{"protocol":1,"tabId":7,"documentEpoch":42,"href":"{long}"}}}}"#
        );
        assert_eq!(
            parse_page_message(json.as_bytes(), SOURCE, &context()),
            Err(MessageError::InvalidPayload),
            "{kind}"
        );
    }
}
