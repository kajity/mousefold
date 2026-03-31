---
name: rust-tooling-conventions
description: "このプロジェクトで Rust の実装品質を揃えるための lint、error handling、logging、CLI、依存追加方針を定義する skill。"
---

# rust-tooling-conventions

## 目的
実装のたびに書き方がぶれないよう、Rust 側の共通規約を固定する。

## この skill を使う場面
- 新しい crate やモジュールを足す
- エラー型を整理する
- ログ出力方針を統一する
- CLI を整える
- 依存追加の判断をする

## 共通規約
- `cargo fmt` 前提で書く
- `clippy -D warnings` を通す
- 本番コードで `unwrap()` / `expect()` を避ける
- 文字列エラーではなく型付きエラーを使う
- ログメッセージは運用者が読める文にする
- public API には短い rustdoc を付ける
- Linux 固有処理は境界の内側へ閉じ込める
- 実装途中でもテストしやすい形を保つ

## CLI 方針
最低限、以下を提供する。

- `--config <path>` (常駐起動)
- `check --config <path>`
- `monitor --config <path>`
- `reload --config <path>`

サブコマンドごとの責務は混ぜない。
常駐処理はサブコマンド無し起動へ置く。
`check` / `monitor` / `reload` は短命コマンドとして保つ。

## エラー方針
エラーは、原因と文脈が読めることを優先する。

例:
- 設定ファイル読込失敗
- YAML 構文不正
- 無効なキーコード
- デバイスオープン失敗
- grab 失敗
- uinput 初期化失敗
- 再読込失敗
- Bluetooth adapter 解決失敗
- Bluetooth pair / trust / connect 失敗

## ログ方針
- 起動時に対象デバイスと設定パスを `INFO`
- grab 成功を `INFO`
- 競合ルールを `WARN`
- 再読込成功を `INFO`
- 再読込失敗で旧設定維持を `WARN`
- イベント詳細は `DEBUG`

## 依存追加の判断基準
依存を追加するときは毎回、次を満たすこと。

- その依存が責務に直結している
- 標準ライブラリだけで無理がある
- 境界の内側に閉じ込められる
- 代替案より保守しやすい
- プロジェクト全体を不必要に複雑にしない

Bluetooth 接続制御では `bluer` を第一候補とし、`bluetooth` モジュール境界の内側へ閉じ込める。

## 推奨チェック
変更後は必ず以下を回す。

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

## レビュー時の観点
- エラー文だけで障害解析の初動ができるか
- ログ粒度が適切か
- 依存追加が妥当か
- public API の責務が狭いか
- 過剰抽象化していないか

## 出力の期待形
この skill を使ったときは、次を返す。

- 追加した依存と理由
- エラー型の変更内容
- ログ追加内容
- CLI 変更内容
- fmt/clippy/test の確認結果
