use super::*;
use std::ffi::OsStr;
use std::thread;

use rc_core::{CommandLineInput, ShellResolutionResponse};
use rc_shell::{ShellDialect, ShellResolution};

fn pump(
    shell: &mut ShellController,
    state: &mut AppState,
    ready: impl Fn(&ShellController, &AppState) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(shell.update(state, 80, 25).is_empty());
        if ready(shell, state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "shell did not settle: {}",
            state.status_line
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn submit(state: &mut AppState, command: &str) {
    state
        .handle_command_line_input(CommandLineInput::Paste(command.into()))
        .unwrap();
    state
        .handle_command_line_input(CommandLineInput::Enter)
        .unwrap();
}

fn idle(shell: &ShellController, _: &AppState) -> bool {
    shell
        .session
        .as_ref()
        .is_some_and(|session| session.initialized && !session.busy())
}

#[test]
fn interactive_startup_accepts_input_before_a_queued_command() {
    const FLAG: &str = "RC_TEST_SUBSHELL_INTERACTIVE_STARTUP";
    if std::env::var_os(FLAG).is_none() {
        let files = tempfile::tempdir().unwrap();
        let startup = files.path().join("startup");
        std::fs::write(
            &startup,
            "printf 'STARTUP_QUESTION\\n'\nread rc_startup_answer\n",
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "subshell::tests::interactive_startup_accepts_input_before_a_queued_command",
                "--nocapture",
            ])
            .env(FLAG, "1")
            .env("ENV", startup)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let files = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(files.path()).unwrap();
    let spec = ResolvedShell {
        program: "/bin/sh".into(),
        dialect: ShellDialect::Posix,
        argument_template: vec!["-c".into(), "{command}".into()],
        identity: "sh".into(),
    };
    let mut state = AppState::new(root.clone()).unwrap();
    let mut shell = ShellController::default();
    shell.start(&state, spec.clone(), (24, 80), true).unwrap();
    shell.session.as_mut().unwrap().pending = Some(ForegroundShellRequest {
        activation_id: 1,
        buffer_revision: state.command_line_model().revision(),
        command: "printf 'ANSWER:%s:DONE' \"$rc_startup_answer\"".into(),
        cwd: root,
        shell: spec,
    });
    pump(&mut shell, &mut state, |shell, _| {
        shell.session.as_ref().is_some_and(|session| {
            session
                .view
                .screen()
                .contents()
                .contains("STARTUP_QUESTION")
        })
    });
    assert!(!shell.session.as_ref().unwrap().initialized);
    shell
        .session
        .as_mut()
        .unwrap()
        .send_input(b"reply\r", &mut state)
        .unwrap();
    pump(&mut shell, &mut state, idle);
    assert!(
        shell
            .session
            .as_ref()
            .unwrap()
            .view
            .screen()
            .contents()
            .contains("ANSWER:reply:DONE")
    );
}

#[test]
fn shell_ui_preserves_state_syncs_directories_and_hides_transport_commands() {
    let files = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(files.path()).unwrap();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    for name in ["sh", "fish"] {
        let Some(program) = rc_shell::find_program_on_path(OsStr::new(name), &root) else {
            continue;
        };
        let spec = ResolvedShell {
            program,
            dialect: if name == "fish" {
                ShellDialect::Fish
            } else {
                ShellDialect::Posix
            },
            argument_template: vec!["-c".into(), "{command}".into()],
            identity: name.into(),
        };
        let mut state = AppState::new(root.clone()).unwrap();
        let mut shell = ShellController::default();
        shell.start(&state, spec.clone(), (24, 80), false).unwrap();
        pump(&mut shell, &mut state, idle);
        state.open_command_line();
        let resolution = state.take_pending_shell_resolution_request().unwrap();
        state.handle_shell_resolution_response(ShellResolutionResponse {
            request_id: resolution.request_id,
            cwd: resolution.cwd,
            result: Ok(ShellResolution {
                shell: spec,
                diagnostic: None,
            }),
        });
        submit(
            &mut state,
            if name == "fish" {
                "set rc_ui_value remembered"
            } else {
                "rc_ui_value=remembered"
            },
        );
        pump(&mut shell, &mut state, idle);
        assert!(state.command_line_model().value().is_empty());
        submit(
            &mut state,
            "cd nested; printf 'VALUE:%s:END\\n' \"$rc_ui_value\"",
        );
        pump(&mut shell, &mut state, idle);
        assert_eq!(state.active_panel().cwd, nested, "{name}");
        let contents = shell.session.as_ref().unwrap().view.screen().contents();
        assert!(
            contents.contains("VALUE:remembered:END"),
            "{name}: {contents}"
        );
        assert!(
            !contents.contains("__rc_") && !contents.contains("RC_SUBSHELL_FILES"),
            "{name}: transport echo leaked: {contents}"
        );
        shell.hide();
        assert!(!shell.visible());
        state.apply(rc_core::AppCommand::TogglePanels).unwrap();
        shell.update(&mut state, 80, 25);
        assert!(shell.visible());
        assert!(
            shell
                .session
                .as_ref()
                .unwrap()
                .view
                .screen()
                .contents()
                .contains("VALUE:remembered:END")
        );
        // Panel navigation drives the idle shell in the other direction.
        state.panels[state.active_panel.index()].cwd = root.clone();
        pump(&mut shell, &mut state, idle);
        assert!(same_directory(
            &shell
                .session
                .as_ref()
                .unwrap()
                .backend
                .last_prompt()
                .unwrap()
                .directory,
            &root
        ));
        state.panels[state.active_panel.index()].cwd = nested.clone();
        shell.update(&mut state, 80, 25);
        assert!(shell.session.as_ref().unwrap().synchronizing.is_some());
        shell
            .session
            .as_mut()
            .unwrap()
            .send_input(b"printf QUEUED", &mut state)
            .unwrap();
        shell
            .session
            .as_mut()
            .unwrap()
            .send_input(b" > queued-input\r", &mut state)
            .unwrap();
        pump(&mut shell, &mut state, |shell, state| {
            idle(shell, state) && nested.join("queued-input").exists()
        });
        assert_eq!(
            std::fs::read(nested.join("queued-input")).unwrap(),
            b"QUEUED"
        );
        assert!(!root.join("queued-input").exists());
        std::fs::remove_file(nested.join("queued-input")).unwrap();

        let chosen = root.join(format!("chosen-{name}"));
        std::fs::create_dir(&chosen).unwrap();
        submit(&mut state, "printf BUSY; sleep 0.15; cd ..");
        pump(&mut shell, &mut state, |shell, _| {
            shell
                .session
                .as_ref()
                .is_some_and(|session| session.running.is_some() && session.echo.is_none())
        });
        shell.hide();
        state.panels[state.active_panel.index()].cwd = chosen.clone();
        state
            .handle_command_line_input(CommandLineInput::Clear)
            .unwrap();
        state
            .handle_command_line_input(CommandLineInput::Paste("new draft".into()))
            .unwrap();
        pump(&mut shell, &mut state, idle);
        assert_eq!(
            state.active_panel().cwd,
            chosen,
            "{name}: overwrote newer navigation"
        );
        assert_eq!(
            state.command_line_model().value(),
            "new draft",
            "{name}: overwrote newer draft"
        );
        assert!(same_directory(
            &shell
                .session
                .as_ref()
                .unwrap()
                .backend
                .last_prompt()
                .unwrap()
                .directory,
            &chosen
        ));
    }
}
