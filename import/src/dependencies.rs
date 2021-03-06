use treasury_id::AssetId;

use crate::{Dependency, NOT_FOUND, NOT_UTF8, SUCCESS};

pub trait Dependencies {
    /// Returns dependency id.
    fn get(&mut self, source: &str, target: &str) -> Result<Option<AssetId>, String>;

    fn get_or_append(
        &mut self,
        source: &str,
        target: &str,
        missing: &mut Vec<Dependency>,
    ) -> Result<Option<AssetId>, String> {
        match self.get(source, target) {
            Err(err) => Err(err),
            Ok(Some(id)) => Ok(Some(id)),
            Ok(None) => {
                missing.push(Dependency {
                    source: source.to_owned(),
                    target: target.to_owned(),
                });
                Ok(None)
            }
        }
    }
}

impl<F> Dependencies for F
where
    F: FnMut(&str, &str) -> Option<AssetId>,
{
    fn get(&mut self, source: &str, target: &str) -> Result<Option<AssetId>, String> {
        Ok((*self)(source, target))
    }
}

#[repr(transparent)]
pub struct DependenciesOpaque(u8);

pub type DependenciesGetFn = unsafe extern "C" fn(
    dependencies: *mut DependenciesOpaque,
    source_ptr: *const u8,
    source_len: u32,
    target_ptr: *const u8,
    target_len: u32,
    id_ptr: *mut u64,
) -> i32;

unsafe extern "C" fn dependencies_get_ffi<F>(
    dependencies: *mut DependenciesOpaque,
    source_ptr: *const u8,
    source_len: u32,
    target_ptr: *const u8,
    target_len: u32,
    id_ptr: *mut u64,
) -> i32
where
    F: FnMut(&str, &str) -> Option<AssetId>,
{
    let source =
        match std::str::from_utf8(std::slice::from_raw_parts(source_ptr, source_len as usize)) {
            Ok(source) => source,
            Err(_) => return NOT_UTF8,
        };
    let target =
        match std::str::from_utf8(std::slice::from_raw_parts(target_ptr, target_len as usize)) {
            Ok(target) => target,
            Err(_) => return NOT_UTF8,
        };

    let f = dependencies as *mut F;
    let f = &mut *f;

    match f(source, target) {
        None => return NOT_FOUND,
        Some(id) => {
            std::ptr::write(id_ptr, id.value().get());
            return SUCCESS;
        }
    }
}

pub struct DependenciesFFI {
    pub opaque: *mut DependenciesOpaque,
    pub get: DependenciesGetFn,
}

impl DependenciesFFI {
    pub fn new<F>(f: &mut F) -> Self
    where
        F: FnMut(&str, &str) -> Option<AssetId>,
    {
        DependenciesFFI {
            opaque: f as *mut F as _,
            get: dependencies_get_ffi::<F>,
        }
    }
}

impl Dependencies for DependenciesFFI {
    fn get(&mut self, source: &str, target: &str) -> Result<Option<AssetId>, String> {
        let mut id = 0u64;
        let result = unsafe {
            (self.get)(
                self.opaque,
                source.as_ptr(),
                source.len() as u32,
                target.as_ptr(),
                target.len() as u32,
                &mut id,
            )
        };

        match result {
            SUCCESS => match AssetId::new(id) {
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
