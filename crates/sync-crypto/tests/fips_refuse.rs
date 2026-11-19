// SPDX-License-Identifier: GPL-3.0-or-later
//! Approved-mode enforcement (cmvp.md §5): sync refuses to start when the
//! OpenSSL FIPS provider is not active. Each case re-runs this test binary as
//! a child with a broken OpenSSL environment (OpenSSL reads OPENSSL_CONF /
//! OPENSSL_MODULES once per process, so a child is the only honest way) and
//! the child asserts `fips::init()` / `FipsProvider::new()` fail.
use std::process::Command;

use sync_crypto::fips::{self, FipsProvider};

const CHILD: &str = "SYNC_FIPS_REFUSE_CHILD";

fn run_child(case: &str, envs: &[(&str, String)]) {
    let exe = std::env::current_exe().unwrap();
    let mut c = Command::new(exe);
    c.args(["--exact", "child_expects_refusal", "--ignored", "--nocapture", "--test-threads=1"]);
    c.env(CHILD, case);
    for (k, v) in envs {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    eprintln!("--- child {case} ---\n{text}");
    assert!(out.status.success(), "child {case} failed");
    assert!(text.contains("REFUSED:"), "child {case} did not report a refusal");
    assert!(text.contains("1 passed"), "child {case} did not run");
}

fn tmpdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("osy-fips-refuse-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn refuses_without_fips_module() {
    // fips.so cannot be found: empty module dir, empty config
    let d = tmpdir("nomod");
    std::fs::write(d.join("openssl.cnf"), "").unwrap();
    run_child(
        "no-module",
        &[
            ("OPENSSL_MODULES", d.display().to_string()),
            ("OPENSSL_CONF", d.join("openssl.cnf").display().to_string()),
        ],
    );
}

#[test]
fn refuses_without_fipsmodule_cnf() {
    // fips.so present (OPENSSL_MODULES from the environment) but the config
    // has no fipsmodule.cnf section: the provider has no integrity value and
    // its self-test refuses to load
    let d = tmpdir("nocnf");
    std::fs::write(d.join("openssl.cnf"), "").unwrap();
    run_child("no-fipsmodule-cnf", &[("OPENSSL_CONF", d.join("openssl.cnf").display().to_string())]);
}

#[test]
fn refuses_when_default_provider_active() {
    // a config that activates the non-approved default provider next to fips
    let d = tmpdir("default");
    let conf = std::env::var("OPENSSL_CONF").expect("fips-env.sh sets OPENSSL_CONF");
    let fipsmod = std::path::Path::new(&conf).with_file_name("fipsmodule.cnf");
    assert!(fipsmod.exists(), "{} missing", fipsmod.display());
    std::fs::write(
        d.join("openssl.cnf"),
        format!(
            "config_diagnostics = 1\nopenssl_conf = openssl_init\n.include {}\n[openssl_init]\nproviders = provider_sect\n[provider_sect]\nfips = fips_sect\ndefault = default_sect\n[default_sect]\nactivate = 1\n",
            fipsmod.display()
        ),
    )
    .unwrap();
    run_child("default-active", &[("OPENSSL_CONF", d.join("openssl.cnf").display().to_string())]);
}

#[test]
fn refuses_when_security_checks_disabled() {
    // a valid module + integrity MAC, but fipsmodule.cnf has security-checks = 0:
    // the provider still loads and serves fips=yes, only without the approved-
    // mode key/digest checks -> must be refused (the -pedantic file is the
    // positive control every other test runs with)
    let d = tmpdir("nochecks");
    let conf = std::env::var("OPENSSL_CONF").expect("fips-env.sh sets OPENSSL_CONF");
    let fipsmod = std::path::Path::new(&conf).with_file_name("fipsmodule.cnf");
    let orig = std::fs::read_to_string(&fipsmod).unwrap();
    assert!(orig.contains("security-checks = 1"), "{} is not a -pedantic config", fipsmod.display());
    std::fs::write(d.join("fipsmodule.cnf"), orig.replace("security-checks = 1", "security-checks = 0")).unwrap();
    std::fs::write(
        d.join("openssl.cnf"),
        format!(
            "config_diagnostics = 1\nopenssl_conf = openssl_init\n.include {}\n[openssl_init]\nproviders = provider_sect\nalg_section = algorithm_sect\n[provider_sect]\nfips = fips_sect\nbase = base_sect\n[base_sect]\nactivate = 1\n[algorithm_sect]\ndefault_properties = fips=yes\n",
            d.join("fipsmodule.cnf").display()
        ),
    )
    .unwrap();
    run_child("security-checks-0", &[("OPENSSL_CONF", d.join("openssl.cnf").display().to_string())]);
}

#[test]
#[ignore = "child process of the refusal tests"]
fn child_expects_refusal() {
    let Ok(case) = std::env::var(CHILD) else {
        return; // only meaningful as a child of the tests above
    };
    let err = fips::init().expect_err("sync must refuse without an active FIPS provider");
    assert!(FipsProvider::new().is_err());
    if case == "security-checks-0" {
        // refused for the disabled check itself, not for some load failure
        assert!(err.to_string().contains("security-checks = Some(0)"), "{err}");
    }
    println!("REFUSED: {case}: {err}");
}
