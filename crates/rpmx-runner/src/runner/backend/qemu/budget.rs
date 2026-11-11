//! One monotonic deadline covers preparation, transport and payload execution.
use rpmx_base::error::{codes, Error, Result};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) struct Budget {
    pub deadline: Option<Instant>,
    pub default: Duration,
}

impl Budget {
    pub fn remaining(self, limit: Duration, label: &str) -> Result<Duration> {
        let remaining = self
            .deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(limit)
            .min(limit);
        if remaining.is_zero() {
            return Err(Error::new(
                codes::SANDBOX_004,
                format!("qemu case deadline expired during {label}"),
            ));
        }
        Ok(remaining)
    }

    pub fn output(self, command: &mut Command, label: &str) -> Result<Output> {
        let remaining = self.remaining(self.default, label)?;
        let mut bounded = Command::new("timeout");
        bounded
            .args(["--signal=TERM", "--kill-after=2", &seconds(remaining)])
            .arg(command.get_program())
            .args(command.get_args());
        if let Some(directory) = command.get_current_dir() {
            bounded.current_dir(directory);
        }
        for (name, value) in command.get_envs() {
            match value {
                Some(value) => {
                    bounded.env(name, value);
                }
                None => {
                    bounded.env_remove(name);
                }
            }
        }
        bounded
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let output = super::super::process::spawn_inheriting_fds(bounded, &[])
            .and_then(|child| child.wait_with_output())
            .map_err(|error| {
                Error::new(codes::RUNNER_003, format!("cannot run {label}: {error}"))
            })?;
        if matches!(output.status.code(), Some(124 | 137)) && started.elapsed() >= remaining {
            return Err(Error::new(
                codes::SANDBOX_004,
                format!("qemu case deadline expired during {label}"),
            ));
        }
        Ok(output)
    }
}

pub(super) fn seconds(duration: Duration) -> String {
    format!("{:.9}", duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_fractional_case_deadline_bounds_preparation_and_is_not_reset() {
        let budget = Budget {
            deadline: Some(Instant::now() + Duration::from_millis(50)),
            default: Duration::from_secs(30),
        };
        let started = Instant::now();
        let error = budget
            .output(
                Command::new("sh").args(["-c", "sleep 30"]),
                "preparing fixture",
            )
            .unwrap_err();
        assert_eq!(error.code, codes::SANDBOX_004);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(budget
            .remaining(Duration::from_secs(30), "second operation")
            .is_err());
    }
}
