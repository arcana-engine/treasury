use std::{fmt, path::Path};

use hashbrown::{hash_map::RawEntryMut, HashMap};
use smallvec::SmallVec;
use treasury_id::AssetId;
use treasury_import::ImportResult;

use self::dylib::DylibImporter;

mod dylib;

#[derive(Debug, thiserror::Error)]
#[error("Multiple importers may import from different formats '{formats:?}' to target '{target}'")]
pub struct CannotDecideOnImporter {
    formats: Vec<String>,
    target: String,
}

pub struct Importers {
    map: HashMap<String, HashMap<String, Importer>>,
}

impl Importers {
    pub fn new() -> Self {
        Importers {
            map: HashMap::new(),
        }
    }

    /// Loads importers from dylib.
    /// There is no possible way to guarantee that dylib does not break safety contracts.
    /// Some measures to ensure safety are taken.
    /// Providing dylib from which importers will be successfully imported and then cause an UB should possible only on purpose.
    pub unsafe fn load_dylib_importers(&mut self, lib_path: &Path) -> eyre::Result<()> {
        let map = load_dylib_importers(lib_path)?;

        for (format, target, importer) in map {
            let exts = importer.extensions().collect::<Vec<_>>();

            tracing::info!(
                "Registering importer '{}' -> '{}' {:?}",
                format,
                target,
                exts
            );

            match self.map.raw_entry_mut().from_key(&target) {
                RawEntryMut::Vacant(entry) => {
                    entry
                        .insert(target, HashMap::new())
                        .1
                        .insert(format, importer);
                }
                RawEntryMut::Occupied(entry) => {
                    match entry.into_mut().raw_entry_mut().from_key(&format) {
                        RawEntryMut::Vacant(entry2) => {
                            entry2.insert(format, importer);
                        }
                        RawEntryMut::Occupied(entry2) => {
                            tracing::error!(
                                "'{} -> {}' importer already registered: {:#?}",
                                format,
                                target,
                                entry2.get(),
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn get(&self, format: &str, target: &str) -> Option<&Importer> {
        let map = self.map.get(target)?;
        map.get(format)
    }

    /// Try to guess importer by file extension and target
    pub fn guess(
        &self,
        ext: Option<&str>,
        target: &str,
    ) -> Result<Option<&Importer>, CannotDecideOnImporter> {
        tracing::debug!(
            "Guessing importer for '{:?}' to '{}'",
            ext.unwrap_or(""),
            target
        );

        let map = self.map.get(target);
        match map {
            None => Ok(None),
            Some(map) => match ext {
                None => match map.len() {
                    0 => {
                        tracing::debug!("No importers to '{}' found", target);
                        Ok(None)
                    }
                    1 => Ok(map.values().next()),
                    _ => {
                        tracing::debug!("Multiple importers to '{}' found", target);
                        Err(CannotDecideOnImporter {
                            target: target.to_owned(),
                            formats: map.keys().cloned().collect(),
                        })
                    }
                },
                Some(ext) => {
                    let mut formats = SmallVec::<[_; 4]>::new();
                    for (format, importer) in map.iter() {
                        if importer.supports_extension(ext) {
                            formats.push(format);
                        }
                    }

                    match formats.len() {
                        0 => {
                            tracing::debug!("No importers to '{}' found", target);
                            Ok(None)
                        }
                        1 => Ok(map.get(formats[0])),
                        _ => {
                            tracing::debug!("Multiple importers to '{}' found", target);
                            Err(CannotDecideOnImporter {
                                target: target.to_owned(),
                                formats: formats.into_iter().cloned().collect(),
                            })
                        }
                    }
                }
            },
        }
    }
}

pub enum Importer {
    DylibImporter(DylibImporter),
}

impl fmt::Debug for Importer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Importer::DylibImporter(importer) => fmt::Debug::fmt(importer, f),
        }
    }
}

impl Importer {
    pub fn import<'a, S, D>(
        &self,
        source: &Path,
        output: &Path,
        sources: S,
        dependencies: D,
    ) -> ImportResult
    where
        S: Fn(&str) -> Option<&'a Path> + 'a,
        D: Fn(&str, Option<&str>, &str) -> Option<AssetId>,
    {
        match self {
            Importer::DylibImporter(importer) => {
                importer.import(source, output, sources, dependencies)
            }
        }
    }

    // pub fn name(&self) -> &str {
    //     match self {
    //         Importer::DylibImporter(importer) => importer.name(),
    //     }
    // }

    pub fn format(&self) -> &str {
        match self {
            Importer::DylibImporter(importer) => importer.format(),
        }
    }

    // pub fn target(&self) -> &str {
    //     match self {
    //         Importer::DylibImporter(importer) => importer.target(),
    //     }
    // }

    pub fn extensions(&self) -> impl Iterator<Item = &str> + '_ {
        match self {
            Importer::DylibImporter(importer) => importer.extensions(),
        }
    }

    pub fn supports_extension(&self, ext: &str) -> bool {
        match self {
            Importer::DylibImporter(importer) => importer.extensions().any(|e| e == ext),
        }
    }
}

unsafe fn load_dylib_importers(
    lib_path: &Path,
) -> eyre::Result<impl Iterator<Item = (String, String, Importer)> + '_> {
    Ok(dylib::load_importers(lib_path)?
        .map(|(format, target, importer)| (format, target, Importer::DylibImporter(importer))))
}
