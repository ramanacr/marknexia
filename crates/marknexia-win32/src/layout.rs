//! Pure physical-pixel layout calculations for the native shell.

const BASE_DPI: u32 = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dip(pub u32);

impl Dip {
    pub const INTERACTION_TARGET: Self = Self(44);
    const SIDEBAR_WIDTH: Self = Self(280);

    pub fn to_pixels(self, dpi: u32) -> u32 {
        self.0.saturating_mul(dpi).saturating_add(BASE_DPI - 1) / BASE_DPI
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    pub fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellLayoutRequest {
    pub client_width: u32,
    pub client_height: u32,
    pub dpi: u32,
    pub sidebar_visible: bool,
    pub find_bar_visible: bool,
}

impl ShellLayoutRequest {
    pub const fn new(client_width: u32, client_height: u32, dpi: u32) -> Self {
        Self {
            client_width,
            client_height,
            dpi,
            sidebar_visible: false,
            find_bar_visible: false,
        }
    }

    pub const fn with_sidebar_visible(mut self, visible: bool) -> Self {
        self.sidebar_visible = visible;
        self
    }

    pub const fn with_find_bar_visible(mut self, visible: bool) -> Self {
        self.find_bar_visible = visible;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellLayout {
    pub command_bar: PixelRect,
    pub tab_strip: PixelRect,
    pub sidebar: PixelRect,
    pub webview: PixelRect,
    pub find_bar: PixelRect,
    pub status_bar: PixelRect,
}

impl ShellLayout {
    pub fn compute(request: ShellLayoutRequest) -> Self {
        let target = Dip::INTERACTION_TARGET.to_pixels(request.dpi);
        let command_height = request.client_height.min(target);
        let tab_height = request
            .client_height
            .saturating_sub(command_height)
            .min(target);
        let content_top = command_height.saturating_add(tab_height);

        let available_below_top = request.client_height.saturating_sub(content_top);
        let status_height = available_below_top.min(target);
        let remaining_above_status = available_below_top.saturating_sub(status_height);
        let find_height = if request.find_bar_visible {
            remaining_above_status.min(target)
        } else {
            0
        };
        let content_bottom = request
            .client_height
            .saturating_sub(status_height)
            .saturating_sub(find_height);

        let sidebar_width = if request.sidebar_visible {
            request
                .client_width
                .min(Dip::SIDEBAR_WIDTH.to_pixels(request.dpi))
        } else {
            0
        };
        let content_width = request.client_width.saturating_sub(sidebar_width);
        let content_height = content_bottom.saturating_sub(content_top);

        Self {
            command_bar: PixelRect {
                width: request.client_width,
                height: command_height,
                ..PixelRect::default()
            },
            tab_strip: PixelRect {
                y: command_height,
                width: request.client_width,
                height: tab_height,
                ..PixelRect::default()
            },
            sidebar: PixelRect {
                y: content_top,
                width: sidebar_width,
                height: content_height,
                ..PixelRect::default()
            },
            webview: PixelRect {
                x: sidebar_width,
                y: content_top,
                width: content_width,
                height: content_height,
            },
            find_bar: PixelRect {
                x: sidebar_width,
                y: content_bottom,
                width: content_width,
                height: find_height,
            },
            status_bar: PixelRect {
                y: request.client_height.saturating_sub(status_height),
                width: request.client_width,
                height: status_height,
                ..PixelRect::default()
            },
        }
    }
}
