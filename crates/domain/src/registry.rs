//! Registry URL helpers.

use url::Url;

use crate::name::PackageName;
use crate::package::PackageId;

/// Build the packument metadata URL for a package name.
pub fn package_metadata_url(registry: &Url, name: &PackageName) -> anyhow::Result<Url> {
    let mut registry = registry.clone();
    if !registry.path().ends_with('/') {
        let path = format!("{}/", registry.path());
        registry.set_path(&path);
    }

    let encoded_name = name.as_str().replace('/', "%2f");
    Ok(registry.join(&encoded_name)?)
}

/// Build the conventional npm tarball URL for a package.
pub fn default_tarball_url(registry: &Url, id: &PackageId) -> anyhow::Result<Url> {
    let mut registry = registry.clone();
    if !registry.path().ends_with('/') {
        let path = format!("{}/", registry.path());
        registry.set_path(&path);
    }

    let unscoped = id.name.unscoped();
    let path = format!("{}/-/{}-{}.tgz", id.name.as_str(), unscoped, id.version);
    Ok(registry.join(&path)?)
}
