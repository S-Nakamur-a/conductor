use std::process::Command;

#[test]
fn 消したcc_signalは引数を問わず何も書かずにexit0で返る() {
    for args in [
        &["cc-signal", "active"][..],
        &["cc-signal", "waiting"],
        &["cc-signal"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_conductor"))
            .args(args)
            .env_remove("CONDUCTOR_NOTIFY_SOCK")
            .output()
            .expect("run conductor");
        assert!(out.status.success(), "{args:?}: {:?}", out.status);
        assert!(out.stdout.is_empty(), "{args:?}: {:?}", out.stdout);
        assert!(out.stderr.is_empty(), "{args:?}: {:?}", out.stderr);
    }
}
