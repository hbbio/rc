# Roadmap

This file contains only unfinished product work. Supported behavior is documented
in the project [README](README.md).

## Command output, diff, and subshell

- Add a persistent PTY-backed shell with a prompt protocol and bidirectional
  working-directory synchronization.
- Implement `Ctrl-O` panel hiding around the persistent shell.

## Virtual filesystems and archives

- Extend the shared read-only archive backend into a unified path/backend model
  for local, helper, and remote locations without weakening local-filesystem behavior.
- Support nested archive paths and persistent indexes across application sessions.
- Support MC-style extfs helpers for additional archive and package formats.
- Add optional SFTP, FTP, and shell links with secure credential handling, host
  verification, cancellation, and reconnect behavior.
- Add an active-VFS screen for inspecting and closing mounted connections.

## Deferred: Windows (out of current scope)

- Bring the modal command line, completion, and shell configuration to Windows.
- Complete platform-specific path, permissions, ownership, and terminal behavior
  on Windows.
