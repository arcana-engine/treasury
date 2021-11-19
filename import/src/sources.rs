#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

#[cfg(target_os = "wasi")]
use std::os::wasi::ffi::{OsStrExt, OsStringExt};

#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    OsChar, ANY_BUF_LEN_LIMIT, BUFFER_IS_TOO_SMALL, NOT_FOUND, NOT_UTF8, PATH_BUF_LEN_START,
    SUCCESS,
};

pub trait Sources {
    /// Get data from specified source.
    fn get(&self, source: &str) -> Result<Option<PathBuf>, String>;
}

#[repr(transparent)]
pub struct SourcesOpaque(u8);

pub type SourcesGetFn = unsafe extern "C" fn(
    sources: *const SourcesOpaque,
    source_ptr: *const u8,
    source_len: u32,
    path_ptr: *mut OsChar,
    path_len: *mut u32,
) -> i32;

unsafe extern "C" fn sources_get_ffi<'a, F>(
    sources: *const SourcesOpaque,
    source_ptr: *const u8,
    source_len: u32,
    path_ptr: *mut OsChar,
    path_len: *mut u32,
) -> i32
where
    F: Fn(&str) -> Option<&'a Path> + 'a,
{
    let source =
        match std::str::from_utf8(std::slice::from_raw_parts(source_ptr, source_len as usize)) {
            Ok(source) => source,
            Err(_) => return NOT_UTF8,
        };

    let f = sources as *const F;
    let f = &*f;

    match f(source) {
        None => return NOT_FOUND,
        Some(path) => {
            let os_str = path.as_os_str();

            #[cfg(any(unix, target_os = "wasi"))]
            let path: &[u8] = os_str.as_bytes();

            #[cfg(windows)]
            let os_str_wide = os_str.encode_wide().collect::<Vec<u16>>();

            #[cfg(windows)]
            let path: &[u16] = &*os_str_wide;

            if *path_len < path.len() as u32 {
                *path_len = path.len() as u32;
                return BUFFER_IS_TOO_SMALL;
            }

            std::ptr::copy_nonoverlapping(path.as_ptr(), path_ptr, path.len() as u32 as usize);
            *path_len = path.len() as u32;

            return SUCCESS;
        }
    }
}

pub struct SourcesFFI {
    pub opaque: *const SourcesOpaque,
    pub get: SourcesGetFn,
}

impl SourcesFFI {
    pub fn new<'a, F>(f: &F) -> Self
    where
        F: Fn(&str) -> Option<&'a Path> + 'a,
    {
        SourcesFFI {
            opaque: f as *const F as _,
            get: sources_get_ffi::<F>,
        }
    }
}

impl Sources for SourcesFFI {
    fn get(&self, source: &str) -> Result<Option<PathBuf>, String> {
        let mut path_buf = vec![0; PATH_BUF_LEN_START];
        let mut path_len = path_buf.len() as u32;

        loop {
            let result = unsafe {
                (self.get)(
                    self.opaque,
                    source.as_ptr(),
                    source.len() as u32,
                    path_buf.as_mut_ptr(),
                    &mut path_len,
                )
            };

            if result == BUFFER_IS_TOO_SMALL {
                if path_len > ANY_BUF_LEN_LIMIT as u32 {
                    return Err(format!(
                        "Source path does not fit into limit '{}', '{}' required",
                        ANY_BUF_LEN_LIMIT, path_len
                    ));
                }

                path_buf.resize(path_len as usize, 0);
                continue;
            }

            return match result {
                SUCCESS => {
                    #[cfg(any(unix, target_os = "wasi"))]
                    let path = OsStrext::from_vec(path_buf).into();

                    #[cfg(windows)]
                    let path = OsString::from_wide(&path_buf).into();

                    Ok(Some(path))
                }
                NOT_FOUND => return Ok(None),
                NOT_UTF8 => Err(format!("Source is not UTF8 while stored in `str`")),
                _ => Err(format!(
                    "Unexpected return code from `Sources::get` FFI: {}",
                    result
                )),
            };
        }
    }
}
