//! One transport record covers every dependency environment of a target build.
use super::*;
use crate::domain::package_spec::{SourcePackageFile, SourcePackageSpec};
use std::{collections::BTreeSet, path::Path};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputRecord {
    document: String,
    version: u32,
    recipe: String,
    target: String,
    inputs: PreparedInputs,
    payloads: BTreeSet<String>,
}

impl PreparedInputs {
    fn payloads(&self) -> BTreeSet<String> {
        self.root
            .iter()
            .chain(self.sysroot.iter())
            .chain(self.tests.iter().flat_map(|cases| cases.values()))
            .flat_map(|root| {
                root.packages
                    .values()
                    .flatten()
                    .map(|package| package.checksum.clone())
            })
            .collect()
    }

    /// Assembly receipts describe execution on one worker, not dependency inputs.
    pub fn portable(&self) -> Self {
        let portable = |root: &RootLock| {
            let mut lock = root.redacted();
            lock.root_digest = None;
            lock.rpmdb_digest = None;
            lock
        };
        Self {
            root: self.root.as_ref().map(portable),
            sysroot: self.sysroot.as_ref().map(portable),
            cross: self.cross.clone(),
            tests: self.tests.as_ref().map(|cases| {
                cases
                    .iter()
                    .map(|(key, root)| (key.clone(), portable(root)))
                    .collect()
            }),
        }
    }

    pub fn record(&self, recipe: &str, target: &str) -> Result<Vec<u8>, Error> {
        if self.root.is_none() {
            return Err(Error::new(
                codes::SRPM_006,
                "prepared build inputs require a compilation root",
            ));
        }
        let inputs = self.portable();
        serde_json::to_vec_pretty(&InputRecord {
            document: "rpmx-build-inputs".into(),
            version: 1,
            recipe: recipe.into(),
            target: target.into(),
            payloads: inputs.payloads(),
            inputs,
        })
        .map_err(record_error)
    }

    pub fn read_record(text: &str, recipe: &str, target: &str) -> Result<Self, Error> {
        let record: InputRecord = serde_json::from_str(text).map_err(record_error)?;
        if record.document != "rpmx-build-inputs"
            || record.version != 1
            || record.recipe != recipe
            || record.target != target
        {
            return Err(Error::new(
                codes::SRPM_006,
                "prepared inputs do not match this recipe, target or supported record version",
            ));
        }
        let inputs = record.inputs;
        if inputs.root.is_none() {
            return Err(Error::new(
                codes::SRPM_006,
                "prepared build inputs require a compilation root",
            ));
        }
        for root in inputs
            .root
            .iter()
            .chain(inputs.sysroot.iter())
            .chain(inputs.tests.iter().flat_map(|cases| cases.values()))
        {
            if root.root_digest.is_some() || root.rpmdb_digest.is_some() {
                return Err(Error::new(
                    codes::SRPM_006,
                    "prepared inputs contain local assembly receipts",
                ));
            }
        }
        let arch = Arch::parse(target)?;
        if let Some(root) = &inputs.root {
            root.validate()?;
            root.require_target(inputs.exec(arch)?.as_str())?;
        }
        if let Some(root) = &inputs.sysroot {
            root.validate()?;
            root.require_target(target)?;
        }
        for root in inputs.tests.iter().flat_map(|cases| cases.values()) {
            root.validate()?;
            source_tests::verify_extension(
                inputs.root.as_ref().ok_or_else(|| {
                    Error::new(
                        codes::TEST_005,
                        "test environment has no pinned compilation root",
                    )
                })?,
                root,
            )?;
        }
        if inputs.payloads() != record.payloads {
            return Err(Error::new(
                codes::SRPM_006,
                "prepared input payload inventory differs from its environments",
            ));
        }
        Ok(inputs)
    }

    pub fn export(
        &self,
        output: &Path,
        recipe: &str,
        target: &str,
    ) -> Result<std::path::PathBuf, Error> {
        let directory = output.join("inputs");
        std::fs::create_dir_all(&directory).map_err(record_error)?;
        let path = directory.join(format!("{target}.json"));
        std::fs::write(&path, self.record(recipe, target)?).map_err(record_error)?;
        Ok(path)
    }

    pub fn embed(
        &self,
        package: &mut SourcePackageSpec,
        recipe: &str,
        target: &str,
    ) -> Result<(), Error> {
        let path = "build-inputs.json";
        if package.files.iter().any(|file| file.path == path) {
            return Err(Error::new(
                codes::SRPM_007,
                "source file uses reserved SRPM name build-inputs.json",
            ));
        }
        let data = self.record(recipe, target)?;
        let reference =
            serde_json::json!({"path": path, "digest": crate::domain::hash::sha512_hex(&data)});
        let lock = package
            .files
            .iter_mut()
            .find(|file| file.path == "package.lock.json")
            .expect("source package lock");
        let mut document: serde_json::Value =
            serde_json::from_slice(&lock.data).map_err(record_error)?;
        document["inputs"] = reference;
        lock.data = serde_json::to_vec_pretty(&document).map_err(record_error)?;
        let digest = crate::domain::hash::sha512_hex(&lock.data);
        let manifest = package
            .files
            .iter_mut()
            .find(|file| file.path == "RPMX-MANIFEST.json")
            .expect("source manifest");
        let mut document: serde_json::Value =
            serde_json::from_slice(&manifest.data).map_err(record_error)?;
        document["lock"]["digest"] = digest.into();
        document["sources"]
            .as_array_mut()
            .expect("source list")
            .push(serde_json::json!({
                "path": path, "digest": crate::domain::hash::sha512_hex(&data), "size": data.len()
            }));
        manifest.data = serde_json::to_vec_pretty(&document).map_err(record_error)?;
        package.files.push(SourcePackageFile {
            path: path.into(),
            data,
            mode: 0o644,
        });
        Ok(())
    }
}

fn record_error(error: impl std::fmt::Display) -> Error {
    Error::new(codes::SRPM_006, error.to_string())
}
