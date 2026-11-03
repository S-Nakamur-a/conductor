use std::process::Command;

#[test]
fn 消したcc_signalは引数を問わず何も書かずにexit0で返る() {
    // 腕が壊れると窓の起動へ進むので、本物の runtime dir と作業ディレクトリに触らせない。
    let sandbox = std::env::temp_dir().join(format!("cc-signal-test-{}", std::process::id()));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    for args in [
        &["cc-signal", "active"][..],
        &["cc-signal", "waiting"],
        &["cc-signal"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_conductor"))
            .args(args)
            .env_remove("CONDUCTOR_NOTIFY_SOCK")
            .env("XDG_RUNTIME_DIR", &sandbox)
            .env("TMPDIR", &sandbox)
            .current_dir(&sandbox)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run conductor");
        assert!(out.status.success(), "{args:?}: {:?}", out.status);
        assert!(out.stdout.is_empty(), "{args:?}: {:?}", out.stdout);
        assert!(out.stderr.is_empty(), "{args:?}: {:?}", out.stderr);
    }
}
