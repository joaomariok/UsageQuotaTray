//! The real environment probe answers presence only. It reads (never writes) this
//! process's environment and the Environment registry keys, by existence.

use uqt_assistant_anthropic::{EnvProbe, EnvScope, SystemEnv};

#[test]
fn reports_whether_a_variable_exists_in_each_scope() {
    let absent = "UQT_SURELY_UNSET_7f3a9c";
    for scope in [EnvScope::Process, EnvScope::User, EnvScope::Machine] {
        assert!(!SystemEnv.is_set(scope, absent), "{scope:?}");
    }
    // Present on every Windows installation.
    assert!(SystemEnv.is_set(EnvScope::Process, "SystemRoot"));
    assert!(SystemEnv.is_set(EnvScope::Machine, "Path"));
    assert!(SystemEnv.is_set(EnvScope::User, "TEMP"));
}
