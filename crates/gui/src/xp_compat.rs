#![cfg(all(target_os = "windows", target_arch = "x86"))]
#![allow(unsafe_code)]

use core::ffi::c_void;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;

const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;
const OFN_NOCHANGEDIR: u32 = 0x0000_0008;
const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
const OFN_EXPLORER: u32 = 0x0008_0000;

const BIF_RETURNONLYFSDIRS: u32 = 0x0000_0001;
const BIF_NEWDIALOGSTYLE: u32 = 0x0000_0040;

pub const CSIDL_APPDATA: i32 = 0x001a;
pub const CSIDL_LOCAL_APPDATA: i32 = 0x001c;

#[repr(C)]
struct OPENFILENAMEW {
    l_struct_size: u32,
    hwnd_owner: *mut c_void,
    h_instance: *mut c_void,
    lpstr_filter: *const u16,
    lpstr_custom_filter: *mut u16,
    n_max_cust_filter: u32,
    n_filter_index: u32,
    lpstr_file: *mut u16,
    n_max_file: u32,
    lpstr_file_title: *mut u16,
    n_max_file_title: u32,
    lpstr_initial_dir: *const u16,
    lpstr_title: *const u16,
    flags: u32,
    n_file_offset: u16,
    n_file_extension: u16,
    lpstr_def_ext: *const u16,
    l_cust_data: isize,
    lpfn_hook: *const c_void,
    lp_template_name: *const u16,
    pv_reserved: *mut c_void,
    dw_reserved: u32,
    flags_ex: u32,
}

#[repr(C)]
struct BROWSEINFOW {
    hwnd_owner: *mut c_void,
    pidl_root: *const c_void,
    psz_display_name: *mut u16,
    lpsz_title: *const u16,
    ul_flags: u32,
    lpfn: *const c_void,
    l_param: isize,
    i_image: i32,
}

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(lpofn: *mut OPENFILENAMEW) -> i32;
    fn GetSaveFileNameW(lpofn: *mut OPENFILENAMEW) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHBrowseForFolderW(lpbi: *mut BROWSEINFOW) -> *mut c_void;
    fn SHGetPathFromIDListW(pidl: *const c_void, psz_path: *mut u16) -> i32;
    fn SHGetFolderPathW(
        hwnd: *mut c_void,
        csidl: i32,
        h_token: *mut c_void,
        dw_flags: u32,
        psz_path: *mut u16,
    ) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(pv: *mut c_void);
}

fn to_wide_null(value: &str) -> Vec<u16> {
    OsString::from(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn build_filter(filters: &[(&str, &str)]) -> Vec<u16> {
    let mut wide = Vec::new();
    for (label, pattern) in filters {
        wide.extend(OsString::from(label).encode_wide());
        wide.push(0);
        wide.extend(OsString::from(pattern).encode_wide());
        wide.push(0);
    }
    wide.push(0);
    wide
}

fn read_wide_path(buf: &[u16]) -> Option<PathBuf> {
    let end = buf.iter().position(|&ch| ch == 0).unwrap_or(buf.len());
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buf[..end])))
}

pub fn pick_open_file(
    title: &str,
    filters: &[(&str, &str)],
    initial_dir: Option<&Path>,
) -> Option<PathBuf> {
    let mut file_buf = vec![0u16; 4096];
    let filter_wide = if filters.is_empty() {
        None
    } else {
        Some(build_filter(filters))
    };
    let title_wide = to_wide_null(title);
    let dir_wide = initial_dir.map(|dir| {
        dir.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    });

    let mut ofn = OPENFILENAMEW {
        l_struct_size: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwnd_owner: ptr::null_mut(),
        h_instance: ptr::null_mut(),
        lpstr_filter: filter_wide.as_ref().map_or(ptr::null(), |f| f.as_ptr()),
        lpstr_custom_filter: ptr::null_mut(),
        n_max_cust_filter: 0,
        n_filter_index: 1,
        lpstr_file: file_buf.as_mut_ptr(),
        n_max_file: file_buf.len() as u32,
        lpstr_file_title: ptr::null_mut(),
        n_max_file_title: 0,
        lpstr_initial_dir: dir_wide.as_ref().map_or(ptr::null(), |d| d.as_ptr()),
        lpstr_title: title_wide.as_ptr(),
        flags: OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR,
        n_file_offset: 0,
        n_file_extension: 0,
        lpstr_def_ext: ptr::null(),
        l_cust_data: 0,
        lpfn_hook: ptr::null(),
        lp_template_name: ptr::null(),
        pv_reserved: ptr::null_mut(),
        dw_reserved: 0,
        flags_ex: 0,
    };

    let result = unsafe { GetOpenFileNameW(&mut ofn) };
    if result != 0 {
        read_wide_path(&file_buf)
    } else {
        None
    }
}

pub fn pick_save_file(
    title: &str,
    default_name: Option<&str>,
    filters: &[(&str, &str)],
    initial_dir: Option<&Path>,
) -> Option<PathBuf> {
    let mut file_buf = vec![0u16; 4096];
    if let Some(name) = default_name {
        let wide: Vec<u16> = OsString::from(name).encode_wide().collect();
        let copy_len = wide.len().min(file_buf.len().saturating_sub(1));
        file_buf[..copy_len].copy_from_slice(&wide[..copy_len]);
    }

    let filter_wide = if filters.is_empty() {
        None
    } else {
        Some(build_filter(filters))
    };
    let title_wide = to_wide_null(title);
    let dir_wide = initial_dir.map(|dir| {
        dir.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    });

    let mut ofn = OPENFILENAMEW {
        l_struct_size: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwnd_owner: ptr::null_mut(),
        h_instance: ptr::null_mut(),
        lpstr_filter: filter_wide.as_ref().map_or(ptr::null(), |f| f.as_ptr()),
        lpstr_custom_filter: ptr::null_mut(),
        n_max_cust_filter: 0,
        n_filter_index: 1,
        lpstr_file: file_buf.as_mut_ptr(),
        n_max_file: file_buf.len() as u32,
        lpstr_file_title: ptr::null_mut(),
        n_max_file_title: 0,
        lpstr_initial_dir: dir_wide.as_ref().map_or(ptr::null(), |d| d.as_ptr()),
        lpstr_title: title_wide.as_ptr(),
        flags: OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT | OFN_NOCHANGEDIR,
        n_file_offset: 0,
        n_file_extension: 0,
        lpstr_def_ext: ptr::null(),
        l_cust_data: 0,
        lpfn_hook: ptr::null(),
        lp_template_name: ptr::null(),
        pv_reserved: ptr::null_mut(),
        dw_reserved: 0,
        flags_ex: 0,
    };

    let result = unsafe { GetSaveFileNameW(&mut ofn) };
    if result != 0 {
        read_wide_path(&file_buf)
    } else {
        None
    }
}

pub fn pick_folder(title: &str) -> Option<PathBuf> {
    let mut display_buf = vec![0u16; 260];
    let mut path_buf = vec![0u16; 4096];
    let title_wide = to_wide_null(title);

    let mut bi = BROWSEINFOW {
        hwnd_owner: ptr::null_mut(),
        pidl_root: ptr::null(),
        psz_display_name: display_buf.as_mut_ptr(),
        lpsz_title: title_wide.as_ptr(),
        ul_flags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        lpfn: ptr::null(),
        l_param: 0,
        i_image: 0,
    };

    unsafe {
        let pidl = SHBrowseForFolderW(&mut bi);
        if pidl.is_null() {
            return None;
        }
        let ok = SHGetPathFromIDListW(pidl, path_buf.as_mut_ptr());
        CoTaskMemFree(pidl);
        if ok != 0 {
            read_wide_path(&path_buf)
        } else {
            None
        }
    }
}

pub fn known_folder_xp(csidl: i32) -> Option<PathBuf> {
    let mut buf = vec![0u16; 260];
    let hr = unsafe { SHGetFolderPathW(ptr::null_mut(), csidl, ptr::null_mut(), 0, buf.as_mut_ptr()) };
    if hr == 0 {
        read_wide_path(&buf)
    } else {
        None
    }
}
