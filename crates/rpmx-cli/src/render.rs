use rpmx_core::{BootstrapPlan, Plan, PlannedStep};

pub fn lint(plan: &Plan) -> String {
    let ids = plan
        .cells
        .iter()
        .map(|cell| {
            if cell.id.is_empty() {
                "default".to_string()
            } else {
                cell.id.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("ok: {} cell(s): {ids}", plan.cells.len())
}

pub fn bootstrap_plan(plan: &BootstrapPlan) {
    match &plan.seed {
        Some(seed) => println!(
            "seed   name={} image={} releasever={} generation={}",
            seed.name.as_deref().unwrap_or("-"),
            seed.image.as_deref().unwrap_or("-"),
            seed.releasever.as_deref().unwrap_or("-"),
            seed.generation.as_deref().unwrap_or("-"),
        ),
        None => println!("seed   -"),
    }
    match plan.source_date_epoch {
        Some(epoch) => println!("epoch  {epoch}"),
        None => println!("epoch  -"),
    }
    for stage in &plan.stages {
        println!(
            "{}  profile={} arch={} inputs={}",
            stage.name,
            stage.profile,
            stage.arch,
            if stage.inputs.is_empty() {
                "-".to_string()
            } else {
                stage.inputs.join(",")
            }
        );
        println!("  root   {}", stage.root);
        println!("  lock   {}", stage.lock);
        println!("  repo   {}  id {}", stage.repo, stage.repo_id);
        if stage.seed {
            println!("  seed   true");
        }
        if !stage.extra_tools.is_empty() {
            println!(
                "  tools  {}",
                stage
                    .extra_tools
                    .iter()
                    .map(|package| format!("+{package}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        if !stage.extra_sysroot.is_empty() {
            println!(
                "  sysroot {}",
                stage
                    .extra_sysroot
                    .iter()
                    .map(|package| format!("+{package}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        if !stage.fixed_point.is_empty() {
            println!("  fixed  {}", stage.fixed_point.join(", "));
        }
        if !stage.replaces.is_empty() {
            println!(
                "  replace {}",
                stage
                    .replaces
                    .iter()
                    .map(|(package, seeds)| format!("{package} -> {}", seeds.join(", ")))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        println!("  order  {}", stage.order.join(", "));
    }
    if let Some(stage) = plan.stages.first() {
        let arch = if plan.archs.len() > 1 {
            format!(" --arch {}", stage.arch)
        } else {
            String::new()
        };
        println!("next: rpmx bootstrap build {}{arch}", plan.manifest);
    }
}

pub fn plan(plan: &Plan) {
    if let Some(source) = &plan.source_assembly {
        println!(
            "source assembly: steps={} tools={}",
            source.steps.len(),
            source.tools.len()
        );
        for step in &source.steps {
            println!("  step   {}", step_summary(step));
        }
    }

    for cell in &plan.cells {
        let id = if cell.id.is_empty() {
            "default"
        } else {
            &cell.id
        };
        println!(
            "{id}  slug={} release={} package={}",
            cell.slug, cell.release, cell.package
        );
        for step in &cell.pipeline {
            println!("  step   {}", step_summary(step));
        }
        for output in &cell.outputs {
            println!("  output {}", output.name);
        }
        for (phase, tests) in [("build test", &cell.build_tests), ("test", &cell.tests)] {
            for test in tests {
                if phase == "build test" {
                    println!("  build test {}", test.qualified_name());
                } else {
                    println!("  test   {} ({})", test.qualified_name(), test.mode);
                }
                if let Some(argv) = &test.argv {
                    println!("    argv   {}", argv.join(" "));
                }
                if !test.packages.is_empty() {
                    println!("    root   {}", test.packages.join(", "));
                }
                for (key, value) in &test.environment {
                    println!("    env    {key}={value}");
                }
                for step in &test.pipeline {
                    println!("    step   {}", step_summary(step));
                }
            }
        }
    }
}

pub fn step_summary(step: &PlannedStep) -> String {
    let name = step_name(step);
    if let Some(command) = &step.command {
        return format!("{name}: {} {}", command, step.args.join(" "));
    }
    let mut parts = Vec::with_capacity(step.inputs.len());
    for (key, value) in &step.inputs {
        parts.push(format!("{key}={}", scalar(value)));
    }
    format!("{name} {}", parts.join(" "))
}

fn step_name(step: &PlannedStep) -> String {
    if step.label.is_empty() || step.label == step.uses || step.uses == "<anonymous>" {
        return if step.label.is_empty() {
            step.uses.clone()
        } else {
            step.label.clone()
        };
    }
    format!("{} ({})", step.label, step.uses)
}

fn scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(boolean) => boolean.to_string(),
        other => other.to_string(),
    }
}
