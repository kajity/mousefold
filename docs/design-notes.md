# ex-g-pro-remapper 設計メモ

## 1. 設計方針

初期実装は、単一マウス入力を監視し、YAML 設定に基づいて仮想キーボードからキーイベントを送出する常駐デーモンとする。

実装上の複雑性を抑えるため、初期スコープでは以下を前提とする。

- 入力対象は単一デバイス
- 入力イベントはボタンイベントのみ
- 設定フォーマットは YAML
- systemd 管理下で root 実行
- 設定変更はホットリロード対応

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
- `evdev` イベント受信
- 対象外イベントの除外

主要処理:

- `open_mouse(path) -> Device`
- `next_event() -> MouseEvent`

### 2.3 `virtual_keyboard`

責務:

- `uinput` 仮想キーボード生成
- キー送出

主要処理:

- `build(keys) -> VirtualKeyboard`
- `emit(sequence)`

### 2.4 `mapper`

責務:

- 入力イベントとルールの照合
- 対応する出力イベント列の決定

主要処理:

- `resolve(event, rules) -> Option<OutputSequence>`

補足:

- 競合ルールはロード時に解決方針を確定する
- 実行時は「最後に有効となるルール」のみ参照すると単純化できる

### 2.5 `reload`

責務:

- 設定ファイル変更監視
- 新設定の読込と差し替え
- 失敗時のロールバック

主要処理:

- `watch(path)`
- `reload_if_changed()`

## 3. 推奨ランタイム構成

単一プロセス内で以下のタスクを持つ構成を推奨する。

1. 入力イベント受信タスク
2. 設定ファイル監視タスク
3. 終了シグナル監視タスク

共有状態:

- 現在有効な設定
- 現在有効なルール集合
- 仮想キーボードハンドル

共有状態の更新は `Arc<RwLock<...>>` または同等の読多書少構造を想定する。

## 4. ホットリロード方針

### 4.1 基本方針

- ファイル変更検知時に設定を再読込する
- 新設定の構文検証と論理検証が通った場合のみ有効化する
- 検証失敗時は旧設定を維持する

### 4.2 差し替え単位

最低限、以下を再構築対象とする。

- ルール集合
- 使用キー一覧

仮想キーボードについては以下の 2 案がある。

案 A:

- 起動時に広めのキー集合を登録して再生成を避ける

案 B:

- 設定変更時に仮想キーボードを再生成する

推奨:

- 初期実装は案 B の方が仕様整合性は高い
- ただし一時的なデバイス切替の影響を評価する必要がある

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

## 6. ログ方針

ログレベル案:

- `ERROR`: 起動失敗、設定読込失敗、デバイスオープン失敗、uinput 生成失敗
- `WARN`: 競合ルール、ホットリロード失敗時の旧設定維持
- `INFO`: 起動完了、対象デバイス、設定再読込成功
- `DEBUG`: 詳細イベントトレース

## 7. YAML スキーマ案

```yaml
device:
  path: /dev/input/by-id/usb-Example_Mouse-event-mouse

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
```

## 8. systemd unit 案

```ini
[Unit]
Description=Mouse to keyboard remapper daemon
After=systemd-udevd.service

[Service]
Type=simple
User=root
ExecStart=/usr/local/bin/ex-g-pro-remapper --config /etc/ex-g-pro-remapper/config.yaml
Restart=on-failure
RestartSec=2
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/etc/ex-g-pro-remapper
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

補足:

- `ProtectSystem=strict` を使う場合、読込対象パスの調整が必要
- 設定ファイルを `/etc` に置くなら `ReadOnlyPaths` または `BindReadOnlyPaths` の調整も検討対象

## 9. 実装順序案

1. YAML 読込と設定バリデーション
2. 単一マウス入力の監視
3. 仮想キーボード生成
4. 単純な 1:1 リマップ
5. 複数キー出力
6. 競合検出と警告ログ
7. ホットリロード
8. systemd unit と運用確認
