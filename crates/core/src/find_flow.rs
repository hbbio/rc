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
                results.query,
                results.entries.len()
            ))
        } else {
            None
        };
        if let Some(status_message) = status_message {
            self.set_status(status_message);
        }
    }
}
