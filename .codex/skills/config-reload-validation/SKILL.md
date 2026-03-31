---
name: config-reload-validation
description: "YAML 設定、構文検証、論理検証、競合解決、ホットリロード、ロールバックを実装するための skill。"
---

# config-reload-validation

## 目的
設定ファイルまわりを壊れにくくする。
この skill は、YAML の読み込みだけでなく、論理検証、競合解決、ホットリロード成功時の差し替え、失敗時の旧設定維持までを対象にする。

## この skill を使う場面
- YAML スキーマを決める
- `serde` 用データ構造を作る
- ルールの論理検証を実装する
- 競合ルールを検出する
- ホットリロードと debounce を実装する
- 再読込失敗時のロールバックを実装する

## 設定仕様の基本
初期実装の設定は以下を表現できること。

- 対象デバイス
- Bluetooth 接続設定
- リロード設定
- モード定義
- モード切替定義
- リマップルール
- 入力条件
- 出力イベント列
- 押下時と解放時の動作

## 推奨 YAML 例
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

## 検証の段階
### 1. 構文検証
- YAML として読めるか
- 必須項目があるか
- 型が合っているか

### 2. 意味検証
- デバイス指定が空でないか
- `transport: bluetooth` のとき name があるか
- Bluetooth 自動接続フラグが矛盾していないか
- モード名が重複していないか
- モード切替先が存在するか
- `input.type` が初期実装で許可した型か
- ボタンコードが許可集合にあるか
- `value` が許容範囲か
- `output` が空でないか
- キーコードが許可集合にあるか

### 3. 競合検証
- 同一入力条件の重複を検出する
- 後勝ちに正規化する
- 警告情報を保持する

## 推奨データ構造
```rust
pub struct ValidationResult {
    pub compiled: CompiledRules,
    pub warnings: Vec<ConfigWarning>,
}

pub enum ConfigWarning {
    ShadowedRule {
        input: String,
        preferred_index: usize,
        shadowed_index: usize,
    },
}
```

## 競合解決ルール
- 同一入力条件への複数定義は競合
- 後に記載されたルールを採用
- 先のルールは無効化
- 起動時または再読込時に `WARN`
- ログには入力条件とルール位置を含める

## ホットリロード方針
### 成功条件
- ファイル変更を検知
- 新設定を再読込
- 構文検証に通る
- 論理検証に通る
- 必要なら新しい仮想キーボードを再構築
- 監視デバイスが変わるなら物理デバイスと仮想マウスも再構築する
- Bluetooth 設定が変わるなら接続状態と監視デバイス解決を再構築する
- 共有状態を安全に差し替える

### 失敗時
- 旧設定を維持
- 入力変換処理は継続
- `WARN` または `ERROR` を出す
- 原因をログに含める

## 差し替え単位
最低限、次を再構築対象にする。

- ルール集合
- 使用キー一覧
- 必要に応じて仮想キーボード
- 監視デバイス変更時は物理デバイスと仮想マウス
- Bluetooth 設定変更時は device 名解決と接続状態

## 実装上の注意
- editor save による短時間多発イベントを考慮する
- debounce を入れる
- 読み込み途中の不完全ファイルを拾う可能性を意識する
- 差し替え前に新設定一式を完全に組み立てる
- 中途半端な状態を共有しない

## テスト項目
### Unit tests
- 正常 YAML を読める
- 欠落項目で失敗する
- 不正キーコードで失敗する
- 出力空配列で失敗する
- 競合ルールが後勝ちになる

### Integration tests
- 正常な再読込で新ルールへ切り替わる
- 無効な再読込で旧ルールが維持される
- 新旧で必要キーが異なる場合に安全に差し替わる
- 監視デバイス変更時に再構築できる
- Bluetooth 設定変更時に接続経路を再構築できる
- モード切替定義の変更時に安全に差し替わる

## 出力の期待形
この skill を使ったときは、次を返す。

- YAML スキーマ案
- Rust の設定型
- 検証ロジック
- 競合解決ロジック
- リロード処理手順
- 追加すべきテスト
