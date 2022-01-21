use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use eyre::WrapErr;
use hashbrown::HashMap;
use importer::Importers;
use meta::{AssetMeta, SourceMeta};
use parking_lot::RwLock;
use sources::Sources;
use temp::Temporaries;
use treasury_id::{AssetId, AssetIdContext};
use treasury_import::ImportError;
use url::Url;

mod importer;
mod meta;
mod sha256;
mod sources;
mod temp;

const TREASURY_META_NAME: &'static str = "Treasury.toml";
const DEFAULT_AUX: &'static str = "treasury";
const DEFAULT_ARTIFACTS: &'static str = "artifacts";
const DEFAULT_EXTERNAL: &'static str = "external";
const MAX_ITEM_ATTEMPTS: u32 = 1024;

#[derive(serde::Serialize, serde::Deserialize)]
struct TreasuryInfo {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    artifacts: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    external: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    temp: Option<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    importers: Vec<PathBuf>,
}

fn write_treasury_info(meta: &TreasuryInfo, path: &Path) -> eyre::Result<()> {
    let meta = toml::to_string_pretty(meta).wrap_err("Failed to serialize metadata")?;
    std::fs::write(path, &meta)
        .wrap_err_with(|| format!("Failed to write metadata file '{}'", path.display()))?;
    Ok(())
}

fn read_treasury_info(path: &Path) -> eyre::Result<TreasuryInfo> {
    let err_ctx = || format!("Failed to read metadata file '{}'", path.display());

    let meta = std::fs::read(path).wrap_err_with(err_ctx)?;
    let meta: TreasuryInfo = toml::from_slice(&meta).wrap_err_with(err_ctx)?;

    Ok(meta)
}

pub struct Treasury {
    id_ctx: AssetIdContext,
    base: PathBuf,
    base_url: Url,
    artifacts_base: PathBuf,
    external: PathBuf,
    temp: PathBuf,
    importers: Importers,

    artifacts: RwLock<HashMap<AssetId, PathBuf>>,
    scanned: RwLock<bool>,
}

impl Treasury {
    #[tracing::instrument]
    pub fn init() -> eyre::Result<Self> {
        let cwd = std::env::current_dir().wrap_err("Failed to get current directory")?;
        Treasury::init_in(&cwd, None, None, None, &[])
    }

    #[tracing::instrument]
    pub fn init_in(
        base: &Path,
        artifacts: Option<&Path>,
        external: Option<&Path>,
        temp: Option<&Path>,
        importers: &[&Path],
    ) -> eyre::Result<Self> {
        eyre::ensure!(
            base.is_dir(),
            "Failed to initialize treasury at '{}'. Not a directory",
            base.display()
        );

        let meta_path = base.join(TREASURY_META_NAME);
        eyre::ensure!(
            !meta_path.exists(),
            "Failed to initialize treasury at '{}', '{}' already exists",
            base.display(),
            meta_path.display(),
        );

        let artifacts = artifacts.map(Path::to_owned);
        let external = external.map(Path::to_owned);
        let temp = temp.map(Path::to_owned);
        let importers = importers.iter().copied().map(|p| p.to_owned()).collect();

        let meta = TreasuryInfo {
            artifacts,
            external,
            temp,
            importers,
        };

        write_treasury_info(&meta, &meta_path)?;
        Treasury::open(&meta_path)
    }

    /// Find and open treasury in ancestors of specified directory.
    #[tracing::instrument]
    pub fn find_from(path: &Path) -> eyre::Result<Self> {
        let meta_path = find_treasury_info(path).ok_or_else(|| {
            eyre::eyre!(
                "Failed to find `Treasury.toml` in ancestors of {}",
                path.display(),
            )
        })?;

        Treasury::open(&meta_path)
    }

    /// Find and open treasury in ancestors of current directory.
    #[tracing::instrument]
    pub fn find() -> eyre::Result<Self> {
        let cwd = std::env::current_dir().wrap_err("Failed to get current directory")?;
        Treasury::find_from(&cwd)
    }

    /// Open treasury database at specified path.
    #[tracing::instrument]
    pub fn open(path: &Path) -> eyre::Result<Self> {
        let path = dunce::canonicalize(path).wrap_err_with(|| {
            eyre::eyre!("Failed to canonicalize base path '{}'", path.display())
        })?;

        let meta = read_treasury_info(&path)?;

        let base = path.parent().unwrap().to_owned();
        let base_url = Url::from_directory_path(&base)
            .map_err(|()| eyre::eyre!("'{}' is invalid base path", base.display()))?;

        let artifacts = base.join(
            meta.artifacts
                .unwrap_or_else(|| Path::new(DEFAULT_AUX).join(DEFAULT_ARTIFACTS)),
        );

        let external = base.join(
            meta.external
                .unwrap_or_else(|| Path::new(DEFAULT_AUX).join(DEFAULT_EXTERNAL)),
        );

        let temp = meta
            .temp
            .map_or_else(std::env::temp_dir, |path| base.join(path));

        let id_ctx = AssetIdContext::new(treasury_epoch(), rand::random());

        let mut importers = Importers::new();

        for lib_path in &meta.importers {
            let lib_path = base.join(lib_path);

            unsafe {
                // # Safety: Nope.
                // There is no way to make this safe.
                // But it is unlikely to cause problems by accident.
                if let Err(err) = importers.load_dylib_importers(&lib_path) {
                    tracing::error!(
                        "Failed to load importers from '{}'. {:#}",
                        lib_path.display(),
                        err
                    );
                }
            }
        }

        Ok(Treasury {
            id_ctx,
            base,
            base_url,
            artifacts_base: artifacts,
            external,
            temp,
            importers,
            artifacts: RwLock::new(HashMap::new()),
            scanned: RwLock::new(false),
        })
    }

    /// Loads importers from dylib.
    /// There is no possible way to guarantee that dylib does not break safety contracts.
    /// Some measures to ensure safety are taken.
    /// Providing dylib from which importers will be successfully loaded and then cause an UB should possible only on purpose.
    #[tracing::instrument(skip(self))]
    pub unsafe fn register_importers_lib(&mut self, lib_path: &Path) -> eyre::Result<()> {
        self.importers.load_dylib_importers(lib_path)
    }

    /// Import an asset.
    #[tracing::instrument(skip(self))]
    pub async fn store(
        &self,
        source: &str,
        format: Option<&str>,
        target: &str,
    ) -> eyre::Result<AssetId> {
        let source = self.base_url.join(source).wrap_err_with(|| {
            format!(
                "Failed to construct URL from base '{}' and source '{}'",
                self.base_url, source
            )
        })?;

        let mut temporaries = Temporaries::new(&self.temp);
        let mut sources = Sources::new();

        let base = &self.base;
        let artifacts = &self.artifacts_base;
        let external = &self.external;
        let importers = &self.importers;

        struct StackItem {
            source: Url,
            format: Option<String>,
            target: String,
            attempt: u32,
        }

        let mut stack = Vec::new();
        stack.push(StackItem {
            source,
            format: format.map(str::to_owned),
            target: target.to_owned(),
            attempt: 0,
        });

        loop {
            // tokio::time::sleep(Duration::from_secs(1)).await;

            let item = stack.last_mut().unwrap();
            item.attempt += 1;

            let mut meta = SourceMeta::new(&item.source, &self.base, &self.external)
                .wrap_err("Failed to fetch source meta")?;

            if let Some(asset) = meta.get_asset(&item.target) {
                tracing::debug!(
                    "'{}' '{:?}' '{}' was already imported",
                    item.source,
                    item.format,
                    item.target
                );

                stack.pop().unwrap();
                if stack.is_empty() {
                    return Ok(asset.id());
                }
                continue;
            }

            let importer = match &item.format {
                None => importers.guess(url_ext(&item.source), &item.target)?,
                Some(format) => importers.get(format, &item.target),
            };

            let importer = importer.ok_or_else(|| {
                eyre::eyre!(
                    "Failed to find importer '{} -> {}' for asset '{}'",
                    item.format.as_deref().unwrap_or("<undefined>"),
                    item.target,
                    item.source,
                )
            })?;

            // let format = importer.format();

            // Fetch source file.
            let source_path = sources
                .fetch(&mut temporaries, &item.source)
                .await?
                .to_owned();
            let output_path = temporaries.make_temporary();

            let result = importer.import(
                &source_path,
                &output_path,
                |src: &str| {
                    let src = item.source.join(src).ok()?;
                    // If parsing fails - source will be listed in `ImportResult::RequireSources`
                    // it will fail there again, aborting importing process.
                    // Otherwise it was not important ^_^
                    sources.get(&src)
                },
                |src: &str, target: &str| {
                    let src = item.source.join(src).ok()?;

                    match SourceMeta::new(&src, base, external) {
                        Ok(meta) => {
                            let asset = meta.get_asset(target)?;
                            Some(asset.id())
                        }
                        Err(err) => {
                            tracing::error!("Fetching dependency failed. {:#}", err);
                            None
                        }
                    }
                },
            );

            match result {
                Ok(()) => {}
                Err(ImportError::Other { reason }) => {
                    return Err(eyre::eyre!(
                        "Failed to import {}:{}->{}. {}",
                        item.source,
                        importer.format(),
                        item.target,
                        reason,
                    ))
                }
                Err(ImportError::RequireSources { sources: srcs }) => {
                    if item.attempt >= MAX_ITEM_ATTEMPTS {
                        return Err(eyre::eyre!(
                            "Failed to import {}:{}->{}. Too many attempts",
                            item.source,
                            importer.format(),
                            item.target,
                        ));
                    }

                    let source = item.source.clone();
                    for src in srcs {
                        match source.join(&src) {
                            Err(err) => {
                                return Err(eyre::eyre!(
                                    "Failed to join URL '{}' with '{}'. {:#}",
                                    source,
                                    src,
                                    err,
                                ))
                            }
                            Ok(url) => {
                                sources.fetch(&mut temporaries, &url).await?;
                            }
                        };
                    }
                    continue;
                }
                Err(ImportError::RequireDependencies { dependencies }) => {
                    if item.attempt >= MAX_ITEM_ATTEMPTS {
                        return Err(eyre::eyre!(
                            "Failed to import {}:{}->{}. Too many attempts",
                            item.source,
                            importer.format(),
                            item.target,
                        ));
                    }

                    let source = item.source.clone();
                    for dep in dependencies.into_iter() {
                        match source.join(&dep.source) {
                            Err(err) => {
                                return Err(eyre::eyre!(
                                    "Failed to join URL '{}' with '{}'. {:#}",
                                    source,
                                    dep.source,
                                    err,
                                ))
                            }
                            Ok(url) => {
                                stack.push(StackItem {
                                    source: url,
                                    format: None,
                                    target: dep.target,
                                    attempt: 0,
                                });
                            }
                        };
                    }
                    continue;
                }
            }

            if !artifacts.exists() {
                std::fs::create_dir_all(artifacts).wrap_err_with(|| {
                    format!(
                        "Failed to create artifacts directory '{}'",
                        artifacts.display()
                    )
                })?;

                if let Err(err) = std::fs::write(artifacts.join(".gitignore"), "*") {
                    tracing::error!(
                        "Failed to place .gitignore into artifacts directory. {:#}",
                        err
                    );
                }
            }

            let new_id = self.id_ctx.generate();

            let asset = AssetMeta::new(new_id, &output_path, artifacts)
                .wrap_err("Failed to prepare new asset")?;

            let item = stack.pop().unwrap();

            meta.add_asset(item.target, asset, base, external)?;

            if stack.is_empty() {
                return Ok(new_id);
            }
        }
    }

    /// Fetch asset data path.
    pub fn fetch(&self, id: AssetId) -> Option<PathBuf> {
        let scanned = self.scanned.read();
        if *scanned {
            self.artifacts.read().get(&id).cloned()
        } else {
            drop(scanned);

            let artifacts = self.artifacts.read();
            if artifacts.contains_key(&id) {
                artifacts.get(&id).cloned()
            } else {
                let mut new_artifacts = Vec::new();
                let mut scanned = self.scanned.write();

                if *scanned {
                    artifacts.get(&id).cloned()
                } else {
                    scan_local(
                        &self.base,
                        &self.artifacts_base,
                        &artifacts,
                        &mut new_artifacts,
                    );
                    scan_external(
                        &self.external,
                        &self.artifacts_base,
                        &artifacts,
                        &mut new_artifacts,
                    );

                    drop(artifacts);
                    let mut artifacts = self.artifacts.write();
                    for (new_id, path) in new_artifacts {
                        artifacts.insert(new_id, path);
                    }

                    *scanned = true;
                    let path = artifacts.get(&id).cloned();

                    drop(artifacts);
                    drop(scanned);

                    path
                }
            }
        }
    }

    /// Fetch asset data path.
    pub async fn find_asset(&self, source: &str, target: &str) -> eyre::Result<Option<AssetId>> {
        let source_url = self.base_url.join(source).wrap_err_with(|| {
            format!(
                "Failed to construct URL from base '{}' and source '{}'",
                self.base_url, source
            )
        })?;

        let meta = SourceMeta::new(&source_url, &self.base, &self.external)
            .wrap_err("Failed to fetch source meta")?;

        match meta.get_asset(target) {
            None => match self.store(source, None, target).await {
                Err(err) => {
                    tracing::warn!(
                        "Failed to store '{}' as '{}' on lookup. {:#}",
                        source,
                        target,
                        err
                    );
                    Ok(None)
                }
                Ok(id) => Ok(Some(id)),
            },
            Some(asset) => Ok(Some(asset.id())),
        }
    }
}

pub fn find_treasury_info(mut path: &Path) -> Option<PathBuf> {
    loop {
        let candidate = path.join(TREASURY_META_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        path = path.parent()?;
    }
}

fn treasury_epoch() -> SystemTime {
    /// Starting point of treasury epoch relative to unix epoch in seconds.
    const TREASURY_EPOCH_FROM_UNIX: u64 = 1609448400;

    SystemTime::UNIX_EPOCH + Duration::from_secs(TREASURY_EPOCH_FROM_UNIX)
}

fn url_ext(url: &Url) -> Option<&str> {
    let path = url.path();
    let dot = path.rfind('.')?;
    let sep = path.rfind('/')?;
    if dot == path.len() || dot <= sep + 1 {
        None
    } else {
        Some(&path[dot + 1..])
    }
}

fn scan_external(
    external: &Path,
    artifacts_base: &Path,
    existing_artifacts: &HashMap<AssetId, PathBuf>,
    artifacts: &mut Vec<(AssetId, PathBuf)>,
) {
    match std::fs::read_dir(&external) {
        Err(err) => tracing::error!(
            "Failed to scan directory '{}'. {:#}",
            external.display(),
            err
        ),
        Ok(dir) => {
            for e in dir {
                match e {
                    Err(err) => tracing::error!(
                        "Failed to read entry in directory '{}'. {:#}",
                        external.display(),
                        err,
                    ),
                    Ok(e) => {
                        let name = e.file_name();
                        let path = external.join(&name);
                        match e.file_type() {
                            Err(err) => {
                                tracing::error!("Failed to check '{}'. {:#}", path.display(), err)
                            }
                            Ok(ft) => match () {
                                () if ft.is_file() && !SourceMeta::is_local_meta_path(&path) => {
                                    match SourceMeta::open_external(&path) {
                                        Err(err) => {
                                            tracing::error!(
                                                "Failed to scan meta file '{}'. {:#}",
                                                path.display(),
                                                err
                                            );
                                        }
                                        Ok(meta) => {
                                            for asset in meta.assets() {
                                                if !existing_artifacts.contains_key(&asset.id()) {
                                                    artifacts.push((
                                                        asset.id(),
                                                        asset.artifact_path(artifacts_base),
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            },
                        }
                    }
                }
            }
        }
    }
}

fn scan_local(
    base: &Path,
    artifacts_base: &Path,
    existing_artifacts: &HashMap<AssetId, PathBuf>,
    artifacts: &mut Vec<(AssetId, PathBuf)>,
) {
    debug_assert!(base.is_absolute());

    let mut queue = VecDeque::new();
    queue.push_back(base.to_owned());

    while let Some(dir_path) = queue.pop_front() {
        match std::fs::read_dir(&dir_path) {
            Err(err) => tracing::error!(
                "Failed to scan directory '{}'. {:#}",
                dir_path.display(),
                err
            ),
            Ok(dir) => {
                for e in dir {
                    match e {
                        Err(err) => tracing::error!(
                            "Failed to read entry in directory '{}'. {:#}",
                            dir_path.display(),
                            err,
                        ),
                        Ok(e) => {
                            let name = e.file_name();
                            let path = dir_path.join(&name);
                            match e.file_type() {
                                Err(err) => {
                                    tracing::error!(
                                        "Failed to check '{}'. {:#}",
                                        path.display(),
                                        err
                                    )
                                }
                                Ok(ft) => match () {
                                    () if ft.is_dir() => {
                                        queue.push_back(path);
                                    }
                                    () if ft.is_file() && SourceMeta::is_local_meta_path(&path) => {
                                        match SourceMeta::open_local(&path) {
                                            Err(err) => {
                                                tracing::error!(
                                                    "Failed to scan meta file '{}'. {:#}",
                                                    path.display(),
                                                    err
                                                );
                                            }
                                            Ok(mut meta) => {
                                                for asset in meta.assets_mut() {
                                                    if !existing_artifacts.contains_key(&asset.id())
                                                    {
                                                        artifacts.push((
                                                            asset.id(),
                                                            asset.artifact_path(artifacts_base),
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    () => {}
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}
