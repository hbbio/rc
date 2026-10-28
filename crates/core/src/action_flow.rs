use std::ffi::{OsStr, OsString};
use std::process::Command;
use std::sync::mpsc::Sender;

use crate::action_macros::{ActionOutput, ActionTemplate, MacroContext, MacroPanel};
use crate::extension_rules::{ExtensionAction, ExtensionRules};
use crate::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalActionRequest {
    pub script: OsString,
    pub environment: Vec<(String, OsString)>,
    pub cwd: PathBuf,
    pub label: String,
    pub edited_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingAction {
    template: ActionTemplate,
    context: MacroContext,
    answers: Vec<String>,
    label: String,
    active: ActivePanel,
    action: Option<ExtensionAction>,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingExtension {
    rules: Arc<ExtensionRules>,
    context: MacroContext,
    active: ActivePanel,
    action: ExtensionAction,
    is_directory: bool,
}

fn extension_environment(context: &MacroContext) -> Result<Vec<(String, OsString)>, String> {
    let panel = &context.panels[0];
    let mut environment = vec![(
        "MC_EXT_CURRENTDIR".into(),
        panel.directory.as_os_str().to_owned(),
    )];
    if let Some(path) = &panel.selected {
        let local = context.local_file.as_deref().unwrap_or(path);
        environment.push(("MC_EXT_FILENAME".into(), local.as_os_str().to_owned()));
        environment.push((
            "MC_EXT_BASENAME".into(),
            path.file_name().unwrap_or(path.as_os_str()).to_owned(),
        ));
    }
    let tagged = quoted_path_list(&panel.tagged, &panel.directory)?;
    let selected = if panel.tagged.is_empty() {
        quoted_path_list(
            &panel.selected.iter().cloned().collect::<Vec<_>>(),
            &panel.directory,
        )?
    } else {
        tagged.clone()
    };
    environment.push(("MC_EXT_ONLYTAGGED".into(), tagged));
    environment.push(("MC_EXT_SELECTED".into(), selected));
    Ok(environment)
}

fn quoted_path_list(paths: &[PathBuf], cwd: &Path) -> Result<OsString, String> {
    let mut result = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let bytes = path
            .strip_prefix(cwd)
            .unwrap_or(path)
            .as_os_str()
            .as_encoded_bytes();
        if index > 0 {
            result.push(b' ');
        }
        result.push(b'\'');
        for byte in bytes {
            if *byte == b'\'' {
                result.extend_from_slice(b"'\\''");
            } else {
                result.push(*byte);
            }
            if result.len() > COMMAND_BUFFER_LIMIT_BYTES {
                return Err("Extension selected-file environment exceeds 64 KiB".into());
            }
        }
        result.push(b'\'');
    }
    crate::action_macros::os_from_bytes(&result)
}

pub fn execute_action_viewer(
    job: WorkerJob,
    jobs: &Sender<JobEvent>,
    events: &Sender<BackgroundEvent>,
) {
    let JobRequest::LoadActionViewer { path, options } = &job.request else {
        return;
    };
    let id = job.id;
    let _ = jobs.send(JobEvent::Started { id });
    let result = ViewerState::open_cancellable(path.clone(), job.cancel_flag().as_ref())
        .map(|mut viewer| {
            viewer.apply_action_view_options(*options);
            viewer
        })
        .map_err(|error| error.to_string());
    let result = if job.cancel_flag().load(std::sync::atomic::Ordering::Relaxed) {
        Err(JOB_CANCELED_MESSAGE.into())
    } else {
        result
    };
    let _ = events.send(BackgroundEvent::ViewerLoaded {
        path: path.clone(),
        result: result.clone(),
    });
    let _ = jobs.send(JobEvent::Finished {
        id,
        result: result.map(|_| ()).map_err(JobError::from_message),
    });
}

pub fn execute_file_type_probe(
    job: WorkerJob,
    jobs: &Sender<JobEvent>,
    events: &Sender<BackgroundEvent>,
) {
    let JobRequest::ProbeFileType { path } = &job.request else {
        return;
    };
    let id = job.id;
    let _ = jobs.send(JobEvent::Started { id });
    let result = (|| -> io::Result<Option<Vec<u8>>> {
        if job.cancel_flag().load(std::sync::atomic::Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                JOB_CANCELED_MESSAGE,
            ));
        }
        let cwd = path.parent().unwrap_or(Path::new("/"));
        let Some(program) = rc_shell::find_program_on_path(OsStr::new("file"), cwd) else {
            return Ok(None);
        };
        let mut command = Command::new(program);
        command
            .args(["-L", "-b", "--"])
            .arg(path)
            .env("LC_ALL", "C");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let status = rc_shell::run_captured_command(
            command,
            job.cancel_flag().as_ref(),
            64 * 1024,
            |stream, bytes| {
                match stream {
                    rc_shell::OutputStream::Stdout => stdout.extend_from_slice(bytes),
                    rc_shell::OutputStream::Stderr => stderr.extend_from_slice(bytes),
                }
                Ok(())
            },
        )?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "file {status}: {}",
                String::from_utf8_lossy(&stderr)
            )));
        }
        // MC matches the first description line without the file utility's newline.
        if let Some(end) = stdout.iter().position(|byte| *byte == b'\n') {
            stdout.truncate(end);
        }
        Ok(Some(stdout))
    })()
    .map_err(JobError::from_io);
    let _ = events.send(BackgroundEvent::FileTypeProbed {
        job_id: id,
        result: result.clone().map_err(|error| error.user_message()),
    });
    let _ = jobs.send(JobEvent::Finished {
        id,
        result: result.map(|_| ()),
    });
}

impl AppState {
    pub fn take_pending_external_action_requests(&mut self) -> Vec<ExternalActionRequest> {
        std::mem::take(&mut self.pending_external_action_requests)
    }

    pub fn reload_extension_rules(&mut self) {
        if let Some((id, _)) = self.pending_extension.take() {
            self.request_cancel_for_job(id);
        }
        self.extension_rules = self
            .configuration_files
            .read(ConfigurationFile::Extensions, None)
            .map_err(|error| error.to_string())
            .and_then(|source| ExtensionRules::parse(&source))
            .map(Arc::new);
        if let Err(error) = &self.extension_rules {
            self.set_status(format!("Cannot load mc.ext.ini: {error}"));
        }
    }

    pub(crate) fn macro_context(&self) -> MacroContext {
        MacroContext {
            panels: [self.active_panel(), self.passive_panel()].map(|panel| MacroPanel {
                directory: panel.cwd.clone(),
                selected: panel
                    .selected_entry()
                    .filter(|entry| !entry.is_parent())
                    .map(|entry| entry.path.clone()),
                tagged: panel.tagged_paths_in_operation_order(),
            }),
            local_file: None,
        }
    }

    pub(crate) fn try_extension_action(&mut self, action: ExtensionAction) -> bool {
        if self.active_panel().is_archive() {
            return false;
        }
        let Some(entry) = self.selected_non_parent_entry() else {
            return false;
        };
        let path = entry.path.clone();
        let is_directory = entry.is_dir();
        let rules = match &self.extension_rules {
            Ok(rules) => Arc::clone(rules),
            Err(error) => {
                if is_directory {
                    return false;
                }
                self.set_status(format!("Cannot use mc.ext.ini: {error}"));
                return true;
            }
        };
        if is_directory && !rules.has_directory_match(&path) {
            return false;
        }
        if let Some((id, _)) = self.pending_extension.take() {
            self.request_cancel_for_job(id);
        }
        if rules.needs_file_type(&path, is_directory) {
            let pending = PendingExtension {
                rules,
                context: self.macro_context(),
                active: self.active_panel,
                action,
                is_directory,
            };
            let id = self.queue_worker_job_request(JobRequest::ProbeFileType { path });
            self.pending_extension = Some((id, pending));
            self.set_status("Checking file type for extension rules...");
            return true;
        }
        let Some(command) = rules.resolve(&path, is_directory, None, action) else {
            return false;
        };
        self.begin_action(
            command,
            format!("{action:?}: {}", path.display()),
            Some(action),
        );
        true
    }

    pub(crate) fn finish_extension_probe(
        &mut self,
        id: JobId,
        result: Result<Option<Vec<u8>>, String>,
    ) {
        if !self
            .pending_extension
            .as_ref()
            .is_some_and(|(pending, _)| *pending == id)
        {
            return;
        }
        let (_, pending) = self
            .pending_extension
            .take()
            .expect("matching extension request");
        if pending.active != self.active_panel
            || pending.context != self.macro_context()
            || !matches!(self.top_route(), Route::FileManager)
        {
            self.set_status("File action canceled because the panel context changed");
            return;
        }
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                self.set_status(format!("File type check failed: {error}"));
                return;
            }
        };
        let path = pending.context.panels[0]
            .selected
            .as_deref()
            .expect("selected extension file");
        if let Some(command) = pending.rules.resolve(
            path,
            pending.is_directory,
            output.as_deref(),
            pending.action,
        ) {
            self.begin_action(
                command,
                format!("{:?}: {}", pending.action, path.display()),
                Some(pending.action),
            );
        } else {
            self.apply_builtin_file_action(pending.action);
        }
    }

    pub(crate) fn begin_action(
        &mut self,
        command: &str,
        label: String,
        action: Option<ExtensionAction>,
    ) {
        let template = match ActionTemplate::parse(command) {
            Ok(template) => template,
            Err(error) => {
                self.set_status(format!("Invalid action: {error}"));
                return;
            }
        };
        let mut context = self.macro_context();
        if action.is_none() {
            for (snapshot, panel) in context
                .panels
                .iter_mut()
                .zip([self.active_panel(), self.passive_panel()])
            {
                if panel
                    .selected_entry()
                    .is_some_and(|entry| entry.is_parent())
                {
                    snapshot.selected = Some(panel.cwd.join(".."));
                }
            }
        }
        self.continue_action(PendingAction {
            template,
            context,
            answers: Vec::new(),
            label,
            active: self.active_panel,
            action,
        });
    }

    pub(crate) fn answer_action_prompt(&mut self, mut pending: PendingAction, answer: String) {
        pending.answers.push(answer);
        self.continue_action(pending);
    }

    fn continue_action(&mut self, pending: PendingAction) {
        if let Some(prompt) = pending.template.prompts().get(pending.answers.len()) {
            self.push_dialog(
                DialogState::input("Action parameter", *prompt, ""),
                PendingDialogAction::ActionPrompt(Box::new(pending)),
            );
            return;
        }
        let mut expanded =
            match pending
                .template
                .expand(&pending.context, &pending.answers, |name| {
                    std::env::var_os(name)
                }) {
                Ok(expanded) => expanded,
                Err(error) => {
                    self.set_status(format!("Cannot expand action: {error}"));
                    return;
                }
            };
        let cwd = pending.context.panels[0].directory.clone();
        let selected = pending.context.panels[0].selected.clone();
        if pending.action.is_some() {
            match extension_environment(&pending.context) {
                Ok(environment) => expanded.environment.extend(environment),
                Err(error) => {
                    self.set_status(error);
                    return;
                }
            }
        }
        let argument_bytes = expanded
            .environment
            .iter()
            .fold(expanded.script.len(), |total, (key, value)| {
                total.saturating_add(key.len() + value.len() + 2)
            });
        if argument_bytes > COMMAND_BUFFER_LIMIT_BYTES {
            self.set_status("Expanded action and environment exceed 64 KiB");
            return;
        }
        match expanded.output {
            ActionOutput::ChangeDirectory => {
                if !self.change_action_directory(cwd.join(&expanded.script)) {
                    return;
                }
            }
            ActionOutput::Viewer(options)
                if expanded
                    .script
                    .to_str()
                    .is_some_and(|script| script.trim().is_empty()) =>
            {
                let Some(path) = selected else {
                    self.set_status("View action requires a selected file");
                    return;
                };
                self.queue_worker_job_request(JobRequest::LoadActionViewer { path, options });
                self.set_status("Opening viewer...");
            }
            ActionOutput::Viewer(options) => {
                let spec = CommandCaptureSpec {
                    cwd,
                    command: CapturedCommand::Action {
                        script: expanded.script,
                        environment: expanded.environment,
                        label: pending.label,
                    },
                };
                let Some(id) = self.try_start_command_capture(spec) else {
                    return;
                };
                self.command_outputs
                    .get_mut(&id)
                    .expect("new command output")
                    .viewer
                    .apply_action_view_options(options);
            }
            ActionOutput::Terminal => {
                self.pending_external_action_requests
                    .push(ExternalActionRequest {
                        script: expanded.script,
                        environment: expanded.environment,
                        cwd,
                        label: pending.label,
                        edited_path: (pending.action == Some(ExtensionAction::Edit))
                            .then_some(selected)
                            .flatten(),
                    });
                self.set_status("Running configured action...");
            }
        }
        for (index, paths) in expanded.untag.into_iter().enumerate() {
            let side = if index == 0 {
                pending.active
            } else {
                pending.active.other()
            };
            if self.panels[side.index()].cwd == pending.context.panels[index].directory {
                for path in paths {
                    self.panels[side.index()].tagged.remove(&path);
                }
                self.sync_selection_size(side, false);
            }
        }
    }

    fn change_action_directory(&mut self, path: PathBuf) -> bool {
        let bytes = path.as_os_str().as_encoded_bytes();
        for suffix in [b"/utar://".as_slice(), b"/uzip://", b"/zip://"] {
            if let Some(archive) = bytes.strip_suffix(suffix) {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStrExt;
                    let archive = PathBuf::from(OsStr::from_bytes(archive));
                    if ArchiveFormat::for_path(&archive).is_some() {
                        if let Err(error) = self.open_archive_path(archive) {
                            self.set_status(format!("Cannot open archive: {error}"));
                            return false;
                        }
                        self.queue_panel_refresh(self.active_panel);
                        self.set_status("Opening archive...");
                        return true;
                    }
                }
                #[cfg(not(unix))]
                let _ = archive;
                self.set_status("This archive backend is unavailable");
                return false;
            }
        }
        match self.set_active_panel_directory(path) {
            Ok(true) => {
                self.set_status("Loading directory...");
                true
            }
            Ok(false) => {
                self.set_status("Action directory is not accessible");
                false
            }
            Err(error) => {
                self.set_status(format!("Cannot change directory: {error}"));
                false
            }
        }
    }

    pub(crate) fn apply_builtin_file_action(&mut self, action: ExtensionAction) {
        match action {
            ExtensionAction::Open => {
                let opened_archive = match self.open_selected_archive() {
                    Ok(opened) => opened,
                    Err(error) => {
                        self.set_status(format!("Cannot open archive: {error}"));
                        return;
                    }
                };
                if opened_archive || self.open_selected_directory() {
                    self.queue_panel_refresh(self.active_panel);
                    self.set_status("Loading selected directory...");
                } else if self.active_panel().is_archive() {
                    if self.open_selected_file_in_viewer() {
                        self.set_status("Opening viewer...");
                    } else {
                        self.set_status("No entry selected");
                    }
                } else {
                    let status = match self.execute_selected_file() {
                        ExecuteSelectionResult::OpenedExternal => "Executing selected file...",
                        ExecuteSelectionResult::QueuedDesktopOpen => {
                            "Opening with the default application..."
                        }
                        ExecuteSelectionResult::NoEntrySelected => "No entry selected",
                        ExecuteSelectionResult::SelectedEntryIsDirectory => {
                            "Selected directory could not be opened"
                        }
                    };
                    self.set_status(status);
                }
            }
            ExtensionAction::View => {
                if self.open_selected_directory() {
                    self.queue_panel_refresh(self.active_panel);
                    self.set_status("Loading selected directory...");
                } else if self.open_selected_file_in_viewer() {
                    self.set_status("Opening viewer...");
                } else {
                    self.set_status("No entry selected");
                }
            }
            ExtensionAction::Edit => {
                let status = match self.open_selected_file_in_editor() {
                    EditSelectionResult::OpenedExternal => "Opening external editor...",
                    EditSelectionResult::NoEditorResolved => {
                        "No external editor found; set editor_command, EDITOR, or VISUAL"
                    }
                    EditSelectionResult::NoEntrySelected => "No entry selected",
                    EditSelectionResult::SelectedEntryIsDirectory => "Directory cannot be edited",
                };
                self.set_status(status);
            }
        }
    }
}
