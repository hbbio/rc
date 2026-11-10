use crate::*;
use std::ffi::OsStr;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FilenameEncoding {
    #[default]
    Utf8,
    Windows1252,
    Windows1251,
    Latin2,
    Koi8R,
    ShiftJis,
    EucJp,
    Gbk,
    Big5,
    EucKr,
    Cp437,
}

impl FilenameEncoding {
    pub const ALL: [Self; 11] = [
        Self::Utf8,
        Self::Windows1252,
        Self::Windows1251,
        Self::Latin2,
        Self::Koi8R,
        Self::ShiftJis,
        Self::EucJp,
        Self::Gbk,
        Self::Big5,
        Self::EucKr,
        Self::Cp437,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Windows1252 => "windows-1252",
            Self::Windows1251 => "windows-1251",
            Self::Latin2 => "ISO-8859-2",
            Self::Koi8R => "KOI8-R",
            Self::ShiftJis => "Shift_JIS",
            Self::EucJp => "EUC-JP",
            Self::Gbk => "GBK",
            Self::Big5 => "Big5",
            Self::EucKr => "EUC-KR",
            Self::Cp437 => "CP437",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|e| e.label().eq_ignore_ascii_case(value.trim()))
    }
    pub fn decode(self, name: &OsStr) -> String {
        if self == Self::Utf8 {
            return rc_shell::encode_path_text(name);
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            if self == Self::Cp437 {
                return name
                    .as_bytes()
                    .iter()
                    .map(|b| {
                        if *b < 128 {
                            char::from(*b)
                        } else {
                            CP437_HIGH[usize::from(*b - 128)]
                        }
                    })
                    .collect();
            }
            let codec = encoding_rs::Encoding::for_label(self.label().as_bytes())
                .expect("known filename encoding");
            codec
                .decode_without_bom_handling(name.as_bytes())
                .0
                .into_owned()
        }
        #[cfg(not(unix))]
        {
            name.to_string_lossy().into_owned()
        }
    }

    pub(crate) fn update_entry(
        self,
        entry: &mut FileEntry,
        cwd: &Path,
        source: &PanelListingSource,
    ) {
        if entry.is_parent() {
            return;
        }
        let name = match source {
            PanelListingSource::Panelize { .. } => entry
                .path
                .strip_prefix(cwd)
                .unwrap_or(&entry.path)
                .as_os_str(),
            PanelListingSource::FindResults { base_dir, .. } => entry
                .path
                .strip_prefix(base_dir)
                .unwrap_or(&entry.path)
                .as_os_str(),
            _ => entry.path.file_name().unwrap_or(entry.path.as_os_str()),
        };
        // ZIP's conventional fallback is CP437 when the member name is not
        // UTF-8. Explicit panel codecs override it without changing path bytes.
        let encoding = self.effective_for_name(name, source);
        entry.name = if name.is_empty() {
            String::from(".")
        } else {
            encoding.decode(name)
        };
    }

    pub(crate) fn effective_for_name(self, name: &OsStr, source: &PanelListingSource) -> Self {
        if self == Self::Utf8
            && name.to_str().is_none()
            && matches!(source, PanelListingSource::Archive(location) if matches!(location.format(), Ok(ArchiveFormat::Zip)))
        {
            Self::Cp437
        } else {
            self
        }
    }
}

impl AppState {
    pub(crate) fn open_encoding_dialog(&mut self, panel: ActivePanel) {
        let encoding = self.panels[panel.index()].filename_encoding;
        self.push_dialog(
            DialogState::listbox(
                "Filename encoding",
                FilenameEncoding::ALL
                    .into_iter()
                    .map(|e| e.label().into())
                    .collect(),
                FilenameEncoding::ALL
                    .iter()
                    .position(|e| *e == encoding)
                    .unwrap_or(0),
            ),
            PendingDialogAction::SetFilenameEncoding { panel },
        );
    }

    pub(crate) fn set_filename_encoding(&mut self, panel: ActivePanel, encoding: FilenameEncoding) {
        if self.panels[panel.index()].loading || self.panel_has_pending_navigation(panel) {
            self.set_status("Wait for the panel to finish loading before changing encoding");
            return;
        }
        if !cfg!(unix) && encoding != FilenameEncoding::Utf8 {
            self.set_status("Legacy byte filename encodings require Unix");
            return;
        }
        self.panels[panel.index()].filename_encoding = encoding;
        self.settings.panel_options.filename_encodings[panel.index()] = encoding;
        self.settings.mark_dirty();
        self.queue_panel_refresh(panel);
        self.set_status(format!(
            "{} panel filename encoding: {}",
            panel.label(),
            encoding.label()
        ));
    }
}

#[cfg(unix)]
const CP437_HIGH: [char; 128] = [
    '\u{00c7}', '\u{00fc}', '\u{00e9}', '\u{00e2}', '\u{00e4}', '\u{00e0}', '\u{00e5}', '\u{00e7}',
    '\u{00ea}', '\u{00eb}', '\u{00e8}', '\u{00ef}', '\u{00ee}', '\u{00ec}', '\u{00c4}', '\u{00c5}',
    '\u{00c9}', '\u{00e6}', '\u{00c6}', '\u{00f4}', '\u{00f6}', '\u{00f2}', '\u{00fb}', '\u{00f9}',
    '\u{00ff}', '\u{00d6}', '\u{00dc}', '\u{00a2}', '\u{00a3}', '\u{00a5}', '\u{20a7}', '\u{0192}',
    '\u{00e1}', '\u{00ed}', '\u{00f3}', '\u{00fa}', '\u{00f1}', '\u{00d1}', '\u{00aa}', '\u{00ba}',
    '\u{00bf}', '\u{2310}', '\u{00ac}', '\u{00bd}', '\u{00bc}', '\u{00a1}', '\u{00ab}', '\u{00bb}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}',
    '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255d}', '\u{255c}', '\u{255b}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252c}', '\u{251c}', '\u{2500}', '\u{253c}', '\u{255e}', '\u{255f}',
    '\u{255a}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256c}', '\u{2567}',
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}', '\u{2558}', '\u{2552}', '\u{2553}', '\u{256b}',
    '\u{256a}', '\u{2518}', '\u{250c}', '\u{2588}', '\u{2584}', '\u{258c}', '\u{2590}', '\u{2580}',
    '\u{03b1}', '\u{00df}', '\u{0393}', '\u{03c0}', '\u{03a3}', '\u{03c3}', '\u{00b5}', '\u{03c4}',
    '\u{03a6}', '\u{0398}', '\u{03a9}', '\u{03b4}', '\u{221e}', '\u{03c6}', '\u{03b5}', '\u{2229}',
    '\u{2261}', '\u{00b1}', '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00f7}', '\u{2248}',
    '\u{00b0}', '\u{2219}', '\u{00b7}', '\u{221a}', '\u{207f}', '\u{00b2}', '\u{25a0}', '\u{00a0}',
];
