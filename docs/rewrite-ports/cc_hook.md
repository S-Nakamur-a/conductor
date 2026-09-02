# cc_hook
旧テスト 2 本 (+ src/cc_notify.rs の parse_message 4 本) → 新テスト 4 本

| 旧テスト名 | 扱い | 新テスト名 / 削除理由 |
|---|---|---|
| session_idを取り出す | 移植 | payloadからsession_idを取り出す (実測 payload は fixture のまま) |
| 使えない入力でも落ちない | 移植 | 使えないpayloadはnone |
| (cc_notify) 状態のメッセージを読む | 移植 | 電文は往復する |
| (cc_notify) セッションのローテーションを読む | 移植 | 同上 |
| (cc_notify) 壊れたメッセージは拒む | 移植 | 壊れた電文は拒む |
| (cc_notify) 空白を含むcwdも壊れない | 移植 | 電文は往復する ("active /tmp/my worktree") |

cc_notify の 4 本をここに移したのは、電文の parse を core に持ってきたため (下記)。
svc 側でリスナを移植するときは parse_message とそのテストを持ち込まず
`cc_hook::Notification::parse` を使う。

API 変更:
- 追加: `Notification { Active{cwd}, Waiting{cwd}, Session{panel_id, session_id} }` と
  `parse(&str) -> Option<Self>` / `to_line(&self) -> String`。cc-notify ソケットの電文の
  定義を書く側 (フック) と聞く側 (svc のリスナ) で 1 箇所にする。`run()` はこれで行を組む
- `run()` / `PANEL_ID_ENV` / `NOTIFY_SOCK_ENV` はそのまま
- 旧 cc_notify の `CcNotifyEvent::State{kind, cwd}` / `CcNotifyKind` は動詞ごとのバリアントに
  平坦化した (svc 側で変換するか、そのまま使うかは svc の判断)

残したコメント (なぜ):
- モジュール doc: フックがパネルの Claude の子として走るので env が見える、バイナリ同居の理由、stdout に書かない理由
- run: payload の source を見ない理由、1 回の write で送る理由 (分割 write で "session" だけ届いた実測)
- Notification: 空白区切りで曖昧にならない理由 (id は UUID、cwd は行末まで)
