# Treasury

[![crates](https://img.shields.io/crates/v/treasury.svg?style=for-the-badge&label=treasury)](https://crates.io/crates/treasury)
[![docs](https://img.shields.io/badge/docs.rs-treasury-66c2a5?style=for-the-badge&labelColor=555555&logoColor=white)](https://docs.rs/treasury)
[![actions](https://img.shields.io/github/workflow/status/arcana-engine/treasury/badge/master?style=for-the-badge)](https://github.com/arcana-engine/treasury/actions?query=workflow%3ARust)
[![MIT/Apache](https://img.shields.io/badge/license-MIT%2FApache-blue.svg?style=for-the-badge)](COPYING)
![loc](https://img.shields.io/tokei/lines/github/arcana-engine/treasury?style=for-the-badge)

Treasury is easy to use set of libraries and tools for creating asset pipelines for game engines or other applications.

## Usage

### Initialization

To start using Treasury an instance must be created.
Treasury instance is defined by `Treasury.toml` file.
Parent directory of the file is called "Base directory".

This file can be created manually or using methods below:\
* CLI tool `treasury init` will initialize Treasury instance using current directory as base.\
`treasury --base <path> init` will initialize Treasury instance with base directory `<path>`.
* Client library API provides method `Client::local`. With `init` argument set to `true` it will initialize Treasury. This is internally used by CLI call above.


Default `Treasury.toml` file looks like this
```
```
Yes, empty file.


There are four fields that can be overridden.

1. ```
   artifacts = "<path>"
   ```
   will override artifacts directory to specified path relative to `<base>`. Defaults to `<base>/treasury/artifacts`

   Artifacts directory is where all artifacts are stored. This can and **SHOULD NOT** be covered by VCS. If path is inside repository then it should be ignored.
   If Treasury creates artifacts directory (when storing an artifact and directory does not exist) it will create .gitignore file with "*".

2. ```
   external = "<path>"
   ```
   will override external directory to specified path relative to `<base>`. Defaults to `<base>/treasury/external`

   External directory is where all metadata files for remote assets are stored.
   This directory **SHOULD** be in repository and not ignored by VCS.

3. ```
   temp = "<path>"
   ```
   will override default directory for temporary files. Defaults to result of `std::env::temp_dir()`.
   Temporary files are used as intermediate storage for sources downloaded for importers to consume and for importers output.

4. ```
   importers = ["<list>", "<of>", "<paths>"]
   ```
   will tell what importer libraries that should be used for this instance.\
   For Rust projects they will typically reside in target directory of the cargo workspace.

Once initialized Treasury instance can be used to store and fetch assets.

### Storing

Assets can be stored and processed by Treasury
The usage is straightforward.

1. User provides asset source, target format that will be used by end application and optionally source format. Source format required only if ambiguous.
2. A registered that matches source and target formats runs and processes asset into artifact.
3. Treasury stores resulting artifact. It avoids storing duplicates though. Different assets may point to the same artifact.
4. AssetId is returned.


### Fetching

User can fetch artifacts of stored assets using asset source and target format.
Or `AssetId`. Artifacts should always use `AssetId`.
When asset sources migrate, .treasury file should come along. In this case reimporting would not be required and their `AssetId` is preserved.

## Importers

In order to store assets an importer is required to transform asset source into an artifact.

Importers are types that implement `treasury_import::Importer` traits.
Treasury can be configured to load importers from dynamic libraries.

To simplify writing of such library and minimize problems that can arise from invalid implementation `treasury_import::make_treasury_importers_library` macro should be used.\
This macro will export all necessary symbols that are expected by server.
It will ensure ABI compatibility using major version of `treasury_import` crate.
The macro an code it generates will do all the unsafe ops, leaving author of importers library with simple and 100% safe Rust.


Artifacts should always use `AssetId` to refer to dependencies.
Asset source file can contain path (relative to source file or absolute) or URL.
`Importer::import` takes a reference to a function that can converted path or URL and known target to `AssetId`.
If dependency is not found, `ImportResult::RequireDependencies

## What is missing?

Currently this project is bare-bone implementation of the asset pipeline.

* Packing is not yet implemented. There must be a way to pack subset of artifacts into package optimized for storing on disk and loading without indirections.
* Server is not ready to be used in remote mode. To prepare for that, server should be able to fetch local source data from client that requests store operation.
* Currently only `file:` and `data:` URLs are supported. This is enough for working with local assets.
* Hot-reloading is not yet possible as server does not watches for changes in sources.

## License

Licensed under either of

* Apache License, Version 2.0, ([license/APACHE](license/APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([license/MIT](license/MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
