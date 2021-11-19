use std::{fmt, mem::MaybeUninit, path::Path, sync::Arc};

use eyre::WrapErr;

use treasury_id::AssetId;
use treasury_import::{
    version, ExportImportersFnType, ImportError, ImporterFFI, VersionFnType,
    EXPORT_IMPORTERS_FN_NAME, MAGIC, MAGIC_NAME, VERSION_FN_NAME,
};

pub struct DylibImporter {
    lib_path: Arc<Path>,
    /// Keeps dylib alive.
    _lib: Arc<libloading::Library>,
    ffi: ImporterFFI,
}

impl fmt::Debug for DylibImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} @ {}",
            &*self.ffi.name_lossy(),
            self.lib_path.display()
        )
    }
}

impl DylibImporter {
    pub fn import<'a, S, D>(
        &self,
        source: &Path,
        output: &Path,
        sources: S,
        dependencies: D,
    ) -> Result<(), ImportError>
    where
        S: Fn(&str) -> Option<&'a Path> + 'a,
        D: Fn(&str, &str) -> Option<AssetId>,
    {
        self.ffi.import(source, output, &sources, &dependencies)
    }

    // pub fn name(&self) -> &str {
    //     self.ffi.name().unwrap_or("<Non-UTF8 name>")
    // }

    pub fn format(&self) -> &str {
        self.ffi.format().unwrap()
    }

    // pub fn target(&self) -> &str {
    //     self.ffi.target().unwrap()
    // }

    pub fn extensions(&self) -> impl Iterator<Item = &str> + '_ {
        self.ffi.extensions().filter_map(Result::ok)
    }
}

/// Load importers from dynamic library at specified path.
pub unsafe fn load_importers(
    lib_path: &Path,
) -> eyre::Result<impl Iterator<Item = (String, String, DylibImporter)> + '_> {
    tracing::info!("Loading importers from '{}'", lib_path.display());

    let lib = libloading::Library::new(lib_path)?;

    // First check the magic value. It must be both present and equal the constant.
    let magic = lib
        .get::<*const u32>(MAGIC_NAME.as_bytes())
        .wrap_err_with(|| eyre::eyre!("'{}' symbol not found", MAGIC_NAME))?;

    eyre::ensure!(
        **magic == MAGIC,
        "Magic value mismatch. Expected '{}', found '{}'",
        MAGIC,
        **magic
    );

    // First check the magic value. It must be both present and equal the constant.
    let lib_ffi_version = lib
        .get::<VersionFnType>(VERSION_FN_NAME.as_bytes())
        .wrap_err_with(|| eyre::eyre!("'{}' symbol not found", VERSION_FN_NAME))?;

    let lib_ffi_version = lib_ffi_version();

    let ffi_version = version();

    eyre::ensure!(
        lib_ffi_version == ffi_version,
        "FFI version mismatch. Dylib is built against treasury-importer-ffi '{}' but this process uses '{}'",
        lib_ffi_version,
        ffi_version,
    );

    let lib = Arc::new(lib);

    let export_importers = lib
        .get::<ExportImportersFnType>(EXPORT_IMPORTERS_FN_NAME.as_bytes())
        .wrap_err_with(|| eyre::eyre!("'{}' symbol not found", EXPORT_IMPORTERS_FN_NAME))?;

    let mut importers: Vec<_> = (0..64).map(|_| MaybeUninit::uninit()).collect();

    loop {
        let count = export_importers(
            importers.as_mut_ptr() as *mut ImporterFFI,
            importers.len() as u32,
        );

        if count > importers.len() as u32 {
            importers.resize_with(count as usize, MaybeUninit::uninit);
            continue;
        }

        importers.truncate(count as usize);
        break;
    }

    let lib_path: Arc<Path> = Arc::from(lib_path);

    Ok(importers.into_iter().filter_map(move |importer| {
        let ffi: ImporterFFI = importer.assume_init();

        let format = match ffi.format() {
            Ok(format) => format.to_owned(),
            Err(_) => {
                tracing::error!(
                    "Library '{}' exports importer with non UTF-8 format",
                    lib_path.display()
                );
                return None;
            }
        };

        let target = match ffi.target() {
            Ok(target) => target.to_owned(),
            Err(_) => {
                tracing::error!(
                    "Library '{}' exports importer with non UTF-8 target",
                    lib_path.display()
                );
                return None;
            }
        };

        match ffi.name() {
            Ok(name) => {
                tracing::info!(
                    "Importer '{}' loader from library '{}'",
                    name,
                    lib_path.display()
                );
            }
            Err(_) => {
                tracing::error!(
                    "Library '{}' exports importer with non UTF-8 name",
                    lib_path.display()
                );
                return None;
            }
        };

        Some((
            format,
            target,
            DylibImporter {
                lib_path: lib_path.clone(),
                _lib: lib.clone(),
                ffi,
            },
        ))
    }))
}
