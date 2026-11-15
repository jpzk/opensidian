// THROWAWAY: deliberately bad pattern to prove the Opengrep SARIF pipeline.
// Not wired into the module tree. Never merge.
use std::process::Command;

pub fn run_user_script(user_input: &str) {
    let _ = Command::new("sh").arg("-c").arg(user_input).status();
}
