//! Contains everything that is required to create treasury importers library.
//!
//!
//! # Usage
//!
//! ```
//! struct FooImporter;
//!
//! impl treasury_import::Importer for FooImporter {
//!     fn name(&self) -> &str {
//!         "Foo importer"
//!     }
//!
//!     fn formats(&self) -> &[&str] {
//!         &["foo"]
//!     }
//!
//!     fn target(&self) -> &str {
//!         "foo"
//!     }
//!
//!     fn extensions(&self) -> &[&str] {
//!         &["json"]
//!     }
//!
//!     fn import(
//!         &self,
//!         source: &std::path::Path,
//!         output: &std::path::Path,
//!         _sources: &mut dyn treasury_import::Sources,
//!         _dependencies: &mut dyn treasury_import::Dependencies,
//!     ) -> Result<(), treasury_import::ImportError> {
//!         match std::fs::copy(source, output) {
//!           Ok(_) => Ok(()),
//!           Err(err) => Err(treasury_import::ImportError::Other { reason: "SOMETHING WENT WRONG".to_owned() }),
//!         }
//!     }
//! }
//!
//!
//! // Define all required exports.
//! treasury_import::make_treasury_importers_library! {
//!     // Each <expr;> must have type &'static I where I: Importer
//!     &FooImporter;
//! }
//! ```

mod dependencies;
mod ffi;
mod importer;
mod sources;

#[cfg(feature = "libloading")]
pub mod loading;

pub use ffi::ImporterFFI;

pub use self::{
    dependencies::{Dependencies, Dependency},
    importer::{ImportError, Importer},
    sources::Sources,
};

/// Helper function to emit an error if some dependencies are missing.
pub fn ensure_dependencies(missing: Vec<Dependency>) -> Result<(), ImportError> {
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ImportError::RequireDependencies {
            dependencies: missing,
        })
    }
}

/// Helper function to emit an error if some sources are missing.
pub fn ensure_sources(missing: Vec<String>) -> Result<(), ImportError> {
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ImportError::RequireSources { sources: missing })
    }
}

pub fn version() -> u32 {
    let version = env!("CARGO_PKG_VERSION_MINOR");
    let version = version.parse().unwrap();
    assert_ne!(
        version,
        u32::MAX,
        "Minor version hits u32::MAX. Oh no. Upgrade to u64",
    );
    version
}

pub const MAGIC: u32 = u32::from_le_bytes(*b"TRES");

/// Defines exports required for an importers library.
/// Accepts repetition of importer expressions of type [`&'static impl Importer`] delimited by ';'.
///
/// This macro must be used exactly once in a library crate.
/// The library must be compiled as a dynamic library to be loaded by the treasury.
#[macro_export]
macro_rules! make_treasury_importers_library {
    ($($importer:expr);* $(;)?) => {
        #[no_mangle]
        pub static TREASURY_DYLIB_MAGIC: u32 = $crate::MAGIC;

        #[no_mangle]
        pub unsafe extern "C" fn treasury_importer_ffi_version_minor() -> u32 {
            $crate::version()
        }

        #[no_mangle]
        pub unsafe extern "C" fn treasury_export_importers(buffer: *mut $crate::ImporterFFI, mut cap: u32) -> u32 {
            let mut len = 0;
            $(
                if cap > 0 {
                    core::ptr::write(buffer.add(len as usize), $crate::ImporterFFI::new($importer));
                    cap -= 1;
                }
                len += 1;
            )*
            len
        }
    };
}
