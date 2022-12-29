use std::{fmt, mem::MaybeUninit, path::Path, sync::Arc};

use eyre::WrapErr;

use treasury_id::AssetId;
use treasury_import::{
    version, ExportImportersFnType, ImportError, Importer, ImporterFFI, VersionFnType,
    EXPORT_IMPORTERS_FN_NAME, MAGIC, MAGIC_NAME, VERSION_FN_NAME,
};

pub struct DylibImporter {
    lib_path: Arc<Path>,
    /// Keeps dylib alive.
    _lib: Arc<libloading::Library>,
    ffi: ImporterFFI,
    formats: Vec<String>,
}

impl fmt::Debug for DylibImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} @ {}", &*self.ffi.name(), self.lib_path.display())
    }
}

impl Importer for DylibImporter {
    fn name(&self) -> &str {
        self.ffi.name()
    }

    fn formats(&self) -> &[&str] {
        self.ffi.formats()
    }

    /// Returns list of extensions for source formats.
    fn extensions(&self) -> &[&str];

    /// Returns target format importer produces.
    fn target(&self) -> &str;

    /// Reads data from `source` path and writes result at `output` path.
    fn import(
        &self,
        source: &Path,
        output: &Path,
        sources: &mut dyn Sources,
        dependencies: &mut dyn Dependencies,
    ) -> Result<(), ImportError>;
}

impl DylibImporter {
    pub fn import<'a, S, D>(
        &self,
        source: &Path,
        output: &Path,
        mut sources: S,
        mut dependencies: D,
    ) -> Result<(), ImportError>
    where
        S: FnMut(&str) -> Option<&'a Path> + 'a,
        D: FnMut(&str, &str) -> Option<AssetId>,
    {
        self.ffi
            .import(source, output, &mut sources, &mut dependencies)
    }

    pub fn name(&self) -> &str {
        self.ffi.name()
    }

    pub fn formats(&self) -> Vec<&str> {
        self.ffi.formats()
    }

    pub fn target(&self) -> &str {
        self.ffi.target()
    }

    pub fn extensions(&self) -> Vec<&str> {
        self.ffi.extensions()
    }
}

/// Load importers from dynamic library at specified path.
pub unsafe fn load_importers(
    lib_path: &Path,
) -> eyre::Result<impl Iterator<Item = DylibImporter> + '_> {
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

    let mut importers = Vec::new();
    importers.resize_with(64, MaybeUninit::uninit);

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

    Ok(importers.into_iter().map(move |importer| {
        let ffi: ImporterFFI = importer.assume_init();
        DylibImporter {
            lib_path: lib_path.clone(),
            _lib: lib.clone(),
            ffi,
        }
    }))
}
