use super::*;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

#[test]
fn prompt_protocol_is_fragment_safe_native_and_preserves_unrelated_output() {
    let mut decoder = PromptDecoder::new("secret");
    let bytes = b"before\x1b]777;rc;wrong;0;2f\x07\x1b]777;rc;secret;9;7;2f726177ff0a\x07after";
    let mut output = Vec::new();
    let mut prompts = Vec::new();
    for byte in bytes {
        for event in decoder.feed(&[*byte]) {
            match event {
                TransportEvent::Event(SubshellEvent::Output(bytes)) => output.extend(bytes),
                TransportEvent::Event(SubshellEvent::Prompt(prompt)) => prompts.push(prompt),
                _ => unreachable!(),
            }
        }
    }
    assert_eq!(output, b"before\x1b]777;rc;wrong;0;2f\x07after");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].exit_status, 7);
    assert_eq!(prompts[0].command_id, 9);
    assert_eq!(prompts[0].directory.as_os_str().as_bytes(), b"/raw\xff\n");
    assert!(matches!(
        decoder.feed(b"\x1b]777;rc;secret;r9;258;2f\x07").as_slice(),
        [TransportEvent::Redraw(ShellPrompt {
            command_id: 9,
            exit_status: 130,
            ..
        })]
    ));
    assert_eq!(
        decoder.feed(b"\x1b]777;rc;secret;s10\x07"),
        vec![TransportEvent::Started(10)]
    );
    for invalid in ["0;0;00", "1;999;2f", "2;0;61", "3;0;xyz", "x;0;2f"] {
        let frame = format!("\x1b]777;rc;secret;{invalid}\x07");
        assert_eq!(
            decoder.feed(frame.as_bytes()),
            vec![SubshellEvent::Output(frame.into_bytes()).into()]
        );
    }
    let oversized = format!("\x1b]777;rc;secret;{}", "0".repeat(PROTOCOL_LIMIT * 2));
    let output = decoder
        .feed(oversized.as_bytes())
        .into_iter()
        .flat_map(|event| match event {
            TransportEvent::Event(SubshellEvent::Output(bytes)) => bytes,
            _ => panic!("invalid frame"),
        })
        .collect::<Vec<_>>();
    assert_eq!(output, oversized.as_bytes());
    assert!(decoder.pending.is_empty());
}

fn answer_queries(shell: &mut PersistentShell, query_tail: &mut Vec<u8>, bytes: &[u8]) {
    // A VT100 frontend answers device attributes before Fish's first prompt.
    query_tail.extend_from_slice(bytes);
    if query_tail.windows(4).any(|bytes| bytes == b"\x1b[0c") {
        shell.write_response(b"\x1b[?1;2c").unwrap();
    }
    if query_tail.windows(4).any(|bytes| bytes == b"\x1b[6n") {
        shell.write_response(b"\x1b[1;1R").unwrap();
    }
    if query_tail.windows(4).any(|bytes| bytes == b"\x1b[?u") {
        shell.write_response(b"\x1b[?0u").unwrap();
    }
    query_tail.drain(..query_tail.len().saturating_sub(3));
}

fn wait_prompt(shell: &mut PersistentShell, query_tail: &mut Vec<u8>) -> (ShellPrompt, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while Instant::now() < deadline {
        let mut prompt = None;
        for event in shell.poll() {
            match event {
                SubshellEvent::Output(bytes) => {
                    answer_queries(shell, query_tail, &bytes);
                    output.extend(bytes);
                }
                SubshellEvent::Prompt(event) => prompt = Some(event),
                SubshellEvent::CommandStarted(_) => {}
                event => panic!(
                    "unexpected shell event {event:?}; output {}",
                    String::from_utf8_lossy(&output)
                ),
            }
        }
        if let Some(prompt) = prompt {
            return (prompt, output);
        }
        thread::sleep(IO_TICK);
    }
    panic!(
        "no prompt; previous {:?}; output {}",
        shell.last_prompt(),
        String::from_utf8_lossy(&output)
    );
}

fn wait_output(shell: &mut PersistentShell, query_tail: &mut Vec<u8>, expected: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while !output.windows(expected.len()).any(|part| part == expected) {
        assert!(Instant::now() < deadline, "output missing: {output:?}");
        for event in shell.poll() {
            if let SubshellEvent::Output(bytes) = event {
                answer_queries(shell, query_tail, &bytes);
                output.extend(bytes);
            }
        }
        thread::sleep(IO_TICK);
    }
}

#[test]
fn persistent_shells_keep_state_report_cwd_and_accept_native_arguments() {
    let root_files = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(root_files.path()).unwrap();
    let nested = root.join("directory ' with\nnewline");
    fs::create_dir(&nested).unwrap();
    for name in ["sh", "bash", "zsh", "fish", "dash"] {
        let Some(program) = crate::find_program_on_path(OsStr::new(name), &root) else {
            continue;
        };
        eprintln!("testing persistent {name}");
        let dialect = if name == "fish" {
            ShellDialect::Fish
        } else {
            ShellDialect::Posix
        };
        let resolved = ResolvedShell {
            program,
            dialect,
            argument_template: vec!["-c".into(), "{command}".into()],
            identity: name.into(),
        };
        let mut shell = PersistentShell::spawn(
            &resolved,
            &root,
            SubshellOptions {
                read_startup_files: false,
                ..Default::default()
            },
        )
        .unwrap();
        let mut query_tail = Vec::new();
        let (prompt, _) = wait_prompt(&mut shell, &mut query_tail);
        assert_eq!(prompt.directory, root);
        shell
            .run_command(if dialect == ShellDialect::Fish {
                "set rc_test_value remembered"
            } else {
                "rc_test_value=remembered"
            })
            .unwrap();
        wait_prompt(&mut shell, &mut query_tail);
        shell
            .run_command("printf 'VALUE:%s:END\\n' \"$rc_test_value\"; false")
            .unwrap();
        let (prompt, output) = wait_prompt(&mut shell, &mut query_tail);
        assert_eq!(prompt.exit_status, 1, "{name}");
        assert!(
            String::from_utf8_lossy(&output).contains("VALUE:remembered:END"),
            "{name}: {}",
            String::from_utf8_lossy(&output)
        );
        shell.synchronize_directory(&nested).unwrap();
        let (prompt, _) = wait_prompt(&mut shell, &mut query_tail);
        assert_eq!(prompt.exit_status, 0, "{name}");
        assert_eq!(prompt.directory, nested, "{name}");
        let value = OsStr::from_bytes(b"raw\xff\n\n");
        shell
            .run_command(&format!(
                "printf 'NATIVE:%s:END' {} {}",
                crate::quote_native_argument(value, dialect),
                crate::quote_native_argument(OsStr::from_bytes(b"other\xfe"), dialect)
            ))
            .unwrap();
        let (_, output) = wait_prompt(&mut shell, &mut query_tail);
        assert!(
            output
                .windows(b"NATIVE:raw\xff\r\n\r\n:END".len())
                .any(|part| part == b"NATIVE:raw\xff\r\n\r\n:END"),
            "{name}: {output:?}"
        );
        shell.run_command("env | grep '^RC_NATIVE_HEX_'").unwrap();
        let (_, output) = wait_prompt(&mut shell, &mut query_tail);
        assert!(
            !output.windows(14).any(|part| part == b"RC_NATIVE_HEX_"),
            "{name}: leaked native environment"
        );
        shell.write(b"unfinished").unwrap();
        assert_eq!(
            shell.run_command("true").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        wait_output(&mut shell, &mut query_tail, b"unfinished");
        shell.interrupt().unwrap();
        wait_prompt(&mut shell, &mut query_tail);
        shell.resize(30, 100).unwrap();
        shell.run_command("stty size").unwrap();
        let (_, output) = wait_prompt(&mut shell, &mut query_tail);
        assert!(
            String::from_utf8_lossy(&output).contains("30 100"),
            "{name}: {}",
            String::from_utf8_lossy(&output)
        );
        shell.run_command("echo 'unterminated").unwrap();
        let (prompt, _) = wait_prompt(&mut shell, &mut query_tail);
        assert_ne!(prompt.exit_status, 0, "{name}: syntax error must fail");
        shell.run_command("printf 'RECOVERED'").unwrap();
        let (_, output) = wait_prompt(&mut shell, &mut query_tail);
        assert!(output.windows(9).any(|part| part == b"RECOVERED"), "{name}");
        for cycle in 0..8 {
            shell
                .run_command(if cycle % 2 == 0 {
                    "sleep 30"
                } else {
                    "printf unexpected > canceled-command"
                })
                .unwrap();
            shell.interrupt().unwrap();
            let (prompt, _) = wait_prompt(&mut shell, &mut query_tail);
            assert_eq!(prompt.exit_status, 130, "{name}");
            assert!(
                !nested.join("canceled-command").exists(),
                "{name}: canceled command executed"
            );
            shell.run_command("printf 'CANCEL_RECOVERY'").unwrap();
            let (_, output) = wait_prompt(&mut shell, &mut query_tail);
            assert!(
                output.windows(15).any(|part| part == b"CANCEL_RECOVERY"),
                "{name}"
            );
        }
        // Queue a resize redraw before raw input. A builtin read stays in the
        // shell process group, so group ownership alone cannot prove readiness.
        shell.resize(32, 102).unwrap();
        thread::sleep(Duration::from_millis(50));
        shell.write(b"read rc_test_input\r").unwrap();
        let deadline = Instant::now() + Duration::from_millis(150);
        while Instant::now() < deadline {
            for event in shell.poll() {
                if let SubshellEvent::Output(bytes) = event {
                    answer_queries(&mut shell, &mut query_tail, &bytes);
                }
            }
            assert!(
                !shell.at_prompt(),
                "{name}: redraw marked builtin read ready"
            );
            thread::sleep(IO_TICK);
        }
        shell.interrupt().unwrap();
        wait_prompt(&mut shell, &mut query_tail);
        shell.run_command("sh -c 'stty -icanon -echo; printf ONE_CHAR; dd bs=1 count=1 2>/dev/null; stty icanon echo'").unwrap();
        wait_output(&mut shell, &mut query_tail, b"ONE_CHAR");
        shell.write(b"q").unwrap();
        wait_prompt(&mut shell, &mut query_tail);
        assert!(
            !shell.at_prompt(),
            "{name}: unacknowledged raw input must prevent command injection"
        );
        shell.interrupt().unwrap();
        wait_prompt(&mut shell, &mut query_tail);
        assert!(
            shell.at_prompt(),
            "{name}: Ctrl+C recovers after program input"
        );
        shell.run_command("printf TYPEAHEAD; sleep 0.1").unwrap();
        wait_output(&mut shell, &mut query_tail, b"TYPEAHEAD");
        shell.write(b"printf DRAFT").unwrap();
        wait_prompt(&mut shell, &mut query_tail);
        assert_eq!(
            shell.run_command("true").unwrap_err().kind(),
            io::ErrorKind::WouldBlock,
            "{name}: typeahead must block private command injection"
        );
        shell.interrupt().unwrap();
        wait_prompt(&mut shell, &mut query_tail);
        shell.run_command("printf 'RUNNING'; sleep 30").unwrap();
        wait_output(&mut shell, &mut query_tail, b"RUNNING");
        shell.interrupt().unwrap();
        let (prompt, _) = wait_prompt(&mut shell, &mut query_tail);
        assert_eq!(
            prompt.exit_status, 130,
            "{name}: running command cancellation"
        );
        shell.run_command("printf 'SHUTDOWN'; sleep 30").unwrap();
        wait_output(&mut shell, &mut query_tail, b"SHUTDOWN");
        let pid = shell.process_id().unwrap();
        let started = Instant::now();
        shell.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{name}: slow shutdown"
        );
        assert_eq!(
            nix::sys::signal::kill(Pid::from_raw(pid as i32), None),
            Err(nix::errno::Errno::ESRCH)
        );
    }
}

use std::fs;
