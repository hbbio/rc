use super::*;
use crate::SortMode;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::{Duration, Instant};

fn server(directory: &Path) -> Option<PathBuf> {
    let executable = [
        "/usr/libexec/sftp-server",
        "/usr/lib/openssh/sftp-server",
        "/usr/lib/ssh/sftp-server",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())?;
    let program = directory.join("ssh-fixture");
    fs::write(&program, format!("#!/bin/sh\nexec {executable} -R\n")).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    Some(program)
}

fn mount(directory: &Path, program: PathBuf) -> Arc<Mount> {
    open_with_program(
        &Connection {
            endpoint: Endpoint::parse("tester@example.test:2222").unwrap(),
            directory: directory.into(),
        },
        VfsPath::Local(directory.into()),
        None,
        program,
    )
    .unwrap()
}

fn member(mount: &Arc<Mount>, name: &str) -> VfsPath {
    mount
        .root()
        .resolve_identity(mount.root().identity().join(name))
        .unwrap()
}

fn read(path: &VfsPath) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    path.with_file(None, &mut |_, reader| {
        reader.read_to_end(&mut bytes)?;
        Ok(())
    })?;
    Ok(bytes)
}

#[test]
fn openssh_server_lists_streams_native_names_and_rejects_links() {
    let dir = tempfile::tempdir().unwrap();
    let Some(program) = server(dir.path()) else {
        return;
    };
    let name = " space\n' ;$.bin";
    let data: Vec<_> = (0..200_000).map(|n| (n % 251) as u8).collect();
    fs::write(dir.path().join(name), &data).unwrap();
    fs::create_dir(dir.path().join("folder")).unwrap();
    fs::write(dir.path().join("folder/empty"), []).unwrap();
    symlink(name, dir.path().join("link")).unwrap();
    let mount = mount(dir.path(), program);
    assert_eq!(mount.kind(), BackendKind::Sftp);
    let entries = mount
        .root()
        .read_directory(SortMode::default(), true, None)
        .unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.path.file_name() == Some(OsStr::new(name)))
    );
    assert_eq!(read(&member(&mount, name)).unwrap(), data);
    assert!(read(&member(&mount, "folder/empty")).unwrap().is_empty());
    assert!(
        read(&member(&mount, "link"))
            .unwrap_err()
            .to_string()
            .contains("special")
    );
    assert_eq!(
        read(&member(&mount, name)).unwrap(),
        data,
        "an error discards the connection and the next read reconnects"
    );
    mount.close();
    assert!(read(&member(&mount, name)).is_err());
}

#[test]
fn reread_discards_metadata_and_changed_sources_fail() {
    let dir = tempfile::tempdir().unwrap();
    let Some(program) = server(dir.path()) else {
        return;
    };
    fs::write(dir.path().join("data"), b"before").unwrap();
    let mount = mount(dir.path(), program);
    let file = member(&mount, "data");
    assert_eq!(file.metadata(None).unwrap().size, 6);
    fs::write(dir.path().join("data"), b"after").unwrap();
    mount.invalidate();
    assert_eq!(file.metadata(None).unwrap().size, 5);
    let error = file
        .with_file(None, &mut |_, reader| {
            assert_eq!(io::copy(reader, &mut io::sink())?, 5);
            fs::write(dir.path().join("data"), b"changed length")?;
            Ok(())
        })
        .unwrap_err();
    assert!(error.to_string().contains("changed"), "{error}");
    assert_eq!(read(&file).unwrap(), b"changed length");
}

#[test]
fn cancellation_interrupts_handshake_and_reaps_transport() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("stall");
    fs::write(&program, "#!/bin/sh\nexec sleep 30\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let canceled = AtomicBool::new(false);
    let started = Instant::now();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(80));
            canceled.store(true, Ordering::Release);
        });
        let error = open_with_program(
            &Connection {
                endpoint: Endpoint::parse("example.test").unwrap(),
                directory: ".".into(),
            },
            VfsPath::Local(dir.path().into()),
            Some(&canceled),
            program,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    });
    assert!(started.elapsed() < Duration::from_secs(3));
}
