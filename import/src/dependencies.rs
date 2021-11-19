use treasury_id::AssetId;

use crate::{NOT_FOUND, NOT_UTF8, SUCCESS};

pub trait Dependencies {
    /// Returns dependency id.
    fn get(
        &self,
        source: &str,
        format: Option<&str>,
        target: &str,
    ) -> Result<Option<AssetId>, String>;
}

#[repr(transparent)]
pub struct DependenciesOpaque(u8);

pub type DependenciesGetFn = unsafe extern "C" fn(
    dependencies: *const DependenciesOpaque,
    source_ptr: *const u8,
    source_len: u32,
    format_ptr: *const u8,
    format_len: u32,
    target_ptr: *const u8,
    target_len: u32,
    id_ptr: *mut u64,
) -> i32;

unsafe extern "C" fn dependencies_get_ffi<F>(
    dependencies: *const DependenciesOpaque,
    source_ptr: *const u8,
    source_len: u32,
    format_ptr: *const u8,
    format_len: u32,
    target_ptr: *const u8,
    target_len: u32,
    id_ptr: *mut u64,
) -> i32
where
    F: Fn(&str, Option<&str>, &str) -> Option<AssetId>,
{
    let source =
        match std::str::from_utf8(std::slice::from_raw_parts(source_ptr, source_len as usize)) {
            Ok(source) => source,
            Err(_) => return NOT_UTF8,
        };
    let format = match format_len {
        u32::MAX => None,
        _ => {
            match std::str::from_utf8(std::slice::from_raw_parts(format_ptr, format_len as usize)) {
                Ok(format) => Some(format),
                Err(_) => return NOT_UTF8,
            }
        }
    };
    let target =
        match std::str::from_utf8(std::slice::from_raw_parts(target_ptr, target_len as usize)) {
            Ok(target) => target,
            Err(_) => return NOT_UTF8,
        };

    let f = dependencies as *const F;
    let f = &*f;

    match f(source, format, target) {
        None => return NOT_FOUND,
        Some(id) => {
            std::ptr::write(id_ptr, id.value().get());
            return SUCCESS;
        }
    }
}

pub struct DependenciesFFI {
    pub opaque: *const DependenciesOpaque,
    pub get: DependenciesGetFn,
}

impl DependenciesFFI {
    pub fn new<F>(f: &F) -> Self
    where
        F: Fn(&str, Option<&str>, &str) -> Option<AssetId>,
    {
        DependenciesFFI {
            opaque: f as *const F as _,
            get: dependencies_get_ffi::<F>,
        }
    }
}

impl Dependencies for DependenciesFFI {
    fn get(
        &self,
        source: &str,
        format: Option<&str>,
        target: &str,
    ) -> Result<Option<AssetId>, String> {
        let mut id = 0u64;
        let result = unsafe {
            (self.get)(
                self.opaque,
                source.as_ptr(),
                source.len() as u32,
                format.map_or(std::ptr::null(), |f| f.as_ptr()),
                format.map_or(u32::MAX, |f| f.len() as u32),
                target.as_ptr(),
                target.len() as u32,
                &mut id,
            )
        };

        match result {
            SUCCESS => match AssetId::new(0) {
                None => Err(format!("Null AssetId returned from `Dependencies::get`")),
                Some(id) => Ok(Some(id)),
            },
            NOT_FOUND => Ok(None),
            NOT_UTF8 => Err(format!("Source is not UTF8 while stored in `str`")),

            _ => Err(format!(
                "Unexpected return code from `Sources::get` FFI: {}",
                result
            )),
        }
    }
}
