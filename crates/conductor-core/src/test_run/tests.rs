use super::*;

fn lines(src: &str) -> Vec<String> {
    src.lines().map(str::to_string).collect()
}

mod go {
    use super::*;

    #[test]
    fn テストでないファイルからは何も出ない() {
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        assert!(scan_go_test_runs(&src, "foo/foo.go").is_empty());
    }

    #[test]
    fn ファイルと関数とサブテストを見つける() {
        let src = lines(
            "package foo\n\
             \n\
             func TestAlpha(t *testing.T) {\n\
             \tt.Run(\"case one\", func(t *testing.T) {})\n\
             }\n\
             \n\
             func TestBeta(t *testing.T) {}\n",
        );
        let runs = scan_go_test_runs(&src, "pkg/foo/foo_test.go");

        // 1 行目のファイルボタンは両方のテストを実行する。
        let file = &runs[&1];
        assert_eq!(file.kind, TestRunKind::File);
        assert_eq!(
            file.command,
            "go test -run '^(TestAlpha|TestBeta)$' './pkg/foo'"
        );

        // TestAlpha の行の関数ボタン。
        let alpha = &runs[&3];
        assert_eq!(alpha.kind, TestRunKind::Func);
        assert_eq!(alpha.command, "go test -run '^TestAlpha$' './pkg/foo'");

        // t.Run の行のサブテストボタン (空白はアンダースコアへ)。
        let sub = &runs[&4];
        assert_eq!(sub.kind, TestRunKind::Subtest);
        assert_eq!(sub.label, "TestAlpha/case one");
        assert_eq!(
            sub.command,
            "go test -run '^TestAlpha$/^case_one$' './pkg/foo'"
        );

        // TestBeta の行の関数ボタン。
        assert_eq!(runs[&7].command, "go test -run '^TestBeta$' './pkg/foo'");
    }

    #[test]
    fn test_mainは実行できるテストとして出さない() {
        let src = lines(
            "package foo\n\
             func TestMain(m *testing.M) {}\n\
             func TestReal(t *testing.T) {}\n",
        );
        let runs = scan_go_test_runs(&src, "a_test.go");
        // 1 行目のファイルボタンは TestReal だけを並べる。TestMain にはボタンが無い。
        assert_eq!(runs[&1].command, "go test -run '^(TestReal)$' '.'");
        assert!(!runs.contains_key(&2));
        assert_eq!(runs[&3].kind, TestRunKind::Func);
    }

    #[test]
    fn ルート直下のファイルはカレントディレクトリを対象にする() {
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        let runs = scan_go_test_runs(&src, "x_test.go");
        assert_eq!(runs[&2].command, "go test -run '^TestX$' '.'");
    }

    #[test]
    fn 厄介なディレクトリ名はシェル用に引用する() {
        // シェルのメタ文字を含むディレクトリ名 (信用できないリポジトリならあり得る)
        // はクォートで無力化されなければならない。; はクォートの内側に留まる。
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        let runs = scan_go_test_runs(&src, "a; rm -rf x/x_test.go");
        assert_eq!(runs[&2].command, "go test -run '^TestX$' './a; rm -rf x'");
    }

    #[test]
    fn ディレクトリ名の単引用符はエスケープする() {
        let src = lines("package foo\nfunc TestX(t *testing.T) {}");
        let runs = scan_go_test_runs(&src, "o'clock/x_test.go");
        // '\'' はクォートを閉じ、エスケープしたクォートを足し、また開く。
        assert_eq!(runs[&2].command, "go test -run '^TestX$' './o'\\''clock'");
    }

    #[test]
    fn テストが無ければボタンも出ない() {
        let src = lines("package foo\nfunc helper() {}\n");
        assert!(scan_go_test_runs(&src, "foo_test.go").is_empty());
    }

    #[test]
    fn テスト関数の外のサブテストは無視する() {
        let src = lines(
            "package foo\n\
             func helper() {\n\
             \tx.Run(\"nope\", nil)\n\
             }\n\
             func TestX(t *testing.T) {}\n",
        );
        let runs = scan_go_test_runs(&src, "foo_test.go");
        assert!(!runs.contains_key(&3));
    }
}

mod rust {
    use super::*;

    #[test]
    fn rustでないファイルからは何も出ない() {
        let src = lines("fn main() {}\n#[test]\nfn t() {}");
        assert!(scan_rust_test_runs(&src, "README.md").is_empty());
    }

    #[test]
    fn srcとtestsの外のファイルからは何も出ない() {
        let src = lines("#[test]\nfn t() {}");
        assert!(scan_rust_test_runs(&src, "benches/bench.rs").is_empty());
    }

    #[test]
    fn ファイルとモジュールと関数を見つける() {
        let src = lines(
            "pub fn foo() {}\n\
             \n\
             #[cfg(test)]\n\
             mod tests {\n\
             \x20   #[test]\n\
             \x20   fn it_works() {}\n\
             }\n",
        );
        let runs = scan_rust_test_runs(&src, "src/ai_caller.rs");

        // 1 行目のファイルボタンはファイル内のすべてを実行する。
        let file = &runs[&1];
        assert_eq!(file.kind, TestRunKind::File);
        assert_eq!(file.label, "ai_caller.rs");
        assert_eq!(file.command, "cargo test 'ai_caller::'");

        // mod tests の行 (4 行目) のモジュールボタン。
        let module = &runs[&4];
        assert_eq!(module.kind, TestRunKind::Module);
        assert_eq!(module.label, "tests");
        assert_eq!(module.command, "cargo test 'ai_caller::tests::'");

        // fn it_works の行 (6 行目) の関数ボタン。
        let func = &runs[&6];
        assert_eq!(func.kind, TestRunKind::Func);
        assert_eq!(func.label, "it_works");
        assert_eq!(
            func.command,
            "cargo test 'ai_caller::tests::it_works' -- --exact"
        );
    }

    #[test]
    fn tokio_testのasync関数も見つける() {
        let src = lines(
            "#[cfg(test)]\n\
             mod tests {\n\
             \x20   #[tokio::test]\n\
             \x20   async fn talks() {}\n\
             }\n",
        );
        let runs = scan_rust_test_runs(&src, "src/net.rs");
        let func = &runs[&4];
        assert_eq!(func.kind, TestRunKind::Func);
        assert_eq!(func.command, "cargo test 'net::tests::talks' -- --exact");
    }

    #[test]
    fn 入れ子のモジュールは完全なパスになる() {
        let src = lines(
            "#[cfg(test)]\n\
             mod tests {\n\
             \x20   mod command {\n\
             \x20       #[test]\n\
             \x20       fn echoes() {}\n\
             \x20   }\n\
             }\n",
        );
        let runs = scan_rust_test_runs(&src, "src/ai_caller.rs");

        // 内側のモジュールボタン (3 行目) は入れ子のモジュールに絞られる。
        assert_eq!(runs[&3].command, "cargo test 'ai_caller::tests::command::'");
        // 外側のモジュールボタン (2 行目)。
        assert_eq!(runs[&2].command, "cargo test 'ai_caller::tests::'");
        // 関数は入れ子を含む完全なパスを持つ (5 行目)。
        assert_eq!(
            runs[&5].command,
            "cargo test 'ai_caller::tests::command::echoes' -- --exact"
        );
    }

    #[test]
    fn テストの無いcfg_testモジュールからは何も出ない() {
        // ヘルパーしか無い #[cfg(test)] モジュールはテストのスコープではない。
        let src = lines(
            "#[cfg(test)]\n\
             mod tests {\n\
             \x20   fn helper() {}\n\
             }\n",
        );
        assert!(scan_rust_test_runs(&src, "src/foo.rs").is_empty());
    }

    #[test]
    fn mod_rsはディレクトリのモジュールに対応する() {
        let src = lines("#[test]\nfn t() {}\n");
        let runs = scan_rust_test_runs(&src, "src/app/mod.rs");
        // app/mod.rs はモジュール app。トップレベルの #[test] はその直下に置かれる。
        assert_eq!(runs[&2].command, "cargo test 'app::t' -- --exact");
        assert_eq!(runs[&1].command, "cargo test 'app::'");
    }

    #[test]
    fn クレートルートのファイルボタンは全部を走らせる() {
        let src = lines("#[test]\nfn t() {}\n");
        let runs = scan_rust_test_runs(&src, "src/main.rs");
        // クレートルートではモジュール接頭辞が無いので、ファイルボタンは全部を実行する。
        assert_eq!(runs[&1].command, "cargo test");
        assert_eq!(runs[&2].command, "cargo test 't' -- --exact");
    }

    #[test]
    fn 統合テストのファイルはtestフラグを使う() {
        let src = lines("#[test]\nfn smoke() {}\n");
        let runs = scan_rust_test_runs(&src, "tests/e2e.rs");
        assert_eq!(runs[&1].command, "cargo test --test 'e2e'");
        assert_eq!(
            runs[&2].command,
            "cargo test --test 'e2e' 'smoke' -- --exact"
        );
    }

    #[test]
    fn 厄介なファイル名はシェル用に引用する() {
        // シングルクォートを含むパス (信用できないリポジトリならあり得る) は、
        // モジュール接頭辞の '\'' エスケープで無力化されなければならない。
        let src = lines("#[test]\nfn t() {}\n");
        let runs = scan_rust_test_runs(&src, "src/o'clock.rs");
        assert_eq!(runs[&1].command, "cargo test 'o'\\''clock::'");
    }
}
