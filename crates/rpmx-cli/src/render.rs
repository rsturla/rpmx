use rpmx_core::{Plan, PlannedStep};

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
        for test in &cell.tests {
            println!("  test   {} ({})", test.name, test.stage);
            for step in &test.pipeline {
                println!("    step   {}", step_summary(step));
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
