---
name: linux-input-pipeline
description: "Linux の evdev/uinput を使って、grab した物理マウス入力を仮想マウスと仮想キーボードへ振り分けるための skill。"
---

# linux-input-pipeline

## 目的
単一の物理マウスからイベントを読み取り、未リマップイベントを仮想マウスへ流し、リマップ対象ボタンを仮想キーボードへ送出する実装を進める。

## この skill を使う場面
- `evdev` 側の入力監視を実装する
- 対象デバイスを `grab` する
- 仮想マウスを生成してパススルーを実装する
- 仮想キーボードを生成してキー送出を実装する
- イベントを mouse / keyboard へ振り分ける
- Linux 依存部を閉じ込める
- Bluetooth マウスの接続完了後に evdev 監視へ移る設計を進める

## 厳守事項
- 監視対象は単一マウスのみ
- 選択した物理マウスは `grab` する
- 初期実装でキーボード変換するのはボタンイベントのみ
- 未リマップの移動、ホイール、通常ボタンは仮想マウスへ流す
- リマップ対象ボタンは仮想マウスへ流さない
- X11/Wayland API に依存しない
- Bluetooth 制御は BlueZ/DBus 境界に閉じ込める

## 実装方針
### 入力
- Bluetooth マウス指定時は `bluer` などでデバイス名から候補を探し、未接続デバイスに対して pair / trust / connect を完了してから進む
- 対象デバイスをオープンする
- capability を取得する
- `grab` を行う
- 読み取ったイベントを正規化する
- ルータへ渡す

### 仮想マウス出力
- 元デバイスの必要 capability をもとに仮想マウスを生成する
- `REL_X`, `REL_Y`, `REL_WHEEL`, `REL_HWHEEL` などの相対イベントを流す
- 未リマップのボタンイベントを流す
- 必要な同期イベントを送る

### 仮想キーボード出力
- 設定から必要キー一覧を集める
- そのキーを登録した仮想キーボードを生成する
- 出力列を順番に送る
- 必要な同期イベントを忘れない

## 推奨 API 形
```rust
pub struct MouseDevice { /* ... */ }

impl MouseDevice {
    pub fn open_and_grab(path: &Path) -> Result<Self, DeviceError>;
    pub fn next_event(&mut self) -> Result<Option<NormalizedMouseEvent>, DeviceError>;
}

pub struct BluetoothConnector { /* ... */ }

impl BluetoothConnector {
    pub async fn ensure_connected(selector: &BluetoothSelector) -> Result<(), BluetoothError>;
}

pub struct VirtualMouse { /* ... */ }

impl VirtualMouse {
    pub fn build_from_source_caps(caps: &SourceMouseCapabilities) -> Result<Self, MouseError>;
    pub fn emit(&mut self, event: &NormalizedMouseEvent) -> Result<(), MouseError>;
}

pub struct VirtualKeyboard { /* ... */ }

impl VirtualKeyboard {
    pub fn build(keys: &[KeyCode]) -> Result<Self, KeyboardError>;
    pub fn emit(&mut self, sequence: &[KeyStroke]) -> Result<(), KeyboardError>;
}
```

## 入力イベントの扱い
初期実装では、内部で次のような正規化を推奨する。

```rust
pub enum NormalizedMouseEvent {
    Button { code: MouseButtonCode, value: ButtonValue },
    Relative { code: RelativeCode, value: i32 },
    SyncReport,
    OtherIgnored,
}
```

## ルーティング方針
- ボタンイベントが remap ルールに一致すればキーボードへ
- 一致しなければ仮想マウスへ
- 相対移動とホイールは仮想マウスへ
- 不要イベントは無視

## 注意点
- `grab` 失敗時は即座に原因を返す
- Bluetooth pair / trust / connect のどこで失敗したか分かるようにする
- 仮想マウスと仮想キーボードの二重送出を避ける
- `SYN_REPORT` はイベントまとまりに合わせて扱う
- ボタン解放漏れやホイール欠落がないか確認する
- capability が不足した仮想マウスを作らない

## デバッグ時に確認すること
- 対象デバイスが本当にマウスか
- Bluetooth マウスが BlueZ 上で名前一致し、pair / trust / connect 済みか
- `grab` されているか
- リマップ対象ボタンだけ抑止できているか
- 未リマップボタンが仮想マウスへ届いているか
- 移動とホイールが途切れていないか
- 仮想マウス/キーボードが正しく見えているか

## 最小受け入れライン
- 単一マウスのイベントを読める
- 物理デバイスを `grab` できる
- 仮想マウスと仮想キーボードを作れる
- 未リマップイベントを仮想マウスへ流せる
- リマップ対象イベントをキーボードへ変換できる
- 二重入力にならない

## 出力の期待形
この skill を使ったときは、次を返す。

- 変更対象ファイル
- 入力処理フロー
- 仮想マウス出力フロー
- 仮想キーボード出力フロー
- ルーティング判定
- エラー処理方針
- 手動確認コマンド例
