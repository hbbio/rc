# Roadmap

This file contains only unfinished product work. Supported behavior is documented
in the project [README](README.md).

## Virtual filesystems and archives

- Extend the shared read-only archive backend into a unified path/backend model
  for local, helper, and remote locations without weakening local-filesystem behavior.
- Add optional FTP and shell links with secure credential handling, host
  verification, cancellation, and reconnect behavior.

## Deferred: Windows (out of current scope)

- Bring the modal command line, completion, and shell configuration to Windows.
- Complete platform-specific path, permissions, ownership, and terminal behavior
  on Windows.
