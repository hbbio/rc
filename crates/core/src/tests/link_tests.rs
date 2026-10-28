#![cfg(unix)]
use super::*;
use std::os::unix::fs::{MetadataExt, symlink};

fn execute(request: JobRequest) -> Result<(), JobError> {
    let mut jobs = JobManager::default();
    let job = jobs.enqueue(request);
    let (tx, rx) = std::sync::mpsc::channel();
    execute_worker_job(job, &tx);
    rx.try_iter()
        .find_map(|event| match event {
            JobEvent::Finished { result, .. } => Some(result),
            _ => None,
        })
        .unwrap()
}

#[test]
fn creates_hard_absolute_and_relative_links_with_literal_names() {
    let root = tempfile::tempdir().unwrap();
    let source_dir = root.path().join("source");
    let dest = root.path().join("output");
    fs::create_dir(&source_dir).unwrap();
    fs::create_dir(&dest).unwrap();
    let source = source_dir.join("a ' ; $file");
    fs::write(&source, "payload").unwrap();
    for (kind, name) in [
        (LinkKind::Hard, "hard"),
        (LinkKind::Symbolic, "absolute"),
        (LinkKind::RelativeSymbolic, "relative"),
    ] {
        let destination = dest.join(name);
        execute(JobRequest::CreateLinks {
            kind,
            sources: vec![source.clone()],
            destination: destination.clone(),
        })
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"payload");
        match kind {
            LinkKind::Hard => assert_eq!(
                fs::metadata(&source).unwrap().ino(),
                fs::metadata(&destination).unwrap().ino()
            ),
            LinkKind::Symbolic => assert_eq!(
                fs::read_link(&destination).unwrap(),
                fs::canonicalize(&source).unwrap()
            ),
            LinkKind::RelativeSymbolic => assert_eq!(
                fs::read_link(&destination).unwrap(),
                PathBuf::from("../source").join(source.file_name().unwrap())
            ),
        }
        assert!(
            execute(JobRequest::CreateLinks {
                kind,
                sources: vec![source.clone()],
                destination: destination.clone()
            })
            .is_err()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"payload");
    }
}

#[test]
fn tagged_link_dialog_uses_passive_directory_and_preserves_original_names() {
    let root = tempfile::tempdir().unwrap();
    let dest = root.path().join("destination");
    fs::create_dir(&dest).unwrap();
    let paths = [root.path().join("first"), root.path().join("second")];
    for path in &paths {
        fs::write(path, "payload").unwrap();
    }
    let mut app = app_with_loaded_panels(root.path().into());
    app.panels[ActivePanel::Right.index()].cwd = dest.clone();
    app.active_panel_mut().tagged.extend(paths.clone());
    app.apply(AppCommand::CreateLink(LinkKind::RelativeSymbolic))
        .unwrap();
    app.finish_dialog(DialogResult::InputSubmitted(".".into()));
    drain_background(&mut app);
    for path in paths {
        assert_eq!(
            fs::read(dest.join(path.file_name().unwrap())).unwrap(),
            b"payload"
        );
    }
}

#[test]
fn batch_preflight_rejects_duplicate_names_and_existing_destinations_before_creating_links() {
    let root = tempfile::tempdir().unwrap();
    for dir in ["a", "b", "dest"] {
        fs::create_dir(root.path().join(dir)).unwrap();
    }
    for name in ["a/same", "b/same", "b/other"] {
        fs::write(root.path().join(name), "data").unwrap();
    }
    let dest = root.path().join("dest");
    assert!(
        execute(JobRequest::CreateLinks {
            kind: LinkKind::Hard,
            sources: vec![root.path().join("a/same"), root.path().join("b/same")],
            destination: dest.clone()
        })
        .is_err()
    );
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    symlink("missing", dest.join("other")).unwrap();
    assert!(
        execute(JobRequest::CreateLinks {
            kind: LinkKind::Symbolic,
            sources: vec![root.path().join("a/same"), root.path().join("b/other")],
            destination: dest.clone()
        })
        .is_err()
    );
    assert!(!dest.join("same").exists());
    assert_eq!(
        fs::read_link(dest.join("other")).unwrap(),
        PathBuf::from("missing")
    );
}

#[test]
fn relative_links_resolve_parent_aliases_and_preserve_dangling_symlink_sources() {
    let root = tempfile::tempdir().unwrap();
    for (kind, name) in [
        (LinkKind::Symbolic, "absolute-root"),
        (LinkKind::RelativeSymbolic, "relative-root"),
    ] {
        let destination = root.path().join(name);
        execute(JobRequest::CreateLinks {
            kind,
            sources: vec![PathBuf::from("/")],
            destination: destination.clone(),
        })
        .unwrap();
        assert_eq!(fs::canonicalize(destination).unwrap(), PathBuf::from("/"));
    }
    let self_link = root.path().join("self-link");
    execute(JobRequest::CreateLinks {
        kind: LinkKind::RelativeSymbolic,
        sources: vec![root.path().to_path_buf()],
        destination: self_link.clone(),
    })
    .unwrap();
    assert_eq!(fs::read_link(self_link).unwrap(), PathBuf::from("."));
    fs::create_dir_all(root.path().join("physical/deep")).unwrap();
    fs::create_dir(root.path().join("output")).unwrap();
    symlink(root.path().join("physical/deep"), root.path().join("alias")).unwrap();
    fs::write(root.path().join("physical/file"), "data").unwrap();
    let source = root.path().join("alias/../file");
    let destination = root.path().join("output/link");
    execute(JobRequest::CreateLinks {
        kind: LinkKind::RelativeSymbolic,
        sources: vec![source],
        destination: destination.clone(),
    })
    .unwrap();
    assert_eq!(fs::read(destination).unwrap(), b"data");
    symlink("missing", root.path().join("dangling")).unwrap();
    execute(JobRequest::CreateLinks {
        kind: LinkKind::RelativeSymbolic,
        sources: vec![root.path().join("dangling")],
        destination: root.path().join("output/dangling"),
    })
    .unwrap();
    assert_eq!(
        fs::read_link(root.path().join("output/dangling")).unwrap(),
        PathBuf::from("../dangling")
    );
    assert!(
        execute(JobRequest::CreateLinks {
            kind: LinkKind::Hard,
            sources: vec![root.path().join("dangling")],
            destination: root.path().join("output/hard")
        })
        .is_err()
    );
}

#[test]
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn edits_tagged_symlinks_including_dangling_links_without_touching_targets() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("target"), "unchanged").unwrap();
    let links = [root.path().join("first"), root.path().join("second")];
    symlink("target", &links[0]).unwrap();
    symlink("missing", &links[1]).unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.active_panel_mut().tagged.extend(links.clone());
    app.apply(AppCommand::EditSymlink).unwrap();
    app.finish_dialog(DialogResult::InputSubmitted(
        "new literal target $HOME".into(),
    ));
    drain_background(&mut app);
    for path in links {
        assert_eq!(
            fs::read_link(path).unwrap(),
            PathBuf::from("new literal target $HOME")
        );
    }
    assert_eq!(fs::read(root.path().join("target")).unwrap(), b"unchanged");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 3);
    assert!(
        execute(JobRequest::EditSymlinks {
            paths: vec![root.path().join("first"), root.path().join("target")],
            target: "other".into()
        })
        .is_err()
    );
    assert_eq!(
        fs::read_link(root.path().join("first")).unwrap(),
        PathBuf::from("new literal target $HOME")
    );
}

#[test]
fn link_shortcuts_dispatch_and_canceling_dialog_queues_no_links() {
    let map = keymap::Keymap::bundled_mc_default().unwrap();
    for (key, expected) in [
        ('l', AppCommand::CreateLink(LinkKind::Hard)),
        ('s', AppCommand::CreateLink(LinkKind::Symbolic)),
        ('v', AppCommand::CreateLink(LinkKind::RelativeSymbolic)),
    ] {
        assert_eq!(
            map.resolve(
                KeyContext::FileManagerXMap,
                keymap::KeyChord::new(keymap::KeyCode::Char(key))
            )
            .and_then(|c| AppCommand::from_key_command(KeyContext::FileManagerXMap, c)),
            Some(expected)
        );
    }
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    fs::write(&file, "data").unwrap();
    let mut app = app_with_loaded_panels(root.path().into());
    app.active_panel_mut().cursor = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.path == file)
        .unwrap();
    app.apply(AppCommand::CreateLink(LinkKind::Hard)).unwrap();
    app.apply(AppCommand::DialogCancel).unwrap();
    assert!(!app.jobs.jobs().iter().any(|j| j.kind == JobKind::Links));
}
