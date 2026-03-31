# mousefold 設計メモ

## 1. 設計方針

初期実装は、単一マウス入力を `grab` して監視し、YAML 設定に基づいてイベントを以下の 2 系統へ振り分ける常駐デーモンとする。

- リマップ対象のボタンイベントは仮想キーボードからキーイベントを送出する
- 未リマップの移動・ホイール・通常ボタンは仮想マウスへ流す

実装上の複雑性を抑えるため、初期スコープでは以下を前提とする。

- 入力対象は単一デバイス
- キーボード変換対象はボタンイベントのみ
- 移動イベントとホイールイベントは初期実装ではパススルーのみ
- 設定フォーマットは YAML
- systemd 管理下で root 実行
- 設定変更はホットリロード対応
- Bluetooth マウス指定時は BlueZ 経由でデバイス名から pair / trust / connect を自動化
- 常駐監視本体はサブコマンド無し起動とし、`monitor` は一時的な入力確認に使う
- モード切替によりキーバインドセットを切り替えられる構成にする

## 2. 推奨モジュール分割

### 2.1 `config`

責務:

- YAML 読み込み
- 構文検証
- 論理検証
- 競合ルール検出

主要処理:

- `load(path) -> Config`
- `validate(config) -> ValidationResult`
- `detect_conflicts(config) -> Vec<ConflictWarning>`

### 2.2 `device`

責務:

- 物理マウスデバイスのオープン
- `grab` / `ungrab`
- `evdev` イベント受信
- デバイス capability 取得
- 入力イベント正規化

主要処理:

- `open_mouse(path) -> Device`
- `grab(device) -> Result<()>`
- `next_event() -> NormalizedMouseEvent`
- `read_capabilities() -> SourceMouseCapabilities`

### 2.3 `bluetooth`

責務:

- BlueZ device 名ベースの探索
- 対象 Bluetooth マウスの pair / trust / connect
- 接続済み判定と evdev 側起動条件の提供
- 同名候補のうち未接続デバイスだけを接続対象に絞る

主要処理:

- `ensure_connected(selector) -> ConnectedBluetoothDevice`
- `pair_if_needed(device) -> Result<()>`
- `connect(device) -> Result<()>`

### 2.4 `virtual_mouse`

責務:

- `uinput` 仮想マウス生成
- 未リマップイベントの送出

主要処理:

- `build_from_capabilities(caps) -> VirtualMouse`
- `emit_mouse(event)`

### 2.5 `virtual_keyboard`

責務:

- `uinput` 仮想キーボード生成
- キー送出

主要処理:

- `build(keys) -> VirtualKeyboard`
- `emit(sequence)`

### 2.6 `router`

責務:

- 入力イベントとルールの照合
- マウスへ流すか、キーボードへ変換するかの判定

主要処理:

- `route(event, rules) -> RoutedAction`

補足:

- 競合ルールはロード時に解決方針を確定する
- 実行時は「最後に有効となるルール」のみ参照すると単純化できる

### 2.7 `reload`

責務:

- 設定ファイル変更監視
- 新設定の読込と差し替え
- 失敗時のロールバック

主要処理:

- `watch(path)`
- `reload_if_changed()`

## 3. 推奨ランタイム構成

単一プロセス内で以下のタスクを持つ構成を推奨する。

1. CLI 引数解決
2. 入力イベント受信タスク
3. 設定ファイル監視タスク
4. 終了シグナル監視タスク
5. 必要に応じて Bluetooth 接続待ちタスク

共有状態:

- 現在有効な設定
- 現在有効なルール集合
- 仮想マウスハンドル
- 仮想キーボードハンドル
- 現在監視中の物理デバイス情報
- 現在有効な Bluetooth 接続情報

共有状態の更新は `Arc<RwLock<...>>` または同等の読多書少構造を想定する。

## 4. ホットリロード方針

### 4.1 基本方針

- ファイル変更検知時に設定を再読込する
- 新設定の構文検証と論理検証が通った場合のみ有効化する
- 検証失敗時は旧設定を維持する
- Bluetooth 設定変更時も同じ差し替え規律を適用する

### 4.2 差し替え単位

最低限、以下を再構築対象とする。

- ルール集合
- 使用キー一覧
- 監視デバイス情報
- Bluetooth 接続設定

仮想デバイスについては以下の方針を推奨する。

- 仮想キーボードは使用キーが変わる場合に再生成する
- 仮想マウスは監視デバイスが変わるか、元デバイス capability が変わる場合に再生成する
- Bluetooth 設定が変わる場合は adapter/device 解決と接続状態を再構築する

推奨:

- 設定変更適用前に新しい状態を丸ごと組み立てる
- 差し替えは一括で行い、中途半端な状態を公開しない

## 5. 競合ルール処理

競合の定義:

- 同一の入力条件に対して複数ルールが存在する状態

解決方針:

- 設定ファイルで後に書かれたルールを採用する
- 先に書かれたルールは無効化扱いとする
- ロード時に警告ログを出力する

警告ログに含めるべき情報:

- 入力条件
- 優先されたルールの位置
- 無効化されたルールの位置

## 6. ルーティング方針

### 6.1 基本方針

- 物理マウスからのイベントはすべて一度プロセスで受ける
- `grab` により元デバイスからのイベントは OS に直接流さない
- リマップ対象のボタンイベントは仮想キーボードへ送る
- 未リマップのボタンイベント、移動イベント、ホイールイベントは仮想マウスへ送る

### 6.2 初期実装での対象

- remap 対象: ボタン押下・解放
- passthrough 対象: 移動、ホイール、未リマップボタン
- 非対象: 複雑なジェスチャ、複数デバイス統合、マクロ DSL

### 6.3 モード切替

- ルール集合はモード単位で保持する
- 現在モードに対応するルール集合だけを実行時参照する
- モード切替入力は通常リマップより先に評価する
- 無効なモード遷移先は設定検証で落とす

## 7. CLI 方針

### 7.1 サブコマンド

- サブコマンド無し起動
  - 常駐監視本体
  - systemd の `ExecStart` で使う
- `monitor`
  - 一時的なキー入力確認やイベント確認に使う
  - grab や常駐前提に固定しない
- `check`
  - 設定の構文検証と論理検証だけを行う
  - 実際の監視や uinput 生成は行わない
- `reload`
  - 実行中プロセスへ再読込要求を送る
  - systemd の `ExecReload` から呼べる形を優先する

### 7.2 設計メモ

- `reload` は SIGHUP 送信または軽量な制御経路のどちらかに寄せる
- 常駐起動だけが長寿命の evdev/uinput 管理を持つ
- `check` / `monitor` / `reload` は短命プロセスとして設計する

## 8. ログ方針

ログレベル案:

- `ERROR`: 起動失敗、設定読込失敗、デバイスオープン失敗、grab 失敗、uinput 生成失敗、Bluetooth adapter / pair / trust / connect 失敗
- `WARN`: 競合ルール、ホットリロード失敗時の旧設定維持
- `INFO`: 起動完了、対象デバイス、grab 成功、設定再読込成功、Bluetooth 接続成功
- `DEBUG`: 詳細イベントトレース、ルーティング判定

## 9. YAML スキーマ案

```yaml
device:
  path: /dev/input/by-id/usb-Example_Mouse-event-mouse
  transport: usb
  name: Logitech G Pro Wireless
  bluetooth:
    auto_pair: true
    auto_trust: true
    auto_connect: true

reload:
  enabled: true
  debounce_ms: 250

remaps:
  - description: right button down -> left meta down
    input:
      type: key
      code: BTN_RIGHT
      value: 1
    output:
      - key: KEY_LEFTMETA
        value: 1

  - description: right button up -> left meta up
    input:
      type: key
      code: BTN_RIGHT
      value: 0
    output:
      - key: KEY_LEFTMETA
        value: 0

modes:
  - name: default
    remaps: []
  - name: fps
    remaps: []

mode_switches:
  - input:
      type: key
      code: BTN_SIDE
      value: 1
    target_mode: fps
```

補足:

- `transport: bluetooth` の場合は `device.name` を必須とする
- `transport: usb` の場合は `device.path` または by-id/name 系 selector を使う
- `device.bluetooth.adapter` や `address` は設定へ持ち込まない

## 10. systemd unit 案

```ini
[Unit]
Description=Mouse remapper with virtual mouse and keyboard routing
After=systemd-udevd.service

[Service]
Type=simple
User=root
ExecStart=/usr/local/bin/mousefold --config /etc/mousefold/config.yaml
ExecReload=/usr/local/bin/mousefold reload --config /etc/mousefold/config.yaml
Restart=on-failure
RestartSec=2
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/etc/mousefold
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

補足:

- `ProtectSystem=strict` を使う場合、読込対象パスの調整が必要
- 設定ファイルを `/etc` に置くなら `ReadOnlyPaths` または `BindReadOnlyPaths` の調整も検討対象

## 11. 実装順序案

1. 常駐起動と `check` / `monitor` / `reload` の CLI 境界を再構成
2. YAML 読込と設定バリデーション
3. 単一マウス入力のオープンと `grab`
4. 元デバイス capability 取得と仮想マウス生成
5. 仮想キーボード生成
6. 単純な 1:1 リマップ
7. 未リマップイベントのパススルー
8. 複数キー出力
9. 競合検出と警告ログ
10. ホットリロード
11. Bluetooth auto pair / trust / connect
12. モード切替
13. systemd unit と運用確認
