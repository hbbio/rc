#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScreenRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl ScreenRect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn contains(self, column: u16, row: u16) -> bool {
        column >= self.x
            && column < self.x.saturating_add(self.width)
            && row >= self.y
            && row < self.y.saturating_add(self.height)
    }

    pub const fn inset(self, amount: u16) -> Self {
        let horizontal = amount.saturating_mul(2);
        let vertical = amount.saturating_mul(2);
        let x_inset = if amount < self.width {
            amount
        } else {
            self.width
        };
        let y_inset = if amount < self.height {
            amount
        } else {
            self.height
        };
        Self {
            x: self.x.saturating_add(x_inset),
            y: self.y.saturating_add(y_inset),
            width: self.width.saturating_sub(horizontal),
            height: self.height.saturating_sub(vertical),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListOverlayLayout {
    pub outer: ScreenRect,
    pub header: ScreenRect,
    pub list: ScreenRect,
    pub footer: ScreenRect,
}

pub const STANDARD_DIALOG_WIDTH: u16 = 56;
pub const STANDARD_DIALOG_HEIGHT: u16 = 14;
pub const FIND_DIALOG_WIDTH: u16 = 88;
pub const FIND_DIALOG_HEIGHT: u16 = 20;
pub const FIND_RESULTS_WIDTH: u16 = 96;
pub const FIND_RESULTS_HEIGHT: u16 = 28;
pub const TREE_WIDTH: u16 = 88;
pub const TREE_HEIGHT: u16 = 28;
pub const HOTLIST_WIDTH: u16 = 88;
pub const HOTLIST_HEIGHT: u16 = 22;

pub const fn centered_overlay_rect(
    viewport: ScreenRect,
    requested_width: u16,
    requested_height: u16,
) -> ScreenRect {
    let width = if requested_width < viewport.width.saturating_sub(2) {
        requested_width
    } else {
        viewport.width.saturating_sub(2)
    };
    let height = if requested_height < viewport.height.saturating_sub(2) {
        requested_height
    } else {
        viewport.height.saturating_sub(2)
    };
    ScreenRect {
        x: viewport
            .x
            .saturating_add(viewport.width.saturating_sub(width) / 2),
        y: viewport
            .y
            .saturating_add(viewport.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

pub const fn list_overlay_layout(
    viewport: ScreenRect,
    width: u16,
    height: u16,
    requested_header_height: u16,
    requested_footer_height: u16,
) -> ListOverlayLayout {
    let outer = centered_overlay_rect(viewport, width, height);
    let inner = outer.inset(1);
    let header_height = if requested_header_height < inner.height {
        requested_header_height
    } else {
        inner.height
    };
    let after_header = inner.height.saturating_sub(header_height);
    let footer_height = if requested_footer_height < after_header {
        requested_footer_height
    } else {
        after_header
    };
    let list_height = after_header.saturating_sub(footer_height);
    let header = ScreenRect::new(inner.x, inner.y, inner.width, header_height);
    let list = ScreenRect::new(
        inner.x,
        inner.y.saturating_add(header_height),
        inner.width,
        list_height,
    );
    let footer = ScreenRect::new(
        inner.x,
        list.y.saturating_add(list_height),
        inner.width,
        footer_height,
    );
    ListOverlayLayout {
        outer,
        header,
        list,
        footer,
    }
}

pub const fn find_results_layout(viewport: ScreenRect) -> ListOverlayLayout {
    list_overlay_layout(viewport, FIND_RESULTS_WIDTH, FIND_RESULTS_HEIGHT, 2, 2)
}

pub const fn tree_layout(viewport: ScreenRect) -> ListOverlayLayout {
    list_overlay_layout(viewport, TREE_WIDTH, TREE_HEIGHT, 1, 2)
}

pub const fn hotlist_layout(viewport: ScreenRect) -> ListOverlayLayout {
    list_overlay_layout(viewport, HOTLIST_WIDTH, HOTLIST_HEIGHT, 0, 1)
}

pub const fn listbox_dialog_layout(viewport: ScreenRect, footer_height: u16) -> ListOverlayLayout {
    list_overlay_layout(
        viewport,
        STANDARD_DIALOG_WIDTH,
        STANDARD_DIALOG_HEIGHT,
        0,
        footer_height,
    )
}

pub fn visible_window(total: usize, cursor: usize, viewport_rows: usize) -> (usize, usize) {
    if total == 0 || viewport_rows == 0 {
        return (0, 0);
    }

    let visible = viewport_rows.min(total);
    let mut start = cursor.saturating_sub(visible / 2);
    if start + visible > total {
        start = total.saturating_sub(visible);
    }
    (start, start + visible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_layout_clamps_to_a_small_viewport() {
        let viewport = ScreenRect::new(4, 7, 10, 8);
        let layout = find_results_layout(viewport);

        assert_eq!(layout.outer, ScreenRect::new(5, 8, 8, 6));
        assert_eq!(layout.header, ScreenRect::new(6, 9, 6, 2));
        assert_eq!(layout.list, ScreenRect::new(6, 11, 6, 0));
        assert_eq!(layout.footer, ScreenRect::new(6, 11, 6, 2));
    }

    #[test]
    fn visible_window_centers_selection_and_clamps_at_edges() {
        assert_eq!(visible_window(20, 0, 5), (0, 5));
        assert_eq!(visible_window(20, 10, 5), (8, 13));
        assert_eq!(visible_window(20, 19, 5), (15, 20));
        assert_eq!(visible_window(0, 0, 5), (0, 0));
    }
}
