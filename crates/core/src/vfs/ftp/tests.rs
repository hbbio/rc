use super::*;
mod server;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

fn curl() -> Option<PathBuf> {
    rc_shell::find_program_on_path(OsStr::new("curl"), Path::new("/"))
}
fn file(mount: &Arc<Mount>) -> VfsPath {
    mount
        .root()
        .read_directory(crate::SortMode::default(), true, None)
        .unwrap()
        .into_iter()
        .find(|entry| !entry.is_parent())
        .map(|entry| mount.root().resolve_identity(entry.path).unwrap())
        .unwrap()
}

#[test]
fn real_curl_lists_and_streams_native_names_and_requires_tls_before_login() {
    let Some(curl) = curl() else {
        return;
    };
    let server = server::Server::start(false);
    let dir = tempfile::tempdir().unwrap();
    let connection =
        Connection::parse(&format!("ftp://{}", server.address), "/data".into()).unwrap();
    let mount = open_with_program(
        &connection,
        VfsPath::Local(dir.path().into()),
        None,
        curl.clone(),
        None,
    )
    .unwrap();
    let file = file(&mount);
    assert_eq!(
        file.identity().file_name().unwrap().as_bytes(),
        b" \xff file;type=a"
    );
    let mut bytes = Vec::new();
    file.with_file(None, &mut |_, input| {
        for _ in 0..128 {
            let mut one = [0];
            input.read_exact(&mut one)?;
            bytes.push(one[0]);
        }
        input.read_to_end(&mut bytes)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(bytes, server::CONTENT);
    mount.invalidate();
    assert!(file.metadata(None).is_ok());
    let commands = server.commands.lock().unwrap();
    assert!(
        commands
            .iter()
            .any(|command| command == b"RETR  \xff file;type=a")
    );
    assert!(
        commands
            .iter()
            .filter(|command| command.starts_with(b"USER "))
            .all(|command| command == b"USER anonymous")
    );
    drop(commands);
    server.commands.lock().unwrap().clear();
    let tls = Connection::parse(&server.address, ".".into()).unwrap();
    assert!(open_with_program(&tls, VfsPath::Local(dir.path().into()), None, curl, None).is_err());
    let commands = server.commands.lock().unwrap();
    assert!(commands.iter().any(|command| command.starts_with(b"AUTH ")));
    assert!(
        !commands
            .iter()
            .any(|command| command.starts_with(b"USER ") || command.starts_with(b"PASS "))
    );
}

#[test]
fn truncated_ftp_payloads_fail_the_read() {
    let Some(curl) = curl() else {
        return;
    };
    let server = server::Server::truncated();
    let dir = tempfile::tempdir().unwrap();
    let connection = Connection::parse(&format!("ftp://{}", server.address), ".".into()).unwrap();
    let mount = open_with_program(
        &connection,
        VfsPath::Local(dir.path().into()),
        None,
        curl,
        None,
    )
    .unwrap();
    assert!(
        file(&mount)
            .with_file(None, &mut |_, input| {
                io::copy(input, &mut io::sink())?;
                Ok(())
            })
            .is_err()
    );
}

#[test]
fn canceled_ftp_download_stops_the_owned_process() {
    let Some(curl) = curl() else {
        return;
    };
    let server = server::Server::start(true);
    let dir = tempfile::tempdir().unwrap();
    let connection = Connection::parse(&format!("ftp://{}", server.address), ".".into()).unwrap();
    let mount = open_with_program(
        &connection,
        VfsPath::Local(dir.path().into()),
        None,
        curl,
        None,
    )
    .unwrap();
    let file = file(&mount);
    let canceled = AtomicBool::new(false);
    let start = Instant::now();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            canceled.store(true, std::sync::atomic::Ordering::Release);
        });
        let error = file
            .with_file(Some(&canceled), &mut |_, input| {
                io::copy(input, &mut io::sink())?;
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    });
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn netrc_must_be_private_and_is_only_supplied_through_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let netrc = dir.path().join("credentials");
    let content = b"machine host login user password test-secret\n";
    fs::write(&netrc, content).unwrap();
    fs::set_permissions(&netrc, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(credentials(Some(&netrc)).is_err());
    fs::set_permissions(&netrc, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(credentials(Some(&netrc)).unwrap().0, content);
    let alias = dir.path().join("alias");
    symlink(&netrc, &alias).unwrap();
    assert!(credentials(Some(&alias)).is_err());
    let program = dir.path().join("curl");
    fs::write(&program, "#!/bin/sh\nset -eu\ncd \"$(dirname \"$0\")\"\nprintf '%s\\n' \"$@\" > arguments\ncat > received\nprintf 'type=file;size=0; empty\\r\\n'\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let connection = Connection::parse("user@host", ".".into()).unwrap();
    let mount = open_with_program(
        &connection,
        VfsPath::Local(dir.path().into()),
        None,
        program.clone(),
        Some(netrc.clone()),
    )
    .unwrap();
    assert_eq!(fs::read(dir.path().join("received")).unwrap(), content);
    let arguments = fs::read_to_string(dir.path().join("arguments")).unwrap();
    assert!(!arguments.contains("test-secret"));
    assert!(arguments.contains("--netrc-file\n/dev/stdin\n"));
    assert!(arguments.contains("--ssl-reqd\n"));
    assert!(arguments.starts_with("--disable\n"));
    mount.close();
    fs::write(
        &program,
        "#!/bin/sh\ncat >/dev/null\nprintf 'test-secret' >&2\nexit 67\n",
    )
    .unwrap();
    let error = open_with_program(
        &connection,
        VfsPath::Local(dir.path().into()),
        None,
        program,
        Some(netrc),
    )
    .unwrap_err();
    assert!(!error.to_string().contains("test-secret"));
}

#[test]
fn urls_preserve_native_bytes_and_plain_ftp_never_accepts_passwords() {
    let connection = Connection::parse("host:21", PathBuf::from("/space dir")).unwrap();
    assert_eq!(
        connection
            .url(Path::new(OsStr::from_bytes(b"\xff;type=a")), false)
            .unwrap(),
        "ftp://host:21/%2Fspace%20dir/%FF%3Btype%3Da"
    );
    assert!(Connection::parse("ftp://user@host", ".".into()).is_err());
    assert!(Connection::parse("user:password@host", ".".into()).is_err());
    assert!(Connection::parse("host", "bad\r\nQUIT".into()).is_err());
    assert!(Connection::parse("ftps://user@[::1]:990", ".".into()).is_ok());
}

#[test]
fn mlsd_parses_native_names_and_rejects_aliases_and_malformed_facts() {
    let cancel = Cancellation {
        requested: None,
        closed: None,
    };
    let entries = listing::parse(Path::new("folder"), b"type=cdir; .\r\ntype=pdir; ..\r\ntype=file;size=4;modify=20261029123456.123;unix.mode=0644;unix.uid=42;  \xff name\r\ntype=dir; sub\r\ntype=OS.unix=slink; link\r\n", &cancel).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].path.as_os_str().as_bytes(), b"folder/ \xff name");
    assert_eq!(entries[0].metadata.mode, Some(0o100644));
    assert_eq!(entries[0].metadata.user_id, Some(42));
    assert!(entries[1].is_dir());
    assert_eq!(entries[2].metadata.symlink_target, Some(false));
    for bytes in [
        b"type=file;size=4; a/b\r\n".as_slice(),
        b"type=file;size=4; a/.\r\n",
        b"type=file;size=4; ../x\r\n",
        b"type=file;size=4; a",
        b"type=file;size=4; a\0b\r\n",
        b"type=file; a\r\n",
        b"type=file;size=1;size=2; a\r\n",
        b"type=file;size=1;modify=bad; a\r\n",
        b"type=file;size=1; name\n\n",
        b"\n",
    ] {
        assert!(
            listing::parse(Path::new(""), bytes, &cancel).is_err(),
            "{bytes:?}"
        );
    }
}
