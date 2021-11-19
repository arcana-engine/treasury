use std::{fs::File, path::Path};

use treasury_import::{
    make_treasury_importers_library, Dependencies, ImportResult, Importer, Sources,
};

struct FooImporter;

impl Importer for FooImporter {
    fn import(
        &self,
        source: &Path,
        output: &Path,
        _sources: &impl Sources,
        _dependencies: &impl Dependencies,
    ) -> ImportResult {
        let mut src = match File::open(source) {
            Ok(f) => f,
            Err(err) => {
                return ImportResult::Err {
                    reason: format!(
                        "Failed to open source file '{}'. {:#}",
                        source.display(),
                        err
                    ),
                }
            }
        };

        let mut dst = match File::create(output) {
            Ok(f) => f,
            Err(err) => {
                return ImportResult::Err {
                    reason: format!(
                        "Failed to open output file '{}'. {:#}",
                        output.display(),
                        err
                    ),
                }
            }
        };

        let value: serde_json::Value = match serde_json::from_reader(&mut src) {
            Ok(value) => value,
            Err(err) => {
                return ImportResult::Err {
                    reason: format!("Failed to read json from '{}'. {:#}", source.display(), err),
                }
            }
        };

        match serde_json::to_writer_pretty(&mut dst, &value) {
            Ok(()) => ImportResult::Ok,
            Err(err) => {
                return ImportResult::Err {
                    reason: format!("Failed to write json to '{}'. {:#}", output.display(), err),
                }
            }
        }
    }
}

make_treasury_importers_library! {
    [json] foo : foo -> foo = &FooImporter;
}
