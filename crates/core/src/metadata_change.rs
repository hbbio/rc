use crate::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionChange(String);

impl PermissionChange {
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.is_empty() || value.len() > 256 {
            return Err("Enter an octal mode or clauses such as u+rw,g-w,o=r".into());
        }
        if value.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            if value.len() <= 4 && u32::from_str_radix(value, 8).is_ok_and(|v| v <= 0o7777) {
                return Ok(Self(value.into()));
            }
            return Err("Octal mode must be 0000–7777".into());
        }
        for clause in value.split(',') {
            let Some(op) = clause.find(['+', '-', '=']) else {
                return Err("Use u/g/o/a followed by +, -, or = and rwxXst".into());
            };
            let (who, rest) = clause.split_at(op);
            if who.is_empty()
                || !who.chars().all(|c| "ugoa".contains(c))
                || !rest[1..].chars().all(|c| "rwxXst".contains(c))
            {
                return Err(
                    "Use explicit u/g/o/a classes and rwxXst permissions (example: u+rw,g-w,o=r)"
                        .into(),
                );
            }
            if rest.len() == 1 && !rest.starts_with('=') {
                return Err("A + or - clause needs permissions".into());
            }
        }
        Ok(Self(value.into()))
    }

    pub fn apply(&self, original: u32, directory: bool) -> u32 {
        if let Ok(mode) = u32::from_str_radix(&self.0, 8) {
            return mode;
        }
        let mut mode = original & 0o7777;
        for clause in self.0.split(',') {
            let op = clause.find(['+', '-', '=']).expect("validated clause");
            let who = &clause[..op];
            let user = who.contains('u') || who.contains('a');
            let group = who.contains('g') || who.contains('a');
            let other = who.contains('o') || who.contains('a');
            let mask = if user { 0o4700 } else { 0 }
                | if group { 0o2070 } else { 0 }
                | if other { 0o1007 } else { 0 };
            let mut bits = 0;
            for c in clause[op + 1..].chars() {
                bits |= match c {
                    'r' => 0o444,
                    'w' => 0o222,
                    'x' => 0o111,
                    'X' if directory || original & 0o111 != 0 => 0o111,
                    's' => 0o6000,
                    't' => 0o1000,
                    _ => 0,
                } & mask;
            }
            mode = match clause.as_bytes()[op] {
                b'+' => mode | bits,
                b'-' => mode & !bits,
                _ => (mode & !mask) | bits,
            };
        }
        mode
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataChange {
    pub permissions: Option<PermissionChange>,
    pub owner: Option<String>,
    pub group: Option<String>,
}

impl MetadataChange {
    pub fn new(mode: &str, owner: &str, group: &str) -> Result<Self, String> {
        let parse_identity = |value: &str| -> Result<Option<String>, String> {
            let value = value.trim();
            if value.is_empty() || value == "-" {
                return Ok(None);
            }
            if value.len() > 256
                || value.contains([':', '\0', '\n', '\r'])
                || value.starts_with('-')
            {
                return Err(
                    "Enter an account name or numeric ID; blank or - keeps the current value"
                        .into(),
                );
            }
            if value.bytes().all(|c| c.is_ascii_digit())
                && value
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n != u32::MAX)
                    .is_none()
            {
                return Err("Numeric account ID is out of range".into());
            }
            Ok(Some(value.into()))
        };
        let change = Self {
            permissions: if mode.trim().is_empty() {
                None
            } else {
                Some(PermissionChange::parse(mode)?)
            },
            owner: parse_identity(owner)?,
            group: parse_identity(group)?,
        };
        if change.permissions.is_none() && change.owner.is_none() && change.group.is_none() {
            return Err("Specify at least one change".into());
        }
        Ok(change)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum MetadataAction {
    Chmod,
    Chown,
    Advanced,
}

impl AppState {
    pub(crate) fn open_metadata_dialog(&mut self, action: MetadataAction) {
        if !cfg!(unix) {
            self.set_status("Permissions and ownership operations require Unix");
            return;
        }
        if self.active_panel().is_virtual() {
            self.set_status("Archives are read-only");
            return;
        }
        if self.active_panel().loading {
            self.set_status("Wait for the panel to finish loading");
            return;
        }
        let paths = self.selected_operation_paths();
        if paths.is_empty() {
            self.set_status("Select an entry or tag entries first");
            return;
        }
        let first = if matches!(action, MetadataAction::Chmod) && paths.len() == 1 {
            self.active_panel()
                .entries
                .iter()
                .find(|entry| entry.path == paths[0])
                .and_then(|e| e.metadata.mode)
                .map_or_else(String::new, |mode| format!("{:04o}", mode & 0o7777))
        } else {
            String::new()
        };
        self.push_metadata_dialog(action, paths, self.active_panel, first, String::new());
    }

    pub(crate) fn push_metadata_dialog(
        &mut self,
        action: MetadataAction,
        paths: Vec<PathBuf>,
        panel: ActivePanel,
        first: String,
        second: String,
    ) {
        let count = paths.len();
        let dialog = match action {
            MetadataAction::Chmod => DialogState::input(
                format!("Chmod ({count} entries)"),
                "Octal mode or u/g/o/a +/-/= rwxXst; no recursion or symlink targets:",
                first,
            ),
            MetadataAction::Chown => DialogState::pair_input(
                format!("Chown ({count} entries)"),
                "Owner name/ID (blank keeps):",
                first,
                "Group name/ID (blank keeps):",
                second,
            ),
            MetadataAction::Advanced => DialogState::pair_input(
                format!("Advanced chown ({count} entries)"),
                "Owner:group (blank fields keep):",
                first,
                "Mode (blank keeps; no recursion):",
                second,
            ),
        };
        self.push_dialog(
            dialog,
            PendingDialogAction::MetadataChange {
                action,
                paths,
                panel,
            },
        );
    }

    pub(crate) fn submit_metadata_dialog(
        &mut self,
        action: MetadataAction,
        paths: Vec<PathBuf>,
        panel: ActivePanel,
        first: String,
        second: String,
    ) {
        let result = match action {
            MetadataAction::Chmod => {
                if first.trim().is_empty() {
                    Err("Enter permissions".into())
                } else {
                    MetadataChange::new(&first, "", "")
                }
            }
            MetadataAction::Chown => MetadataChange::new("", &first, &second),
            MetadataAction::Advanced => {
                let (owner, group) = first.split_once(':').unwrap_or((&first, ""));
                MetadataChange::new(&second, owner, group)
            }
        };
        match result {
            Ok(change) => {
                let id = self.queue_filesystem_job(
                    JobRequest::ChangeMetadata { paths, change },
                    OperationOrigin::Panel(panel),
                );
                self.set_status(format!("Queued permissions/ownership job #{id}"));
            }
            Err(error) => {
                self.push_metadata_dialog(action, paths, panel, first, second);
                self.set_status(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mode_changes_preserve_unselected_bits_and_respect_conditional_execute() {
        assert_eq!(
            PermissionChange::parse("0640").unwrap().apply(0o777, false),
            0o640
        );
        assert_eq!(
            PermissionChange::parse("u+rw,g-w,o=")
                .unwrap()
                .apply(0o127, false),
            0o700
        );
        assert_eq!(
            PermissionChange::parse("a+X").unwrap().apply(0o640, false),
            0o640
        );
        assert_eq!(
            PermissionChange::parse("a+X").unwrap().apply(0o640, true),
            0o751
        );
        assert_eq!(
            PermissionChange::parse("u+s,g+s,o+t")
                .unwrap()
                .apply(0o755, false),
            0o7755
        );
        for bad in ["888", "10000", "u+", "u+z", "+x", "u+r,", "u+r;touch x", ""] {
            assert!(PermissionChange::parse(bad).is_err(), "{bad}");
        }
        assert!(MetadataChange::new("", "4294967295", "").is_err());
        assert!(MetadataChange::new("", "-1", "").is_err());
        assert!(MetadataChange::new("", "", "").is_err());
    }
}
