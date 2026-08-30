# Roadmap

This file contains only unfinished product work. Supported behavior is documented
in the project [README](README.md).

## Extension rules and user menus

- Parse MC `mc.ext.ini` files, including ordered rules, `Include=`
  composition, and Open/View/Edit actions.
- Expand MC action macros safely and route command output to the internal viewer
  when requested.
- Parse `mc.menu`, evaluate its conditions, and implement the F2 user menu.
- Support menu macros for current and passive panels, tagged files, quoting modes,
  interactive prompts, and `%view{...}` output.
- Provide commands for opening the effective extension, menu, and highlighting
  configuration files in the external editor.

## File-manager workflows

- Add chmod, chown, advanced chown, hard-link, symlink, relative-symlink, and
  edit-symlink operations with tagged-file support.
- Add select-group and unselect-group dialogs and filtered-view command handling.
- Add explicit directory and file comparison commands.
- Add panel swapping, directory-size calculation on demand, viewed/edited-file
  history, and a screen selector.
- Support user-defined panel listing formats in addition to Full, Brief, and Long.

## Command output, diff, and subshell

- Add a reusable output viewer that streams stdout and stderr, supports search and
  paging, and records command completion.
- Run file comparisons through `difftastic` when available, with `diff` as the
  fallback, and display their output in that viewer.
- Support captured and background command execution without blocking the
  interface.
- Add a persistent PTY-backed shell with a prompt protocol and bidirectional
  working-directory synchronization.
- Implement `Ctrl-O` panel hiding around the persistent shell.
- Bring the modal command line, completion, and shell configuration to Windows.

## Virtual filesystems and archives

- Introduce a path and backend model that can represent local, archive, helper, and
  remote locations without weakening local-filesystem behavior.
- Browse TAR and ZIP archives as directories and support nested archive paths.
- Support MC-style extfs helpers for additional archive and package formats.
- Add optional SFTP, FTP, and shell links with secure credential handling, host
  verification, cancellation, and reconnect behavior.
- Add an active-VFS screen for inspecting and closing mounted connections.

## Portability and configuration fidelity

- Preserve non-UTF-8 filenames losslessly throughout panels, jobs, search,
  command insertion, and external-tool invocation.
- Add optional per-panel legacy filename encodings.
- Complete platform-specific path, permissions, ownership, and terminal behavior
  on Windows.
- Turn Learn Keys capture into editable, persistent keymap overrides.
