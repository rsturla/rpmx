//! Writer-ready package models.
//!
//! These types are the single representation shared by stage preparation and
//! the RPM writer. Keeping them in core avoids a field-by-field conversion
//! between `prepare_*` and `rpmx-rpm`.

use crate::model::Scriptlets;

#[derive(Debug, Clone)]
pub struct BinaryPackageFile {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
    /// Target when this entry is a symbolic link; `data` is ignored then.
    pub target: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
    pub caps: Option<String>,
    pub config: bool,
    pub noreplace: bool,
    pub doc: bool,
    pub license: bool,
}

#[derive(Debug, Clone)]
pub struct BinaryPackageDirectory {
    pub path: String,
    pub mode: u32,
    pub user: Option<String>,
    pub group: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BinaryPackageSpec {
    pub name: String,
    pub version: String,
    pub release: String,
    pub epoch: Option<u32>,
    pub arch: String,
    pub summary: String,
    pub license: String,
    pub description: String,
    pub url: Option<String>,
    pub source_rpm: String,
    pub source_date_epoch: u32,
    pub files: Vec<BinaryPackageFile>,
    pub directories: Vec<BinaryPackageDirectory>,
    pub requires: Vec<String>,
    pub recommends: Vec<String>,
    pub suggests: Vec<String>,
    pub supplements: Vec<String>,
    pub enhances: Vec<String>,
    pub provides: Vec<String>,
    pub obsoletes: Vec<String>,
    pub conflicts: Vec<String>,
    pub scriptlets: Scriptlets,
}

#[derive(Debug, Clone)]
pub struct SourcePackageFile {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
}

#[derive(Debug, Clone)]
pub struct SourcePackageSpec {
    pub name: String,
    pub version: String,
    pub release: String,
    pub epoch: Option<u32>,
    pub arch: String,
    pub summary: String,
    pub license: String,
    pub description: String,
    pub source_date_epoch: u32,
    pub files: Vec<SourcePackageFile>,
}
