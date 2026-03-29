use evdev::{
    AttributeSet, Device, EventSummary, EventType, InputEvent, KeyCode, uinput::VirtualDevice,
};
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 1. 物理マウスデバイスの特定 (環境に合わせてパスを変更してください)
    // /dev/input/by-id/ 等で固定すると安定します
    let device_path = "/dev/input/event13";
    let mut device = Device::open(device_path)?;

    // 2. 仮想出力デバイス（uinput）の作成
    // with_keys() は &AttributeSetRef<KeyCode> を受け取るため、
    // AttributeSet を構築してから Deref で渡す
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::KEY_A);
    keys.insert(KeyCode::KEY_ENTER);
    keys.insert(KeyCode::KEY_BACKSPACE);
    keys.insert(KeyCode::KEY_LEFTMETA);

    let mut v_device = VirtualDevice::builder()?
        .name("Remapped Mouse Virtual Device")
        .with_keys(&keys)? // リマップ先のキーを登録
        .build()?;

    println!("Monitoring device: {}", device.name().unwrap_or("Unknown"));

    // 3. イベントループ
    loop {
        for event in device.fetch_events()? {
            match event.destructure() {
                // 例: マウスのサイドボタン(BTN_SIDE)を検知
                EventSummary::Key(_, KeyCode::BTN_RIGHT, value) => {
                    // Enterキーとして送信 (valueは1でプレス、0でリリース)
                    // let new_event = InputEvent::new(EventType::KEY.0, KeyCode::KEY_ENTER.0, value);
                    if value == 1 {
                        let new_event = [InputEvent::new(
                            EventType::KEY.0,
                            KeyCode::KEY_LEFTMETA.0,
                            1,
                        )];
                        v_device.emit(&new_event)?;
                    } else {
                        let new_event =
                            InputEvent::new(EventType::KEY.0, KeyCode::KEY_LEFTMETA.0, 0);
                        v_device.emit(&[new_event])?;
                    }
                }
                // それ以外のイベントはスルー、または必要に応じて複製
                _ => {}
            }
        }
    }
}
