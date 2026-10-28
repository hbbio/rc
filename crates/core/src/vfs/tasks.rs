//! Worker requests that retain a mount instead of treating display paths as
//! native files. Native and built-in archive jobs retain their fast paths.
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use super::{MountedPath, VfsPath};
use crate::{
    ActivePanel, BackgroundEvent, JobError, JobEvent, JobKind, JobRequest, OverwritePolicy,
    ViewerState, WorkerJob,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Task {
    #[cfg(unix)]
    OpenShell {
        connection: super::sftp::Connection,
        return_to: VfsPath,
    },
    Reconnect {
        location: MountedPath,
    },
    #[cfg(unix)]
    OpenSftp {
        connection: super::sftp::Connection,
        return_to: VfsPath,
    },
    #[cfg(unix)]
    OpenHelper {
        helper: super::extfs::Helper,
        source: Option<VfsPath>,
        return_to: VfsPath,
    },
    View {
        path: MountedPath,
        quick_view: Option<(ActivePanel, u64)>,
    },
    Measure {
        location: MountedPath,
        paths: Vec<PathBuf>,
        panel: ActivePanel,
        request_id: u64,
    },
    DirectorySizes {
        location: MountedPath,
        paths: Vec<PathBuf>,
    },
    Copy {
        location: MountedPath,
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        overwrite: OverwritePolicy,
    },
}

impl Task {
    pub fn kind(&self) -> JobKind {
        match self {
            Self::Reconnect { .. } => JobKind::RefreshPanel,
            #[cfg(unix)]
            Self::OpenHelper { .. } | Self::OpenSftp { .. } | Self::OpenShell { .. } => {
                JobKind::RefreshPanel
            }
            Self::View { quick_view, .. } => {
                if quick_view.is_some() {
                    JobKind::LoadQuickView
                } else {
                    JobKind::LoadViewer
                }
            }
            Self::Measure { .. } => JobKind::MeasureSelection,
            Self::DirectorySizes { .. } => JobKind::MeasureDirectories,
            Self::Copy { .. } => JobKind::Copy,
        }
    }
    pub fn item_count(&self) -> usize {
        match self {
            Self::Measure { paths, .. } | Self::DirectorySizes { paths, .. } => paths.len(),
            Self::Copy { sources, .. } => sources.len(),
            _ => 1,
        }
    }
    pub fn summary(&self) -> String {
        match self {
            Self::Reconnect { location } => format!("reconnect {}", location.mount().label()),
            #[cfg(unix)]
            Self::OpenHelper { helper, .. } => format!("open extfs {}", helper.name()),
            #[cfg(unix)]
            Self::OpenSftp { connection, .. } => {
                format!("connect SFTP {}", connection.endpoint.authority())
            }
            #[cfg(unix)]
            Self::OpenShell { connection, .. } => {
                format!("connect Shell {}", connection.endpoint.authority())
            }
            Self::View { path, .. } => format!("view {}", path.identity().display()),
            Self::Measure { paths, .. } => format!("measure {} VFS entries", paths.len()),
            Self::DirectorySizes { paths, .. } => {
                format!("measure {} VFS directories", paths.len())
            }
            Self::Copy {
                sources,
                destination_dir,
                ..
            } => format!(
                "copy {} VFS entries to {}",
                sources.len(),
                destination_dir.display()
            ),
        }
    }
    pub fn mount_id(&self) -> Option<u64> {
        match self {
            Self::View { path, .. } => Some(path.mount().id()),
            Self::Measure { location, .. }
            | Self::Reconnect { location }
            | Self::DirectorySizes { location, .. }
            | Self::Copy { location, .. } => Some(location.mount().id()),
            #[cfg(unix)]
            Self::OpenHelper {
                source: Some(VfsPath::Mounted(path)),
                ..
            } => Some(path.mount().id()),
            #[cfg(unix)]
            Self::OpenHelper { .. } | Self::OpenSftp { .. } | Self::OpenShell { .. } => None,
        }
    }
}

pub fn execute(job: WorkerJob, jobs: &Sender<JobEvent>, events: &Sender<BackgroundEvent>) {
    let JobRequest::Vfs(task) = &job.request else {
        return;
    };
    if matches!(task, Task::Copy { .. }) {
        crate::jobs::execute_worker_job(job, jobs);
        return;
    }
    let id = job.id;
    let cancel = job.cancel_flag();
    let _ = jobs.send(JobEvent::Started { id });
    let result = (|| -> io::Result<()> {
        let event = match task {
            #[cfg(unix)]
            Task::OpenShell {
                connection,
                return_to,
            } => BackgroundEvent::VfsMounted {
                job_id: id,
                mount: super::shell_link::open(connection, return_to.clone(), Some(&cancel))?,
            },
            Task::Reconnect { location } => {
                VfsPath::Mounted(location.clone()).read_directory(
                    crate::SortMode::default(),
                    true,
                    Some(&cancel),
                )?;
                return Ok(());
            }
            #[cfg(unix)]
            Task::OpenSftp {
                connection,
                return_to,
            } => BackgroundEvent::VfsMounted {
                job_id: id,
                mount: super::sftp::open(connection, return_to.clone(), Some(&cancel))?,
            },
            #[cfg(unix)]
            Task::OpenHelper {
                helper,
                source,
                return_to,
            } => {
                let mount = super::extfs::open(
                    helper.clone(),
                    source.clone(),
                    return_to.clone(),
                    Some(&cancel),
                )?;
                BackgroundEvent::VfsMounted { job_id: id, mount }
            }
            Task::View { path, quick_view } => {
                let result =
                    ViewerState::open_vfs_cancellable(VfsPath::Mounted(path.clone()), &cancel)
                        .map_err(|error| error.to_string());
                let identity = path.identity();
                let failure = result.as_ref().err().cloned();
                let event = match quick_view {
                    Some((panel, request_id)) => BackgroundEvent::QuickViewLoaded {
                        panel: *panel,
                        path: identity,
                        request_id: *request_id,
                        result,
                    },
                    None => BackgroundEvent::ViewerLoaded {
                        path: identity,
                        result,
                    },
                };
                events
                    .send(event)
                    .map_err(|_| io::Error::other("background event channel disconnected"))?;
                return failure.map_or(Ok(()), |error| Err(io::Error::other(error)));
            }
            Task::Measure {
                location,
                paths,
                panel,
                request_id,
            } => BackgroundEvent::SelectionSizeMeasured {
                panel: *panel,
                request_id: *request_id,
                report: super::walk::measure(&VfsPath::Mounted(location.clone()), paths, &cancel)?,
            },
            Task::DirectorySizes { location, paths } => {
                let mut sizes = Vec::new();
                for path in paths {
                    let report = super::walk::measure(
                        &VfsPath::Mounted(location.clone()),
                        std::slice::from_ref(path),
                        &cancel,
                    )?;
                    sizes.push((path.clone(), report));
                }
                BackgroundEvent::DirectorySizesMeasured { job_id: id, sizes }
            }
            Task::Copy { .. } => unreachable!("filesystem worker"),
        };
        events
            .send(event)
            .map_err(|_| io::Error::other("background event channel disconnected"))
    })();
    let result = if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        Err(JobError::canceled())
    } else {
        result.map_err(JobError::from_io)
    };
    let _ = jobs.send(JobEvent::Finished { id, result });
}
