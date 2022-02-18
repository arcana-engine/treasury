use std::{borrow::Cow, fmt, path::Path};

use hashbrown::{hash_map::RawEntryMut, HashMap};
use treasury_id::AssetId;
use treasury_import::{Dependencies, ImportError, Sources};

use self::dylib::DylibImporter;

mod dylib;

#[derive(Debug, thiserror::Error)]
#[error("Multiple importers may import from different formats '{formats:?}' to target '{target}'")]
pub struct CannotDecideOnImporter {
    formats: Vec<String>,
    target: String,
}

struct ToTarget {
    importers: Vec<Importer>,
    formats: HashMap<String, usize>,
    extensions: HashMap<String, usize>,
}

pub struct Importers {
    targets: HashMap<String, ToTarget>,
}

impl Importers {
    pub fn new() -> Self {
        Importers {
            targets: HashMap::new(),
        }
    }

    pub fn register_importer(&mut self, importer: impl treasury_import::Importer + 'static) {
        self.add_importer(Importer::DynTraitImporter(Box::new(importer)));
    }

    /// Loads importers from dylib.
    /// There is no possible way to guarantee that dylib does not break safety contracts.
    /// Some measures to ensure safety are taken.
    /// Providing dylib from which importers will be successfully imported and then cause an UB should possible only on purpose.
    pub unsafe fn load_dylib_importers(&mut self, lib_path: &Path) -> eyre::Result<()> {
        let iter = load_dylib_importers(lib_path)?;

        for importer in iter {
            self.add_importer(importer);
        }

        Ok(())
    }

    /// Try to guess importer by optionally provided format and extension or by target alone.
    pub fn guess(
        &self,
        format: Option<&str>,
        extension: Option<&str>,
        target: &str,
    ) -> Result<Option<&Importer>, CannotDecideOnImporter> {
        tracing::debug!("Guessing importer to '{}'", target);

        let to_target = self.targets.get(target);

        match to_target {
            None => {
                tracing::debug!("No importers to '{}' found", target);
                Ok(None)
            }
            Some(to_target) => match format {
                None => match extension {
                    None => match to_target.importers.len() {
                        0 => {
                            unreachable!()
                        }
                        1 => Ok(Some(&to_target.importers[0])),
                        _ => {
                            tracing::debug!("Multiple importers to '{}' found", target);
                            Err(CannotDecideOnImporter {
                                target: target.to_owned(),
                                formats: to_target.formats.keys().cloned().collect(),
                            })
                        }
                    },
                    Some(extension) => match to_target.extensions.get(extension) {
                        None => Ok(None),
                        Some(&idx) => Ok(Some(&to_target.importers[idx])),
                    },
                },
                Some(format) => match to_target.formats.get(format) {
                    None => Ok(None),
                    Some(&idx) => Ok(Some(&to_target.importers[idx])),
                },
            },
        }
    }

    fn add_importer(&mut self, importer: Importer) {
        let name = importer.name();
        let target = importer.target();
        let formats = importer.formats();
        let extensions = importer.extensions();

        tracing::info!(
            "Registering importer '{}'. '{:?}' -> '{}' {:?}",
            name,
            formats,
            target,
            extensions,
        );

        match self.targets.raw_entry_mut().from_key(target) {
            RawEntryMut::Vacant(entry) => {
                let to_target = entry
                    .insert(
                        target.to_owned(),
                        ToTarget {
                            importers: Vec::new(),
                            formats: HashMap::new(),
                            extensions: HashMap::new(),
                        },
                    )
                    .1;

                for &format in &*formats {
                    to_target.formats.insert(format.to_owned(), 0);
                }

                for &extension in &*extensions {
                    to_target.extensions.insert(extension.to_owned(), 0);
                }
                to_target.importers.push(importer);
            }
            RawEntryMut::Occupied(entry) => {
                let to_target = entry.into_mut();
                let idx = to_target.importers.len();

                for &format in &*formats {
                    match to_target.formats.raw_entry_mut().from_key(format) {
                        RawEntryMut::Vacant(entry) => {
                            entry.insert(format.to_owned(), idx);
                        }
                        RawEntryMut::Occupied(entry) => {
                            tracing::error!(
                                "'{}' -> '{}' importer already registered: {:#?}",
                                format,
                                target,
                                entry.get(),
                            );
                        }
                    }
                }

                for &extension in &*extensions {
                    match to_target.extensions.raw_entry_mut().from_key(extension) {
                        RawEntryMut::Vacant(entry) => {
                            entry.insert(extension.to_owned(), idx);
                        }
                        RawEntryMut::Occupied(entry) => {
                            tracing::error!(
                                "'.{}' -> '{}' importer already registered: {:#?}",
                                extension,
                                target,
                                entry.get(),
                            );
                        }
                    }
                }

                to_target.importers.push(importer);
            }
        }
    }
}

/// Trait for an importer.
pub trait DynImporter: Send + Sync {
    fn name(&self) -> &str;

    fn formats(&self) -> &[&str];

    fn extensions(&self) -> &[&str];

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

impl<T> DynImporter for T
where
    T: treasury_import::Importer,
{
    fn name(&self) -> &str {
        treasury_import::Importer::name(self)
    }

    fn formats(&self) -> &[&str] {
        treasury_import::Importer::formats(self)
    }

    fn extensions(&self) -> &[&str] {
        treasury_import::Importer::extensions(self)
    }

    fn target(&self) -> &str {
        treasury_import::Importer::target(self)
    }

    fn import(
        &self,
        source: &Path,
        output: &Path,
        sources: &mut dyn Sources,
        dependencies: &mut dyn Dependencies,
    ) -> Result<(), ImportError> {
        treasury_import::Importer::import(self, source, output, sources, dependencies)
    }
}

pub enum Importer {
    DynTraitImporter(Box<dyn DynImporter>),
    DylibImporter(DylibImporter),
}

impl fmt::Debug for Importer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Importer::DynTraitImporter(importer) => f.write_str(importer.name()),
            Importer::DylibImporter(importer) => fmt::Debug::fmt(importer, f),
        }
    }
}

impl Importer {
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
        match self {
            Importer::DynTraitImporter(importer) => {
                importer.import(source, output, &mut sources, &mut dependencies)
            }
            Importer::DylibImporter(importer) => {
                importer.import(source, output, sources, dependencies)
            }
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Importer::DynTraitImporter(importer) => importer.name(),
            Importer::DylibImporter(importer) => importer.name(),
        }
    }

    pub fn formats(&self) -> Cow<'_, [&str]> {
        match self {
            Importer::DynTraitImporter(importer) => importer.formats().into(),
            Importer::DylibImporter(importer) => importer.formats().into(),
        }
    }

    pub fn target(&self) -> &str {
        match self {
            Importer::DynTraitImporter(importer) => importer.target(),
            Importer::DylibImporter(importer) => importer.target(),
        }
    }

    pub fn extensions(&self) -> Cow<'_, [&str]> {
        match self {
            Importer::DynTraitImporter(importer) => importer.extensions().into(),
            Importer::DylibImporter(importer) => importer.extensions().into(),
        }
    }
}

unsafe fn load_dylib_importers(
    lib_path: &Path,
) -> eyre::Result<impl Iterator<Item = Importer> + '_> {
    Ok(dylib::load_importers(lib_path)?.map(Importer::DylibImporter))
}
