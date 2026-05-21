use std::process::Command;

pub trait GitOps {
    fn base_branch_exists(&self, base_branch: &str) -> bool;
    fn ensure_hygiene(&self, base_branch: &str) -> Result<(), Box<dyn std::error::Error>>;
}

pub struct GitCliAdapter;

impl GitOps for GitCliAdapter {
    fn base_branch_exists(&self, base_branch: &str) -> bool {
        let local = format!("refs/heads/{}", base_branch);
        let remote = format!("refs/remotes/origin/{}", base_branch);

        for reference in [&local, &remote] {
            if Command::new("git")
                .args(["show-ref", "--verify", "--quiet", reference])
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
            {
                return true;
            }
        }

        false
    }

    fn ensure_hygiene(&self, base_branch: &str) -> Result<(), Box<dyn std::error::Error>> {
        println!(
            "nightshift: enforcing git hygiene (checking out and pulling {})...",
            base_branch
        );

        let checkout_status = Command::new("git")
            .args(["checkout", base_branch])
            .status()?;

        if !checkout_status.success() {
            return Err(format!("failed to checkout base branch '{}'", base_branch).into());
        }

        let pull_status = Command::new("git").args(["pull"]).status()?;

        if !pull_status.success() {
            return Err("failed to pull latest changes from remote".into());
        }

        Ok(())
    }
}
