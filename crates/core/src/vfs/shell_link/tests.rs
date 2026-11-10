use super::*;
use crate::{
    SortMode,
    vfs::{BackendKind, Cancellation, process},
};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};

fn python() -> Option<std::path::PathBuf> {
    rc_shell::find_program_on_path(OsStr::new("python3"), Path::new("/"))
}

fn fixture(directory: &Path) -> Option<Arc<Mount>> {
    python()?;
    let program = directory.join("fake-ssh");
    fs::write(&program, "#!/bin/sh\nset -eu\ncd \"$(dirname \"$0\")\"\nfor argument do last=$argument; done\nexec /bin/sh -c \"$last\"\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    Some(
        sftp::open_with_program(
            &sftp::Connection {
                endpoint: Endpoint::parse("user@example.test").unwrap(),
                directory: directory.into(),
            },
            VfsPath::Local(directory.into()),
            None,
            program,
            sftp::Transport::Shell,
        )
        .unwrap(),
    )
}

fn member(mount: &Arc<Mount>, name: &str) -> VfsPath {
    mount
        .root()
        .resolve_identity(mount.root().identity().join(name))
        .unwrap()
}

fn read(path: &VfsPath) -> io::Result<Vec<u8>> {
    let mut result = Vec::new();
    path.with_file(None, &mut |_, input| {
        input.read_to_end(&mut result)?;
        Ok(())
    })?;
    Ok(result)
}

#[test]
fn shell_helper_streams_native_paths_without_evaluating_names_and_reconnects() {
    let dir = tempfile::tempdir().unwrap();
    let name = " space\n';$(touch INJECTED)'\t\\file";
    let data: Vec<_> = (0..180_000).map(|n| (n % 251) as u8).collect();
    fs::write(dir.path().join(name), &data).unwrap();
    fs::create_dir(dir.path().join("folder")).unwrap();
    for n in 0..37 {
        fs::write(dir.path().join(format!("folder/{n}")), []).unwrap();
    }
    symlink(name, dir.path().join("link")).unwrap();
    let Some(mount) = fixture(dir.path()) else {
        return;
    };
    assert_eq!(mount.kind(), BackendKind::Shell);
    assert!(
        mount
            .root()
            .identity()
            .as_os_str()
            .as_bytes()
            .starts_with(b"sh://")
    );
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
    assert_eq!(
        member(&mount, "folder")
            .read_directory(SortMode::default(), true, None)
            .unwrap()
            .len(),
        38
    );
    assert!(read(&member(&mount, "link")).is_err());
    assert_eq!(read(&member(&mount, name)).unwrap(), data);
    assert!(!dir.path().join("INJECTED").exists());
    mount.invalidate();
    assert_eq!(read(&member(&mount, name)).unwrap(), data);
    mount.close();
    assert!(read(&member(&mount, name)).is_err());
}

#[test]
fn shell_identity_detects_same_size_rewrites_even_with_restored_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::write(&source, b"before").unwrap();
    let original_mtime =
        filetime::FileTime::from_last_modification_time(&fs::metadata(&source).unwrap());
    let Some(mount) = fixture(dir.path()) else {
        return;
    };
    let error = member(&mount, "source")
        .with_file(None, &mut |_, reader| {
            assert_eq!(io::copy(reader, &mut io::sink())?, 6);
            fs::write(&source, b"after!")?;
            filetime::set_file_mtime(&source, original_mtime)?;
            Ok(())
        })
        .unwrap_err();
    assert!(error.to_string().contains("changed"), "{error}");
}

#[test]
fn helper_rejects_write_requests_and_cleans_up_at_eof() {
    let Some(python) = python() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("new");
    fs::write(&source, b"untouched").unwrap();
    let mut requests = Vec::new();
    fn packet(target: &mut Vec<u8>, data: &[u8]) {
        target.extend_from_slice(&(data.len() as u32).to_be_bytes());
        target.extend_from_slice(data);
    }
    fn path(target: &mut Vec<u8>, path: &Path) {
        let bytes = path.as_os_str().as_bytes();
        target.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        target.extend_from_slice(bytes);
    }
    packet(&mut requests, &[1, 0, 0, 0, 3]);
    let mut open = vec![3, 0, 0, 0, 1];
    path(&mut open, &target);
    open.extend_from_slice(&10_u32.to_be_bytes());
    open.extend_from_slice(&0_u32.to_be_bytes());
    packet(&mut requests, &open);
    let mut delete = vec![13, 0, 0, 0, 2];
    path(&mut delete, &source);
    packet(&mut requests, &delete);
    let mut rename = vec![18, 0, 0, 0, 3];
    path(&mut rename, &source);
    path(&mut rename, &target);
    packet(&mut requests, &rename);
    let mut command = Command::new(python);
    command.args(["-I", "-u", "-c", SCRIPT]);
    let output = process::run(
        command,
        &requests,
        4096,
        std::time::Duration::from_secs(5),
        &Cancellation {
            requested: None,
            closed: None,
        },
    )
    .unwrap();
    let output = process::ensure_success(output).unwrap();
    let mut rest = output.as_slice();
    let mut statuses = Vec::new();
    while !rest.is_empty() {
        let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        let packet = &rest[4..4 + len];
        if packet[0] == 101 {
            statuses.push(u32::from_be_bytes(packet[5..9].try_into().unwrap()));
        }
        rest = &rest[4 + len..];
    }
    assert_eq!(statuses, [3, 8, 8]);
    assert_eq!(fs::read(source).unwrap(), b"untouched");
    assert!(!target.exists());
}
