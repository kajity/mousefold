---
name: rust-remapper-architecture
description: "Rust 製マウス入力リマッパの crate 構成、責務分離、API 境界、実装順序を設計するための skill。"
---

# rust-remapper-architecture

## 目的
Linux 上で動くマウス入力カスタマイズ用デーモンの土台を、過不足なく設計する。
この skill は、機能追加そのものよりも「どこに何を書くべきか」を明確にするために使う。

## この skill を使う場面
- 新規に crate 構成を作る
- `config` / `device` / `virtual_mouse` / `virtual_keyboard` / `router` / `reload` の分割を決める
- 型設計やエラー設計を決める
- `grab` を含むイベント経路を決める
- ホットリロード時の状態差し替え方針を決める
- Bluetooth マウスの接続責務をどこへ置くか決める
- 常駐起動と `check` / `monitor` / `reload` の CLI 境界を決める
- モード切替の責務をどこへ置くか決める
- 過剰抽象化を避けつつ将来拡張の余地を残したい

## 守る制約
- 単一マウスのみ
- 選択デバイスは `grab` する
- キーボードへ変換する対象は初期実装ではボタンイベントのみ
- 未リマップの移動・ホイール・通常ボタンは仮想マウスへ流す
- 設定は YAML 優先
- systemd 管理下の常駐デーモン
- root 実行
- 仮想マウスと仮想キーボードを生成する
- ホットリロードあり
- 無効設定時は旧設定を維持
- 競合ルールは後勝ち
- Bluetooth マウス指定時はデバイス名から pair / trust / connect を自動化する
- 常駐起動はサブコマンド無し、`monitor` は一時確認用
- モード切替を扱う

## 推奨ディレクトリ構成
```text
src/
  main.rs
  app.rs
  cli.rs
  error.rs
  config/
    mod.rs
    model.rs
    load.rs
    validate.rs
    conflict.rs
    mode.rs
  device/
    mod.rs
    mouse.rs
    grab.rs
    normalize.rs
  bluetooth/
    mod.rs
    adapter.rs
    pairing.rs
    connect.rs
  virtual_mouse/
    mod.rs
    builder.rs
    emit.rs
  virtual_keyboard/
    mod.rs
    builder.rs
    emit.rs
  router/
    mod.rs
    resolve.rs
    compiled.rs
    mode_switch.rs
  reload/
    mod.rs
    watcher.rs
    apply.rs
  runtime/
    mod.rs
    state.rs
    signal.rs
tests/
  config_parse.rs
  config_validate.rs
  conflict_resolution.rs
  router_resolve.rs
  passthrough_behavior.rs
  reload_rollback.rs
```

## 推奨責務
### `main.rs`
- 引数処理
- ロガー初期化
- 起動シーケンス開始
- エラー時の終了コード決定

### `cli.rs`
- サブコマンド無し起動と `check` / `monitor` / `reload` の定義
- 共通オプション定義
- `reload` の制御経路の抽象化

### `app.rs`
- アプリ全体の起動順序をまとめる
- 設定ロード
- デバイスオープンと `grab`
- 必要なら Bluetooth 接続の確立
- 仮想マウス/キーボード生成
- ランタイム開始

### `error.rs`
- アプリ共通エラー型
- 文脈付きエラーの集約

### `config/*`
- YAML モデル
- パース
- 論理検証
- 競合解決
- モード定義とモード切替定義
- 実行用ルールへのコンパイル

### `device/*`
- 入力デバイスのオープン
- `grab` / `ungrab`
- capability の取得
- イベント取得
- 入力イベントの正規化

### `bluetooth/*`
- `bluer` を使った BlueZ adapter / device 制御
- pair / trust / connect の順序制御
- 接続完了後に evdev 側へ渡す識別情報の確定

### `virtual_mouse/*`
- 元デバイス capability をもとに仮想マウス構築
- 移動、ホイール、未リマップボタンの送出
- SYN の送出

### `virtual_keyboard/*`
- 使用キー一覧から仮想キーボード構築
- キー送出

### `router/*`
- 入力イベントとルールの照合
- モード切替入力の評価
- `PassThroughToMouse` か `RemapToKeyboard` かを決める
- 実行時に高速参照できる形へ変換

### `reload/*`
- ファイル監視
- debounce
- 新設定検証
- 旧状態から新状態への安全な差し替え

### `runtime/*`
- 共有状態
- 終了シグナル処理
- タスクの束ね役

## 型設計の指針
- 設定の生データ型と、実行用のコンパイル済み型を分ける
- 文字列のまま引き回さず、可能なら enum 化する
- 実行時ルックアップを軽くするため、ルールはロード時に正規化する

## 例
```rust
pub struct AppConfig {
    pub device: DeviceConfig,
    pub reload: ReloadConfig,
    pub remaps: Vec<RemapRule>,
}

pub enum CliCommand {
    Run { config: PathBuf },
    Check { config: PathBuf },
    Monitor { config: PathBuf },
    Reload { config: PathBuf },
}

pub struct CompiledRules {
    pub remap_entries: Vec<CompiledRule>,
    pub keyboard_keys_needed: Vec<KeyCode>,
}

pub struct CompiledModes {
    pub default_mode: ModeId,
    pub active_mode: ModeId,
    pub rules_by_mode: HashMap<ModeId, CompiledRules>,
}

pub struct CompiledRule {
    pub input: MouseButtonTrigger,
    pub output: Vec<KeyStroke>,
    pub source_index: usize,
    pub description: Option<String>,
}

pub enum RoutedAction {
    PassThroughToMouse(NormalizedMouseEvent),
    RemapToKeyboard(Vec<KeyStroke>),
    Ignore,
}
```

## エラー設計の指針
- 失敗地点が分かる名前を付ける
- I/O エラー、grab エラー、設定エラーを分ける
- Bluetooth adapter / pair / trust / connect エラーを分ける
- 設定エラーにはルール位置や対象項目を持たせる
- 再読込失敗は致命終了ではなく `WARN` と旧設定維持に落とす

## 実装順序
1. サブコマンド無し起動を含む `cli` 定義
2. `config` のデータモデルとパーサ
3. 論理検証
4. 競合解決
5. `device` のオープンと `grab`
6. `virtual_mouse` の最小パススルー
7. `virtual_keyboard` の最小出力
8. `router` の 1:1 解決
9. 複数キー出力
10. `reload`
11. `bluetooth`
12. モード切替
13. systemd 運用

## 変更時のチェックリスト
- 責務が別モジュールへ漏れていないか
- 実行時判定をロード時へ寄せられないか
- 無効設定のとき旧設定維持になっているか
- 競合ルールがログに出るか
- grab 失敗や二重入力を考慮しているか
- Bluetooth の再接続責務が他モジュールへ漏れていないか
- `monitor` が常駐経路と混線していないか
- モード切替が通常リマップより先に評価されるか
- 受け入れ条件への影響を説明できるか

## 出力の期待形
この skill を使ったときは、次を返す。

- 変更対象ファイル一覧
- 各ファイルの責務
- 追加する型
- 失敗モード
- 最小実装手順
- 必要なテスト
