use std::{
    fs::File,
    io::Write,
    mem::size_of_val,
    path::{Path, PathBuf},
};

use eyre::WrapErr;
use hashbrown::{hash_map::RawEntryMut, HashMap};
use url::Url;

use crate::temp::Temporaries;

/// Fetches and caches sources.
pub struct Sources {
    feched: HashMap<Url, PathBuf>,
}

impl Sources {
    pub fn new() -> Self {
        Sources {
            feched: HashMap::new(),
        }
    }

    pub fn get(&self, source: &Url) -> Option<&Path> {
        Some(self.feched.get(source)?)
    }

    pub async fn fetch(
        &mut self,
        temporaries: &mut Temporaries<'_>,
        source: &Url,
    ) -> eyre::Result<&Path> {
        match self.feched.raw_entry_mut().from_key(&source) {
            RawEntryMut::Occupied(entry) => Ok(entry.into_mut()),
            RawEntryMut::Vacant(entry) => match source.scheme() {
                "file" => {
                    let path = source
                        .to_file_path()
                        .map_err(|()| eyre::eyre!("Invalid file: URL"))?;

                    tracing::debug!("Fetching file '{}' ('{}')", source, path.display());
                    Ok(entry.insert(source.clone(), path).1)
                }
                "data" => {
                    let data_start = source.as_str()[size_of_val("data:")..]
                        .find(',')
                        .ok_or_else(|| eyre::eyre!("Invalid data URL"))?
                        + 1
                        + size_of_val("data:");
                    let data = &source.as_str()[data_start..];

                    let temp = temporaries.make_temporary();
                    let mut file = File::create(&temp)
                        .wrap_err("Failed to create temporary file to store data URL content")?;

                    if source.as_str()[..data_start].ends_with(";base64,") {
                        dbg!(data);

                        let decoded = base64::decode_config(data, base64::URL_SAFE_NO_PAD)
                            .wrap_err("Failed to decode base64 data url")?;

                        dbg!(&decoded);

                        file.write_all(&decoded).wrap_err_with(|| {
                            format!(
                                "Failed to write data URL content to temporary file '{}'",
                                temp.display(),
                            )
                        })?;
                    } else {
                        file.write_all(data.as_bytes()).wrap_err_with(|| {
                            format!(
                                "Failed to write data URL content to temporary file '{}'",
                                temp.display(),
                            )
                        })?;
                    }

                    Ok(entry.insert(source.clone(), temp).1)
                }
                schema => Err(eyre::eyre!("Unsupported schema '{}'", schema)),
            },
        }
    }
}
