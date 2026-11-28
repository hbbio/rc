use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::ERROR_CANT_RESOLVE_FILENAME;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FileBasicInfo,
    GetFileInformationByHandleEx, GetVolumeInformationByHandleW,
};

/// Follow the path using a handle that can read attributes without reading file data.
pub fn open_file_for_metadata(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// Whether this volume records a change timestamp independently of the last-write time.
pub fn volume_supports_change_time(file: &File) -> bool {
    let mut name = [0_u16; 32];
    // SAFETY: file owns the handle for the call and name is writable for the
    // specified number of UTF-16 code units. The unused output pointers are optional.
    let result = unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if result == 0 {
        return false;
    }
    let end = name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(name.len());
    let name = String::from_utf16_lossy(&name[..end]);
    ["NTFS", "ReFS", "UDFS"]
        .iter()
        .any(|filesystem| name.eq_ignore_ascii_case(filesystem))
}

/// Read the change timestamp when the handle provides one.
pub fn file_change_time(file: &File) -> Option<i64> {
    let mut info = FILE_BASIC_INFO::default();
    // SAFETY: file owns the handle for the call and info has the layout and size
    // required by FileBasicInfo.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            std::ptr::from_mut(&mut info).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    (result != 0 && info.ChangeTime > 0).then_some(info.ChangeTime)
}

pub fn is_symlink_loop(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ERROR_CANT_RESOLVE_FILENAME as i32)
}
