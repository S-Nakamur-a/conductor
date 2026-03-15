# Permission Rules

このファイルは Claude Code のツール実行許可を自動判定するためのルールを定義します。

## 自動承認 (approve)

- Read / Glob / Grep ツールはすべて許可
- Write / Edit でプロジェクト内のソースコード（src/, tests/, Cargo.toml 等）への変更は許可
- Bash で cargo build, cargo test, cargo clippy, cargo check は許可
- Bash で git status, git diff, git log 等の読み取り系 git コマンドは許可
- Bash で ls, cat, head, tail, wc 等の読み取り系コマンドは許可

## 自動拒否 (deny)

- Bash で rm -rf / や重要なシステムディレクトリへの操作は拒否
- Bash で git push --force は拒否
- プロジェクト外のファイルへの Write / Edit は拒否（/tmp は例外として許可）

## ユーザー判断 (ask_user)

- 上記に該当しない場合はユーザーに判断を求める
- git commit, git push 等の変更を伴う git コマンドはユーザー判断
- 外部ネットワークへのアクセスはユーザー判断
