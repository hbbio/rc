use crate::*;

impl AppState {
    pub(crate) fn handle_find_entries_chunk(
        &mut self,
        job_id: JobId,
        entries: Vec<FindResultEntry>,
    ) {
        let status_message = if let Some(results) = self.find_results_by_job_id_mut(job_id) {
            let was_empty = results.entries.is_empty();
            results.entries.extend(entries);
            if was_empty && !results.entries.is_empty() {
                results.cursor = 0;
            }
            Some(format!(
                "Finding '{}': {} result(s)...",
                results.spec.display_pattern(),
                results.entries.len()
            ))
        } else {
            None
        };
        if let Some(status_message) = status_message {
            self.set_status(status_message);
        }
    }

    pub(crate) fn handle_find_completed(&mut self, job_id: JobId, report: FindSearchReport) {
        let status = self.find_results_by_job_id_mut(job_id).map(|results| {
            results.apply_report(report);
            find_results_status_message(results)
        });
        if let Some(status) = status {
            self.set_status(status);
        }
    }
}

pub(crate) fn find_results_status_message(results: &FindResultsState) -> String {
    let mut details = Vec::new();
    if let Some(report) = &results.report {
        if report.truncated {
            details.push("result limit reached".to_string());
        }
        if report.issue_count > 0 {
            details.push(format!("{} read error(s)", report.issue_count));
        }
    }
    if let FindResultsStatus::Failed(message) = &results.status {
        details.push(message.clone());
    }
    let suffix = if details.is_empty() {
        String::new()
    } else {
        format!(" ({})", details.join(", "))
    };
    format!(
        "Find '{}': {} result(s), {}{}",
        results.spec.display_pattern(),
        results.entries.len(),
        results.status.label(),
        suffix
    )
}
