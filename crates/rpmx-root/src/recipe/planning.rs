use std::path::{Path, PathBuf};

use rpmx_base::Error;
use rpmx_language::{load_recipe_bundle, plan_with_options, Plan, PlanOptions, RecipeBundle};

use rpmx_language::plan::PlanPolicy;

pub fn step_root(pipeline_dir: Option<&Path>, recipe: &Path) -> Option<PathBuf> {
    pipeline_dir
        .map(Path::to_path_buf)
        .or_else(|| recipe.parent().map(|parent| parent.join("pipelines")))
}

pub fn load_plan(
    file: &Path,
    arch: &str,
    pipeline_dir: Option<&Path>,
    allow_builtin_override: bool,
    // A recipe extracted from an SRPM keeps its `build.root.profile` path from
    // the build host, which does not exist here. The SRPM lock is the
    // authority, so profile discovery is skipped.
    discover_profile: bool,
    policy: PlanPolicy<'_>,
) -> Result<(RecipeBundle, Plan), Error> {
    let bundle = load_recipe_bundle(file)?;
    let root = step_root(pipeline_dir, &bundle.root);
    let discovered_tag = if policy.dist_tag.is_none() && discover_profile {
        super::profile_dist_tag(&bundle.recipe, &bundle.root)?
    } else {
        None
    };
    let dist_tag = policy.dist_tag.or(discovered_tag.as_deref());
    let plan = plan_with_options(
        &bundle.recipe,
        PlanOptions {
            arch,
            step_root: root.as_deref(),
            allow_builtin_override,
            dist_tag,
            build_release: policy.build_release,
        },
    )?;
    Ok((bundle, plan))
}

#[cfg(test)]
mod tests {
    use super::load_plan;
    use rpmx_language::plan::PlanPolicy;

    const RECIPE: &str = r#"
schema: 2
package:
  name: demo
  version: "1"
  release: 1
  summary: demo
  license: MIT
build:
  root:
    profile: missing.profile.yaml
outputs:
  - name: demo
    metapackage: true
"#;

    #[test]
    fn an_extracted_recipe_does_not_rediscover_its_profile() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("package.yaml");
        std::fs::write(&path, RECIPE).expect("recipe");
        let policy = PlanPolicy {
            dist_tag: None,
            build_release: None,
        };

        // A normal build discovers the profile and fails when it is missing.
        let error = load_plan(&path, "x86_64", None, false, true, policy)
            .expect_err("a missing profile is an error for a real recipe");
        assert_eq!(error.code, "RPMX-ROOT-006");

        // A recipe extracted from an SRPM keeps a path from the build host;
        // the embedded lock is the authority, so planning must not look for it.
        let (_, plan) = load_plan(&path, "x86_64", None, false, false, policy).expect("plan");
        assert_eq!(plan.cells.len(), 1);
    }
}
