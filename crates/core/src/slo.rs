use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SloBudgets {
    pub ui_frame_budget: Duration,
    pub cancel_ack_budget: Duration,
    pub first_panel_refresh_budget: Duration,
    pub queue_stale_warn_after: Duration,
    pub viewer_memory_soft_limit_bytes: usize,
}

impl SloBudgets {
    pub fn is_queue_stale(self, pending_age: Duration) -> bool {
        pending_age >= self.queue_stale_warn_after
    }
}

pub const FOUNDATION_SLO: SloBudgets = SloBudgets {
    ui_frame_budget: Duration::from_millis(16),
    cancel_ack_budget: Duration::from_millis(100),
    first_panel_refresh_budget: Duration::from_millis(150),
    queue_stale_warn_after: Duration::from_secs(10),
    viewer_memory_soft_limit_bytes: 64 * 1024 * 1024,
};

#[cfg(test)]
mod tests {
    use super::{FOUNDATION_SLO, SloBudgets};
    use std::time::Duration;

    #[test]
    fn foundation_slo_values_are_non_zero_and_sane() {
        let budgets = FOUNDATION_SLO;
        assert!(budgets.ui_frame_budget > Duration::ZERO);
        assert!(budgets.cancel_ack_budget > Duration::ZERO);
        assert!(budgets.first_panel_refresh_budget > Duration::ZERO);
        assert!(budgets.queue_stale_warn_after > Duration::ZERO);
        assert!(budgets.viewer_memory_soft_limit_bytes > 0);
        assert!(
            budgets.ui_frame_budget <= budgets.cancel_ack_budget,
            "UI budget should be tighter than cancellation budget"
        );
    }

    #[test]
    fn queue_stale_threshold_is_enforced_exactly() {
        let budgets = SloBudgets {
            ui_frame_budget: Duration::from_millis(1),
            cancel_ack_budget: Duration::from_millis(1),
            first_panel_refresh_budget: Duration::from_millis(1),
            queue_stale_warn_after: Duration::from_millis(10),
            viewer_memory_soft_limit_bytes: 1,
        };
        assert!(!budgets.is_queue_stale(Duration::from_millis(9)));
        assert!(budgets.is_queue_stale(Duration::from_millis(10)));
    }
}
