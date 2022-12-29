use std::{fs::File, path::Path};

use treasury_import::{
    make_treasury_importers_library, Dependencies, ImportError, Importer, Sources,
};

struct FooImporter;

impl Importer for FooImporter {
    fn name(&self) -> &str {
        "Foo importer"
    }

    fn formats(&self) -> &[&str] {
        &["foo"]
    }

    fn target(&self) -> &str {
        "foo"
    }

    fn extensions(&self) -> &[&str] {
        &["json"]
    }

    fn import(
        &self,
        source: &Path,
        output: &Path,
        _sources: &mut dyn Sources,
        _dependencies: &mut dyn Dependencies,
    ) -> Result<(), ImportError> {
        let mut src = match File::open(source) {
            Ok(f) => f,
            Err(err) => {
                return Err(ImportError::Other {
                    reason: format!(
                        "Failed to open source file '{}'. {:#}",
                        source.display(),
                        err
                    ),
                })
            }
        };

        let mut dst = match File::create(output) {
            Ok(f) => f,
            Err(err) => {
                return Err(ImportError::Other {
                    reason: format!(
                        "Failed to open output file '{}'. {:#}",
                        output.display(),
                        err
                    ),
                })
            }
        };

        let value: serde_json::Value = match serde_json::from_reader(&mut src) {
            Ok(value) => value,
            Err(err) => {
                return Err(ImportError::Other {
                    reason: format!("Failed to read json from '{}'. {:#}", source.display(), err),
                })
            }
        };

        match serde_json::to_writer_pretty(&mut dst, &value) {
            Ok(()) => Ok(()),
            Err(err) => Err(ImportError::Other {
                reason: format!("Failed to write json to '{}'. {:#}", output.display(), err),
            }),
        }
    }
}

make_treasury_importers_library! {
    &FooImporter;
}
