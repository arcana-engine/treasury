//! Contains everything that is required to create treasury importers library.
//!
//!
//! # Usage
//!
//! ```
//! use std::path::Path;
//! use treasury_importer::*;
//!
//! impl Importer for MyImporter {
//!     fn import(&self, source: &Path, output: &Path, treasury: &impl Treasury) -> Result<(), String> {
//!         todo!("Implement this")
//!     }
//! }
//!
//! make_treasury_importers_library! {
//!     my_importer : input_format -> target_format : &MyImporter;
//! }
//! ```

use std::{borrow::Cow, ffi::OsString, mem::size_of, path::Path, str::Utf8Error};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

#[cfg(target_os = "wasi")]
use std::os::wasi::ffi::{OsStrExt, OsStringExt};

#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use dependencies::DependenciesFFI;
use sources::SourcesFFI;

mod dependencies;
mod sources;

pub use dependencies::Dependencies;
pub use sources::Sources;
use treasury_id::AssetId;

pub const MAGIC: u32 = u32::from_le_bytes(*b"TRES");

pub type MagicType = u32;
pub const MAGIC_NAME: &'static str = "TREASURY_DYLIB_MAGIC";

pub type VersionFnType = unsafe extern "C" fn() -> u32;
pub const VERSION_FN_NAME: &'static str = "treasury_importer_ffi_version";

pub type ExportImportersFnType = unsafe extern "C" fn(buffer: *mut ImporterFFI, count: u32) -> u32;
pub const EXPORT_IMPORTERS_FN_NAME: &'static str = "treasury_export_importers";

pub fn version() -> u32 {
    let major = env!("CARGO_PKG_VERSION_MAJOR");
    let version = major.parse().unwrap();
    assert_ne!(
        version,
        u32::MAX,
        "Major version hits u32::MAX. Oh no. Upgrade to u64",
    );
    version
}

const RESULT_BUF_LEN_START: usize = 1024;
const PATH_BUF_LEN_START: usize = 1024;
const ANY_BUF_LEN_LIMIT: usize = 65536;

const REQUIRE_SOURCES: i32 = 2;
const REQUIRE_DEPENDENCIES: i32 = 1;
const SUCCESS: i32 = 0;
const NOT_FOUND: i32 = -1;
const NOT_UTF8: i32 = -2;
const BUFFER_IS_TOO_SMALL: i32 = -3;
const OTHER_ERROR: i32 = -6;

#[cfg(any(unix, target_os = "wasi"))]
type OsChar = u8;

#[cfg(windows)]
type OsChar = u16;

/// Result of `Importer::import` method.
pub enum ImportResult {
    /// Import successful.
    Ok,
    /// Importer requires data from other sources.
    RequireSources {
        /// URLs relative to source path.
        sources: Vec<String>,
    },
    /// Importer requires following dependencies.
    RequireDependencies {
        triples: Vec<(String, Option<String>, String)>,
    },
    /// Importer failed to import the asset.
    Err {
        /// Failure reason.
        reason: String,
    },
}

/// Trait for an importer.
pub trait Importer: Send + Sync {
    /// Reads data from `source` path and writes result at `output` path.
    fn import(
        &self,
        source: &Path,
        output: &Path,
        sources: &impl Sources,
        dependencies: &impl Dependencies,
    ) -> ImportResult;
}

#[repr(transparent)]
struct ImporterOpaque(u8);

type ImporterImportFn = unsafe extern "C" fn(
    importer: *const ImporterOpaque,
    source_ptr: *const OsChar,
    source_len: u32,
    output_ptr: *const OsChar,
    output_len: u32,
    sources: *const sources::SourcesOpaque,
    sources_get: sources::SourcesGetFn,
    dependencies: *const dependencies::DependenciesOpaque,
    dependencies_get: dependencies::DependenciesGetFn,
    result_ptr: *mut u8,
    result_len: *mut u32,
) -> i32;

unsafe extern "C" fn importer_import_ffi<I>(
    importer: *const ImporterOpaque,
    source_ptr: *const OsChar,
    source_len: u32,
    output_ptr: *const OsChar,
    output_len: u32,
    sources: *const sources::SourcesOpaque,
    sources_get: sources::SourcesGetFn,
    dependencies: *const dependencies::DependenciesOpaque,
    dependencies_get: dependencies::DependenciesGetFn,
    result_ptr: *mut u8,
    result_len: *mut u32,
) -> i32
where
    I: Importer,
{
    let source = std::slice::from_raw_parts(source_ptr, source_len as usize);
    let output = std::slice::from_raw_parts(output_ptr, output_len as usize);

    #[cfg(any(unix, target_os = "wasi"))]
    let source = OsStr::from_bytes(source);
    #[cfg(any(unix, target_os = "wasi"))]
    let output = OsStr::from_bytes(output);

    #[cfg(windows)]
    let source = OsString::from_wide(source);
    #[cfg(windows)]
    let output = OsString::from_wide(output);

    let sources = SourcesFFI {
        opaque: sources,
        get: sources_get,
    };

    let dependencies = DependenciesFFI {
        opaque: dependencies,
        get: dependencies_get,
    };

    let importer = &*(importer as *const I);
    let result = importer.import(source.as_ref(), output.as_ref(), &sources, &dependencies);

    match result {
        ImportResult::Ok => SUCCESS,
        ImportResult::RequireSources { sources } => {
            let len_required = sources
                .iter()
                .fold(0, |acc, p| acc + p.len() + size_of::<u32>())
                + size_of::<u32>();

            assert!(u32::try_from(len_required).is_ok());

            if *result_len < len_required as u32 {
                *result_len = len_required as u32;
                return BUFFER_IS_TOO_SMALL;
            }

            std::ptr::copy_nonoverlapping(
                (sources.len() as u32).to_le_bytes().as_ptr(),
                result_ptr,
                size_of::<u32>(),
            );

            let mut offset = size_of::<u32>();

            for url in &sources {
                let len = url.len();

                std::ptr::copy_nonoverlapping(
                    (len as u32).to_le_bytes().as_ptr(),
                    result_ptr.add(offset),
                    size_of::<u32>(),
                );
                offset += size_of::<u32>();

                std::ptr::copy_nonoverlapping(
                    url.as_ptr(),
                    result_ptr.add(offset),
                    len as u32 as usize,
                );
                offset += len;
            }

            debug_assert_eq!(len_required, offset);

            *result_len = len_required as u32;
            REQUIRE_SOURCES
        }
        ImportResult::RequireDependencies { triples } => {
            let len_required = triples.iter().fold(0, |acc, (s, f, t)| {
                acc + s.len() + f.as_ref().map_or(0, String::len) + t.len() + size_of::<u32>() * 3
            }) + size_of::<u32>();

            assert!(u32::try_from(len_required).is_ok());

            if *result_len < len_required as u32 {
                *result_len = len_required as u32;
                return BUFFER_IS_TOO_SMALL;
            }

            std::ptr::copy_nonoverlapping(
                (triples.len() as u32).to_le_bytes().as_ptr(),
                result_ptr,
                size_of::<u32>(),
            );

            let mut offset = size_of::<u32>();

            for (s, f, t) in &triples {
                for s in [Some(s), f.as_ref(), Some(t)] {
                    match s {
                        None => {
                            std::ptr::copy_nonoverlapping(
                                (u32::MAX).to_le_bytes().as_ptr(),
                                result_ptr.add(offset),
                                size_of::<u32>(),
                            );
                        }
                        Some(s) => {
                            let len = s.len();

                            std::ptr::copy_nonoverlapping(
                                (len as u32).to_le_bytes().as_ptr(),
                                result_ptr.add(offset),
                                size_of::<u32>(),
                            );
                            offset += size_of::<u32>();

                            std::ptr::copy_nonoverlapping(
                                s.as_ptr(),
                                result_ptr.add(offset),
                                len as u32 as usize,
                            );
                            offset += len;
                        }
                    }
                }
            }

            debug_assert_eq!(len_required, offset);

            *result_len = len_required as u32;
            REQUIRE_DEPENDENCIES
        }
        ImportResult::Err { reason } => {
            if *result_len < reason.len() as u32 {
                *result_len = reason.len() as u32;
                return BUFFER_IS_TOO_SMALL;
            }

            let error_buf = std::slice::from_raw_parts_mut(result_ptr, reason.len());
            error_buf.copy_from_slice(reason.as_bytes());
            *result_len = reason.len() as u32;
            OTHER_ERROR
        }
    }
}

const MAX_EXTENSION_LEN: usize = 16;
const MAX_EXTENSION_COUNT: usize = 256;
const MAX_FFI_NAME_LEN: usize = 256;

#[repr(C)]
pub struct ImporterFFI {
    importer: *const ImporterOpaque,
    import: ImporterImportFn,
    name: [u8; MAX_FFI_NAME_LEN],
    format: [u8; MAX_FFI_NAME_LEN],
    target: [u8; MAX_FFI_NAME_LEN],
    extensions: [[u8; MAX_EXTENSION_LEN]; MAX_EXTENSION_COUNT],
}

/// Exporting non thread-safe importers breaks the contract of the FFI.
/// The potential unsoundness is covered by `load_dylib_importers` unsafety.
/// There is no way to guarantee that dynamic library will uphold the contract,
/// making `load_dylib_importers` inevitably unsound.
unsafe impl Send for ImporterFFI {}
unsafe impl Sync for ImporterFFI {}

impl ImporterFFI {
    pub fn new<'a, I>(
        importer: &'static I,
        name: &str,
        format: &str,
        target: &str,
        extensions: &[&'a str],
    ) -> Self
    where
        I: Importer,
    {
        let importer = importer as *const I as *const ImporterOpaque;

        assert!(
            name.len() <= MAX_FFI_NAME_LEN,
            "Importer name should fit into {} bytes",
            MAX_FFI_NAME_LEN
        );
        assert!(
            format.len() <= MAX_FFI_NAME_LEN,
            "Importer format should fit into {} bytes",
            MAX_FFI_NAME_LEN
        );
        assert!(
            target.len() <= MAX_FFI_NAME_LEN,
            "Importer target should fit into {} bytes",
            MAX_FFI_NAME_LEN
        );
        assert!(
            extensions.len() < MAX_EXTENSION_COUNT,
            "Importer should support no more than {} extensions",
            MAX_EXTENSION_COUNT,
        );
        assert!(
            extensions.iter().all(|e| e.len() < MAX_EXTENSION_LEN),
            "Importer extensions should fit into {} bytes",
            MAX_EXTENSION_LEN,
        );

        assert!(!name.is_empty(), "Importer name should not be empty");
        assert!(!format.is_empty(), "Importer format should not be empty");
        assert!(!target.is_empty(), "Importer target should not be empty");
        assert!(
            extensions.iter().all(|e| !e.is_empty()),
            "Importer extensions should not be empty"
        );

        assert!(
            !name.contains('\0'),
            "Importer name should not contain '\\0' byte"
        );
        assert!(
            !format.contains('\0'),
            "Importer format should not contain '\\0' byte"
        );
        assert!(
            !target.contains('\0'),
            "Importer target should not contain '\\0' byte"
        );
        assert!(
            extensions.iter().all(|e| !e.contains('\0')),
            "Importer extensions should not contain '\\0' byte"
        );

        let mut name_buf = [0; MAX_FFI_NAME_LEN];
        name_buf[..name.len()].copy_from_slice(name.as_bytes());

        let mut format_buf = [0; MAX_FFI_NAME_LEN];
        format_buf[..format.len()].copy_from_slice(format.as_bytes());

        let mut target_buf = [0; MAX_FFI_NAME_LEN];
        target_buf[..target.len()].copy_from_slice(target.as_bytes());

        let mut extensions_buf = [[0; MAX_EXTENSION_LEN]; MAX_EXTENSION_COUNT];

        for (i, &extension) in extensions.iter().enumerate() {
            extensions_buf[i][..extension.len()].copy_from_slice(extension.as_bytes());
        }

        ImporterFFI {
            importer,
            import: importer_import_ffi::<I>,
            name: name_buf,
            format: format_buf,
            target: target_buf,
            extensions: extensions_buf,
        }
    }

    pub fn name(&self) -> Result<&str, Utf8Error> {
        match self.name.iter().position(|b| *b == 0) {
            None => std::str::from_utf8(&self.name),
            Some(i) => std::str::from_utf8(&self.name[..i]),
        }
    }

    pub fn name_lossy(&self) -> Cow<'_, str> {
        match self.name.iter().position(|b| *b == 0) {
            None => String::from_utf8_lossy(&self.name),
            Some(i) => String::from_utf8_lossy(&self.name[..i]),
        }
    }

    pub fn format(&self) -> Result<&str, Utf8Error> {
        match self.format.iter().position(|b| *b == 0) {
            None => std::str::from_utf8(&self.format),
            Some(i) => std::str::from_utf8(&self.format[..i]),
        }
    }

    pub fn target(&self) -> Result<&str, Utf8Error> {
        match self.target.iter().position(|b| *b == 0) {
            None => std::str::from_utf8(&self.target),
            Some(i) => std::str::from_utf8(&self.target[..i]),
        }
    }

    pub fn extensions(&self) -> impl Iterator<Item = Result<&str, Utf8Error>> {
        let iter = self
            .extensions
            .iter()
            .take_while(|extension| extension[0] != 0);

        iter.map(|extension| match extension.iter().position(|b| *b == 0) {
            None => std::str::from_utf8(extension),
            Some(i) => std::str::from_utf8(&extension[..i]),
        })
    }

    pub fn import<'a, S, D>(
        &self,
        source: &Path,
        output: &Path,
        sources: &S,
        dependencies: &D,
    ) -> ImportResult
    where
        S: Fn(&str) -> Option<&'a Path> + 'a,
        D: Fn(&str, Option<&str>, &str) -> Option<AssetId>,
    {
        let os_str = source.as_os_str();

        #[cfg(any(unix, target_os = "wasi"))]
        let source: &[u8] = os_str.as_bytes();

        #[cfg(windows)]
        let os_str_wide = os_str.encode_wide().collect::<Vec<u16>>();

        #[cfg(windows)]
        let source: &[u16] = &*os_str_wide;

        let os_str = output.as_os_str();

        #[cfg(any(unix, target_os = "wasi"))]
        let output: &[u8] = os_str.as_bytes();

        #[cfg(windows)]
        let os_str_wide = os_str.encode_wide().collect::<Vec<u16>>();

        #[cfg(windows)]
        let output: &[u16] = &*os_str_wide;

        let sources = SourcesFFI::new(sources);
        let dependencies = DependenciesFFI::new(dependencies);

        let mut result_buf = vec![0; RESULT_BUF_LEN_START];
        let mut result_len = result_buf.len() as u32;

        loop {
            let result = unsafe {
                (self.import)(
                    self.importer,
                    source.as_ptr(),
                    source.len() as u32,
                    output.as_ptr(),
                    output.len() as u32,
                    sources.opaque,
                    sources.get,
                    dependencies.opaque,
                    dependencies.get,
                    result_buf.as_mut_ptr(),
                    &mut result_len,
                )
            };

            if result == BUFFER_IS_TOO_SMALL {
                if result_len > ANY_BUF_LEN_LIMIT as u32 {
                    return ImportResult::Err {
                        reason: format!(
                            "Result does not fit into limit '{}', '{}' required",
                            ANY_BUF_LEN_LIMIT, result_len
                        ),
                    };
                }

                result_buf.resize(result_len as usize, 0);
                continue;
            }

            return match result {
                SUCCESS => ImportResult::Ok,
                REQUIRE_SOURCES => unsafe {
                    let mut u32buf = [0; size_of::<u32>()];
                    std::ptr::copy_nonoverlapping(
                        result_buf[..size_of::<u32>()].as_ptr(),
                        u32buf.as_mut_ptr(),
                        size_of::<u32>(),
                    );
                    let count = u32::from_le_bytes(u32buf);

                    let mut offset = size_of::<u32>();

                    let mut sources = Vec::new();
                    for _ in 0..count {
                        std::ptr::copy_nonoverlapping(
                            result_buf[offset..][..size_of::<u32>()].as_ptr(),
                            u32buf.as_mut_ptr(),
                            size_of::<u32>(),
                        );
                        offset += size_of::<u32>();
                        let len = u32::from_be_bytes(u32buf);
                        let mut source = vec![0; len as usize];
                        std::ptr::copy_nonoverlapping(
                            result_buf[offset..][..len as usize].as_ptr(),
                            source.as_mut_ptr(),
                            len as usize,
                        );
                        match String::from_utf8(source) {
                            Ok(source) => sources.push(source),
                            Err(_) => return ImportResult::Err {
                                reason: "`Importer::import` requires sources, but one of the sources is not UTF-8"
                                    .to_owned(),
                            },
                        }
                    }

                    ImportResult::RequireSources { sources }
                },
                REQUIRE_DEPENDENCIES => unsafe {
                    let mut u32buf = [0; size_of::<u32>()];
                    std::ptr::copy_nonoverlapping(
                        result_buf[..size_of::<u32>()].as_ptr(),
                        u32buf.as_mut_ptr(),
                        size_of::<u32>(),
                    );
                    let count = u32::from_le_bytes(u32buf);

                    let mut offset = size_of::<u32>();

                    let mut triples = Vec::new();
                    for _ in 0..count {
                        // Decode source

                        std::ptr::copy_nonoverlapping(
                            result_buf[offset..][..size_of::<u32>()].as_ptr(),
                            u32buf.as_mut_ptr(),
                            size_of::<u32>(),
                        );
                        offset += size_of::<u32>();
                        let len = u32::from_be_bytes(u32buf);

                        let mut source = vec![0; len as usize];
                        std::ptr::copy_nonoverlapping(
                            result_buf[offset..][..len as usize].as_ptr(),
                            source.as_mut_ptr(),
                            len as usize,
                        );

                        let source = match String::from_utf8(source) {
                            Ok(source) => source,
                            Err(_) => return ImportResult::Err {
                                reason: "`Importer::import` requires dependencies, but one of the sources is not UTF-8"
                                    .to_owned(),
                            },
                        };

                        // Decode format

                        std::ptr::copy_nonoverlapping(
                            result_buf[offset..][..size_of::<u32>()].as_ptr(),
                            u32buf.as_mut_ptr(),
                            size_of::<u32>(),
                        );
                        offset += size_of::<u32>();
                        let len = u32::from_be_bytes(u32buf);

                        let format = match len {
                            u32::MAX => None,
                            _ => {
                                let mut format = vec![0; len as usize];
                                std::ptr::copy_nonoverlapping(
                                    result_buf[offset..][..len as usize].as_ptr(),
                                    format.as_mut_ptr(),
                                    len as usize,
                                );

                                match String::from_utf8(format) {
                                    Ok(format) => Some(format),
                                    Err(_) => return ImportResult::Err {
                                        reason: "`Importer::import` requires dependencies, but one of the formats is not UTF-8"
                                            .to_owned(),
                                    },
                                }
                            }
                        };

                        // Decode target

                        std::ptr::copy_nonoverlapping(
                            result_buf[offset..][..size_of::<u32>()].as_ptr(),
                            u32buf.as_mut_ptr(),
                            size_of::<u32>(),
                        );
                        offset += size_of::<u32>();
                        let len = u32::from_be_bytes(u32buf);
                        let mut target = vec![0; len as usize];
                        std::ptr::copy_nonoverlapping(
                            result_buf[offset..][..len as usize].as_ptr(),
                            target.as_mut_ptr(),
                            len as usize,
                        );

                        let target = match String::from_utf8(target) {
                            Ok(target) => target,
                            Err(_) => return ImportResult::Err {
                                reason: "`Importer::import` requires dependencies, but one of the targets is not UTF-8"
                                    .to_owned(),
                            },
                        };

                        triples.push((source, format, target));
                    }

                    ImportResult::RequireDependencies { triples }
                },
                OTHER_ERROR => {
                    debug_assert!(result_len <= result_buf.len() as u32);

                    let error = &result_buf[..result_len as usize];
                    let error_lossy = String::from_utf8_lossy(error);

                    ImportResult::Err {
                        reason: error_lossy.into_owned(),
                    }
                }
                _ => ImportResult::Err {
                    reason: format!(
                        "Unexpected return code from `Importer::import` FFI: {}",
                        result
                    ),
                },
            };
        }
    }
}

/// Define exports required for an importers library.
/// Accepts repetition of the following pattern:
/// <format string literal> -> <target string literal> : <importer expression of type [`&'static impl Importer`]">
#[macro_export]
macro_rules! make_treasury_importers_library {
    ($(
        $([$( $ext:ident ),* $(,)?])? $name:ident : $format:ident -> $target:ident = $importer:expr;
    )*) => {
        #[no_mangle]
        pub static TREASURY_DYLIB_MAGIC: u32 = $crate::MAGIC;

        #[no_mangle]
        pub unsafe extern "C" fn treasury_importer_ffi_version() -> u32 {
            $crate::version()
        }

        #[no_mangle]
        pub unsafe extern "C" fn treasury_export_importers(buffer: *mut $crate::ImporterFFI, count: u32) -> u32 {
            let mut len = 0;
            let mut cap = count + 1;
            $(
                cap -= 1;
                if cap > 0 {
                    core::ptr::write(buffer.add(len as usize), $crate::ImporterFFI::new($importer, ::core::stringify!($name), ::core::stringify!($format), ::core::stringify!($target), &[ $($(::core::stringify!($ext)),*)? ]));
                }
                len += 1;
            )*

            len
        }
    };
}
