use std::process::Command;
use std::sync::mpsc::Sender;

use crate::*;

const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const OUTPUT_HISTORY_LIMIT: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandCaptureSpec {
    pub cwd: PathBuf,
    pub command: CapturedCommand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapturedCommand {
    Shell {
        script: String,
        shell: ShellSettings,
    },
    Compare {
        left: PathBuf,
        right: PathBuf,
        directories: bool,
    },
}

impl CommandCaptureSpec {
    pub fn shell(cwd: PathBuf, script: String, shell: ShellSettings) -> Self {
        Self {
            cwd,
            command: CapturedCommand::Shell { script, shell },
        }
    }

    pub fn summary(&self) -> String {
        sanitize_display_line(&match &self.command {
            CapturedCommand::Shell { script, .. } => script.clone(),
            CapturedCommand::Compare { left, right, .. } => {
                format!("Compare {} ↔ {}", left.display(), right.display())
            }
        })
    }
}

#[derive(Debug)]
pub(crate) struct CommandOutput {
    pub(crate) viewer: ViewerState,
    finished: bool,
}

/// Execute a captured command on a background worker and deliver bounded output.
pub fn execute_command_capture(
    job: WorkerJob,
    jobs: &Sender<JobEvent>,
    events: &Sender<BackgroundEvent>,
) {
    let JobRequest::CaptureCommand { spec } = &job.request else {
        return;
    };
    let id = job.id;
    let _ = jobs.send(JobEvent::Started { id });
    let result = (|| {
        let mut command = match &spec.command {
            CapturedCommand::Shell { script, shell } => {
                let invocation = rc_shell::resolve_shell(shell, &spec.cwd)?
                    .shell
                    .invocation(script);
                let mut command = Command::new(invocation.program);
                command.args(invocation.arguments);
                command
            }
            CapturedCommand::Compare {
                left,
                right,
                directories,
            } => crate::comparison::comparison_process(left, right, *directories, &spec.cwd)?,
        };
        command.current_dir(&spec.cwd);
        rc_shell::run_captured_command(
            command,
            job.cancel_flag().as_ref(),
            OUTPUT_LIMIT,
            |_, bytes| {
                events
                    .send(BackgroundEvent::CommandOutput {
                        job_id: id,
                        bytes: bytes.to_vec(),
                    })
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "output consumer closed")
                    })
            },
        )
    })();
    let comparison = matches!(spec.command, CapturedCommand::Compare { .. });
    let status = match &result {
        Ok(status) if comparison && status.code() == Some(0) => "No differences".into(),
        Ok(status) if comparison && status.code() == Some(1) => "Differences found".into(),
        Ok(status) => status.to_string(),
        Err(error) => error.to_string(),
    };
    let _ = events.send(BackgroundEvent::CommandCompleted { job_id: id, status });
    let result = result.map_err(JobError::from_io).and_then(|status| {
        if status.success() || (comparison && status.code() == Some(1)) {
            Ok(())
        } else {
            Err(JobError::from_message(format!("command {status}")))
        }
    });
    let _ = jobs.send(JobEvent::Finished { id, result });
}

impl AppState {
    pub(crate) fn open_filtered_view(&mut self) {
        self.push_dialog(
            DialogState::input(
                "Filtered view",
                "Shell command (runs in the panel directory):",
                "",
            ),
            PendingDialogAction::FilteredView {
                cwd: self.active_panel().cwd.clone(),
            },
        );
    }

    pub(crate) fn start_command_capture(&mut self, spec: CommandCaptureSpec) {
        if let CapturedCommand::Shell { script, .. } = &spec.command
            && (script.len() > COMMAND_BUFFER_LIMIT_BYTES || script.contains('\0'))
        {
            self.set_status("Command is too long or contains NUL");
            return;
        }
        if self.command_outputs.len() >= OUTPUT_HISTORY_LIMIT {
            let oldest =
                self.command_outputs
                    .iter()
                    .filter(|(id, output)| {
                        output.finished
                            && !self.routes.iter().any(
                                |route| matches!(route, Route::CommandOutput(open) if open == *id),
                            )
                    })
                    .map(|(id, _)| *id)
                    .min_by_key(|id| id.0);
            if let Some(id) = oldest {
                self.command_outputs.remove(&id);
            } else {
                self.set_status("Close or finish an output screen before starting another command");
                return;
            }
        }
        let title = spec.summary();
        let job_id = self.queue_worker_job_request(JobRequest::CaptureCommand { spec });
        self.command_outputs.insert(
            job_id,
            CommandOutput {
                viewer: ViewerState::command_output(title),
                finished: false,
            },
        );
        self.routes.push(Route::CommandOutput(job_id));
        self.set_status("Capturing command output; Esc returns to panels, Alt-J cancels, Jobs/Enter reopens output");
    }

    pub(crate) fn append_command_output(&mut self, id: JobId, bytes: &[u8]) {
        if let Some(output) = self.command_outputs.get_mut(&id) {
            let remaining = OUTPUT_LIMIT.saturating_sub(output.viewer.bytes.len());
            output
                .viewer
                .append_output(&bytes[..bytes.len().min(remaining)]);
        }
    }

    pub(crate) fn complete_command_output(&mut self, id: JobId, status: String) {
        if let Some(output) = self.command_outputs.get_mut(&id) {
            output.viewer.finish_output(sanitize_display_line(&status));
            output.finished = true;
        }
    }

    pub(crate) fn fail_command_output(&mut self, id: JobId, error: &JobError) {
        if let Some(output) = self.command_outputs.get_mut(&id)
            && !output.finished
        {
            // Job events can overtake buffered output events. Only the ordered
            // CommandCompleted event may flush an incomplete UTF-8 sequence.
            output
                .viewer
                .set_output_status(sanitize_display_line(&error.user_message()));
            output.finished = true;
        }
    }

    pub(crate) fn open_selected_job_output(&mut self) {
        let Some(job) = self.jobs.jobs().get(self.jobs_cursor) else {
            return;
        };
        let id = job.id;
        if self.command_outputs.contains_key(&id) {
            self.routes.push(Route::CommandOutput(id));
            self.set_status(format!("Output for job #{id}"));
        } else {
            self.set_status("This job has no retained command output");
        }
    }
}
