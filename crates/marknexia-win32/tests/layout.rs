use marknexia_win32::layout::{Dip, PixelRect, ShellLayout, ShellLayoutRequest};

fn assert_rectangles_stay_in_client_and_do_not_overlap(
    layout: ShellLayout,
    client_width: u32,
    client_height: u32,
) {
    let rectangles = [
        layout.command_bar,
        layout.tab_strip,
        layout.sidebar,
        layout.webview,
        layout.find_bar,
        layout.status_bar,
    ];

    for rectangle in rectangles {
        assert!(rectangle.right() <= client_width, "{rectangle:?}");
        assert!(rectangle.bottom() <= client_height, "{rectangle:?}");
    }

    for (index, first) in rectangles.iter().enumerate() {
        for second in &rectangles[index + 1..] {
            assert!(
                first.right() <= second.x
                    || second.right() <= first.x
                    || first.bottom() <= second.y
                    || second.bottom() <= first.y,
                "{first:?} overlaps {second:?}"
            );
        }
    }
}

#[test]
fn minimum_interaction_targets_scale_from_44_dip_at_supported_dpis() {
    for (dpi, expected_pixels) in [(96, 44), (144, 66), (192, 88)] {
        let layout = ShellLayout::compute(ShellLayoutRequest::new(1600, 1200, dpi));

        assert_eq!(layout.command_bar.height, expected_pixels, "dpi={dpi}");
        assert_eq!(layout.tab_strip.height, expected_pixels, "dpi={dpi}");
        assert_eq!(layout.status_bar.height, expected_pixels, "dpi={dpi}");
        assert_eq!(Dip::INTERACTION_TARGET.to_pixels(dpi), expected_pixels);
    }
}

#[test]
fn visible_find_bar_scales_from_44_dip_at_supported_dpis() {
    for (dpi, expected_pixels) in [(96, 44), (144, 66), (192, 88)] {
        let layout = ShellLayout::compute(
            ShellLayoutRequest::new(1600, 1200, dpi).with_find_bar_visible(true),
        );

        assert_eq!(layout.find_bar.height, expected_pixels, "dpi={dpi}");
    }
}

#[test]
fn visible_shell_surfaces_recompute_webview_bounds_from_client_size_and_dpi() {
    let layout = ShellLayout::compute(
        ShellLayoutRequest::new(1600, 1200, 144)
            .with_sidebar_visible(true)
            .with_find_bar_visible(true),
    );

    assert_eq!(layout.sidebar.width, 420);
    assert_eq!(layout.webview.x, 420);
    assert_eq!(layout.webview.y, 132);
    assert_eq!(layout.webview.width, 1180);
    assert_eq!(layout.webview.height, 936);
    assert_eq!(layout.find_bar.height, 66);
    assert_eq!(layout.status_bar.y, 1134);
}

#[test]
fn hiding_optional_surfaces_gives_their_space_back_to_the_webview() {
    let layout = ShellLayout::compute(ShellLayoutRequest::new(1600, 1200, 192));

    assert_eq!(layout.webview.x, 0);
    assert_eq!(layout.webview.y, 176);
    assert_eq!(layout.webview.width, 1600);
    assert_eq!(layout.webview.height, 936);
    assert_eq!(layout.status_bar.y, 1112);
}

#[test]
fn constrained_client_never_produces_negative_or_out_of_client_webview_bounds() {
    let layout = ShellLayout::compute(
        ShellLayoutRequest::new(120, 160, 192)
            .with_sidebar_visible(true)
            .with_find_bar_visible(true),
    );

    assert_eq!(layout.webview.x, 120);
    assert_eq!(layout.webview.y, 160);
    assert_eq!(layout.webview.width, 0);
    assert_eq!(layout.webview.height, 0);
    assert!(layout.webview.right() <= 120);
    assert!(layout.webview.bottom() <= 160);
}

#[test]
fn every_surface_stays_in_client_without_overlapping_at_normal_and_tiny_sizes() {
    for request in [
        ShellLayoutRequest::new(1600, 1200, 96)
            .with_sidebar_visible(true)
            .with_find_bar_visible(true),
        ShellLayoutRequest::new(120, 160, 192)
            .with_sidebar_visible(true)
            .with_find_bar_visible(true),
    ] {
        let layout = ShellLayout::compute(request);
        assert_rectangles_stay_in_client_and_do_not_overlap(
            layout,
            request.client_width,
            request.client_height,
        );
    }
}

#[test]
fn zero_and_tiny_clients_degrade_to_only_the_surfaces_that_fit() {
    let zero = ShellLayout::compute(
        ShellLayoutRequest::new(0, 0, 96)
            .with_sidebar_visible(true)
            .with_find_bar_visible(true),
    );
    assert_eq!(zero.command_bar, PixelRect::default());
    assert_eq!(zero.tab_strip, PixelRect::default());
    assert_eq!(zero.sidebar, PixelRect::default());
    assert_eq!(zero.webview, PixelRect::default());
    assert_eq!(zero.find_bar, PixelRect::default());
    assert_eq!(zero.status_bar, PixelRect::default());

    let tiny = ShellLayout::compute(
        ShellLayoutRequest::new(500, 100, 96)
            .with_sidebar_visible(true)
            .with_find_bar_visible(true),
    );
    assert_eq!(tiny.command_bar.height, 44);
    assert_eq!(tiny.tab_strip.height, 44);
    assert_eq!(tiny.status_bar.height, 12);
    assert_eq!(tiny.find_bar.height, 0);
    assert_eq!(
        tiny.sidebar,
        PixelRect {
            x: 0,
            y: 88,
            width: 280,
            height: 0
        }
    );
    assert_eq!(
        tiny.webview,
        PixelRect {
            x: 280,
            y: 88,
            width: 220,
            height: 0
        }
    );
    assert_rectangles_stay_in_client_and_do_not_overlap(tiny, 500, 100);
}
