use std::io;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use rc_core::{AppState, ViewerFileSnapshot};

const CHECK_INTERVAL: Duration = Duration::from_millis(500);
// Blocking filesystem calls may outlive their viewers. Leave room for new
// snapshots while bounding the number of workers that can become stuck.
const MAX_CHECK_WORKERS: usize = 4;

#[derive(Default)]
pub(crate) struct ViewerWatch {
    workers: Vec<ViewerCheckWorker>,
    next_check: Option<(Arc<ViewerFileSnapshot>, Instant)>,
}

struct ViewerCheckWorker {
    checks: mpsc::Sender<Arc<ViewerFileSnapshot>>,
    results: mpsc::Receiver<bool>,
    pending: Option<Arc<ViewerFileSnapshot>>,
}

impl ViewerWatch {
    pub(crate) fn poll(&mut self, state: &mut AppState) {
        self.poll_with(state, ViewerFileSnapshot::poll_for_change);
    }

    fn poll_with(
        &mut self,
        state: &mut AppState,
        check: impl Fn(&ViewerFileSnapshot) -> bool + Send + 'static,
    ) {
        self.workers
            .retain_mut(|worker| worker.collect_result(state));

        let Some(snapshot) = state.viewer_file_to_watch() else {
            self.next_check = None;
            return;
        };
        let now = Instant::now();
        if self
            .next_check
            .as_ref()
            .is_some_and(|(last_snapshot, next_check)| {
                Arc::ptr_eq(last_snapshot, snapshot) && now < *next_check
            })
            || self.workers.iter().any(|worker| {
                worker
                    .pending
                    .as_ref()
                    .is_some_and(|pending| Arc::ptr_eq(pending, snapshot))
            })
        {
            return;
        }

        let worker_index = match self
            .workers
            .iter()
            .position(|worker| worker.pending.is_none())
        {
            Some(index) => index,
            None if self.workers.len() < MAX_CHECK_WORKERS => {
                match ViewerCheckWorker::spawn(check) {
                    Ok(worker) => {
                        self.workers.push(worker);
                        self.workers.len() - 1
                    }
                    Err(error) => {
                        self.next_check = Some((Arc::clone(snapshot), now + CHECK_INTERVAL));
                        state.set_viewer_change_detection_unavailable(true);
                        state.set_status(format!("Viewer file check unavailable ({error})"));
                        return;
                    }
                }
            }
            None => {
                state.set_viewer_change_detection_unavailable(true);
                return;
            }
        };
        self.next_check = Some((Arc::clone(snapshot), now + CHECK_INTERVAL));
        let worker = &mut self.workers[worker_index];
        let scheduled = worker.checks.send(Arc::clone(snapshot)).is_ok();
        if scheduled {
            worker.pending = Some(Arc::clone(snapshot));
        }
        state.set_viewer_change_detection_unavailable(!scheduled);
    }
}

impl ViewerCheckWorker {
    fn spawn(check: impl Fn(&ViewerFileSnapshot) -> bool + Send + 'static) -> io::Result<Self> {
        let (checks, worker_checks) = mpsc::channel::<Arc<ViewerFileSnapshot>>();
        let (worker_results, results) = mpsc::channel();
        // A metadata lookup may block on an unavailable mount. Keep it off the
        // notification thread and let shutdown drop the channels without joining.
        thread::Builder::new()
            .name(String::from("rc-viewer-watch"))
            .spawn(move || {
                for snapshot in worker_checks {
                    if worker_results.send(check(&snapshot)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            checks,
            results,
            pending: None,
        })
    }

    fn collect_result(&mut self, state: &mut AppState) -> bool {
        match self.results.try_recv() {
            Ok(changed) => {
                if let Some(snapshot) = self.pending.take()
                    && changed
                {
                    state.mark_viewer_file_changed(&snapshot);
                }
                true
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use ratatui::{Terminal, backend::TestBackend};
    use rc_core::{AppCommand, BackgroundEvent, ViewerState, keymap::Keymap};

    use super::*;

    fn show_file(state: &mut AppState, path: &Path) {
        state.handle_background_event(BackgroundEvent::ViewerLoaded {
            path: path.to_path_buf(),
            result: Ok(ViewerState::open(path.to_path_buf()).unwrap()),
        });
    }

    fn start_blocked_check(watch: &mut ViewerWatch, state: &mut AppState) -> mpsc::Sender<()> {
        let blocked_snapshot = Arc::clone(state.viewer_file_to_watch().unwrap());
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        watch.poll_with(state, move |snapshot| {
            if !std::ptr::eq(snapshot, blocked_snapshot.as_ref()) {
                return snapshot.poll_for_change();
            }
            let _ = started.send(());
            let _ = release_rx.recv();
            true
        });
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        release
    }

    #[test]
    fn a_blocked_lookup_does_not_stall_a_new_viewer() {
        let root = tempfile::tempdir().unwrap();
        let first_path = root.path().join("first.txt");
        let second_path = root.path().join("second.txt");
        fs::write(&first_path, "first").unwrap();
        fs::write(&second_path, "second").unwrap();
        let mut state = AppState::new(root.path().to_path_buf()).unwrap();
        let mut watch = ViewerWatch::default();

        show_file(&mut state, &first_path);
        let release = start_blocked_check(&mut watch, &mut state);

        state.apply(AppCommand::CloseViewer).unwrap();
        show_file(&mut state, &second_path);
        fs::write(&second_path, "updated second").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !state.active_viewer().unwrap().updated_on_disk() {
            watch.poll(&mut state);
            assert!(
                Instant::now() < deadline,
                "the new viewer should detect its edit"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(watch.workers[0].pending.is_some());

        state.apply(AppCommand::CloseViewer).unwrap();
        show_file(&mut state, &second_path);
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while watch.workers[0].pending.is_some() {
            watch.workers[0].collect_result(&mut state);
            assert!(Instant::now() < deadline, "the old lookup should finish");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!state.active_viewer().unwrap().updated_on_disk());
    }

    #[test]
    fn a_full_pool_shows_manual_refresh_until_checks_resume() {
        let root = tempfile::tempdir().unwrap();
        let mut state = AppState::new(root.path().to_path_buf()).unwrap();
        state.set_keybinding_hints_from_keymap(&Keymap::bundled_mc_default().unwrap());
        let mut watch = ViewerWatch::default();
        let mut releases = Vec::new();
        for index in 0..MAX_CHECK_WORKERS {
            let path = root.path().join(format!("blocked-{index}.txt"));
            fs::write(&path, "blocked").unwrap();
            show_file(&mut state, &path);
            releases.push(start_blocked_check(&mut watch, &mut state));
            state.apply(AppCommand::CloseViewer).unwrap();
        }

        let path = root.path().join("local.txt");
        fs::write(&path, "original").unwrap();
        show_file(&mut state, &path);
        fs::write(&path, "updated local file").unwrap();
        watch.poll(&mut state);
        assert!(
            state
                .active_viewer()
                .unwrap()
                .change_detection_unavailable()
        );

        state.set_status("Another job finished");
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| rc_ui::render(frame, &state)).unwrap();
        let displayed: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(displayed.contains("Automatic change detection unavailable, press r to refresh"));

        releases.pop().unwrap().send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !state.active_viewer().unwrap().updated_on_disk() {
            watch.poll(&mut state);
            assert!(Instant::now() < deadline, "automatic checks should resume");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !state
                .active_viewer()
                .unwrap()
                .change_detection_unavailable()
        );
    }
}
