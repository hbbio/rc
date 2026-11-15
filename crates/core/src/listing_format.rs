//! Validated, bounded column descriptions for the user panel format.

pub const DEFAULT_USER_LISTING_FORMAT: &str = "name size:8 mtime:12";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListingField {
    Name,
    Size,
    Modified,
    Permissions,
    Links,
    User,
    Group,
    Inode,
}

impl ListingField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Modified => "Modify time",
            Self::Permissions => "Mode",
            Self::Links => "Links",
            Self::User => "UID",
            Self::Group => "GID",
            Self::Inode => "Inode",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListingColumn {
    pub field: ListingField,
    /// None lets the name consume the remaining space.
    pub width: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserListingFormat {
    pub columns: Vec<ListingColumn>,
}

impl UserListingFormat {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.len() > 1024 {
            return Err(String::from("Listing format is too long"));
        }
        let mut columns = Vec::new();
        for token in value
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
        {
            if columns.len() == 16 {
                return Err(String::from("Use at most 16 columns"));
            }
            let (name, width) = token
                .split_once(':')
                .map_or((token, None), |(n, w)| (n, Some(w)));
            let (field, default_width) = match name {
                "name" => (ListingField::Name, None),
                "size" => (ListingField::Size, Some(8)),
                "mtime" => (ListingField::Modified, Some(12)),
                "perm" | "mode" => (ListingField::Permissions, Some(10)),
                "nlink" => (ListingField::Links, Some(5)),
                "uid" | "owner" => (ListingField::User, Some(6)),
                "gid" | "group" => (ListingField::Group, Some(6)),
                "inode" => (ListingField::Inode, Some(10)),
                _ => return Err(format!("Unknown listing field: {name}")),
            };
            if columns.iter().any(|c: &ListingColumn| c.field == field) {
                return Err(format!("Duplicate listing field: {name}"));
            }
            let width = match width {
                Some(raw) => Some(
                    raw.parse::<u16>()
                        .ok()
                        .filter(|n| (1..=256).contains(n))
                        .ok_or_else(|| format!("Column width must be 1–256: {token}"))?,
                ),
                None => default_width,
            };
            columns.push(ListingColumn { field, width });
        }
        if !columns.iter().any(|c| c.field == ListingField::Name) {
            return Err(String::from("Include a name column"));
        }
        Ok(Self { columns })
    }
}

impl Default for UserListingFormat {
    fn default() -> Self {
        Self::parse(DEFAULT_USER_LISTING_FORMAT).expect("valid default listing format")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_order_widths_and_rejects_unusable_formats() {
        let parsed = UserListingFormat::parse("perm, size:9 name:30").unwrap();
        assert_eq!(parsed.columns[0].field, ListingField::Permissions);
        assert_eq!(parsed.columns[1].width, Some(9));
        assert_eq!(parsed.columns[2].width, Some(30));
        for bad in [
            "",
            "size",
            "name name",
            "name unknown",
            "name:0",
            "name:257",
            "name:-1",
            "name:2:3",
            "name uid owner",
        ] {
            assert!(UserListingFormat::parse(bad).is_err(), "{bad}");
        }
    }
}
