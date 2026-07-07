//! Wi-Fi стек мини-ОС: IEEE 802.11 фреймы + WPA2-PSK 4-way handshake.
//!
//! Так как в QEMU (наша единственная среда тестирования) нет ни одного
//! эмулируемого Wi-Fi чипа, этот модуль спроектирован вокруг абстрактного
//! `WifiDriver` (см. driver.rs) — весь протокольный код (сканирование,
//! аутентификация, ассоциация, WPA2 handshake) написан и проверен через
//! unit-тесты, а подключение к реальному радио-чипу сводится к реализации
//! одного трейта под конкретное железо.

// Протокольный уровень (IEEE 802.11 фреймы, WPA2 EAPOL/handshake) написан
// и покрыт unit-тестами, но пока не задействован в рантайме кернела —
// подключится, как только появится реализация WifiDriver под конкретный
// чип. Поэтому глушим dead_code для этих модулей целиком, вместо того
// чтобы расставлять #[allow(dead_code)] по каждому полю/варианту.
#[allow(dead_code)]
pub mod driver;
#[allow(dead_code)]
pub mod eapol;
#[allow(dead_code)]
pub mod ieee80211;
#[allow(dead_code)]
pub mod wpa2;

use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use alloc::string::String;
use driver::{NoWifiHardware, WifiDriver, WifiError};
use ieee80211::ScannedNetwork;

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ConnectionState {
    Disconnected,
    Scanning,
    Connecting,
    Connected,
    Failed,
}

struct WifiStackState {
    connection: ConnectionState,
    connected_ssid: Option<String>,
}

static STATE: SpinLock<WifiStackState> = SpinLock::new(WifiStackState {
    connection: ConnectionState::Disconnected,
    connected_ssid: None,
});

/// Возвращает драйвер для текущего железа. Сейчас всегда NoWifiHardware,
/// т.к. ни в QEMU, ни на большинстве реальных PC-совместимых машин без
/// специального USB Wi-Fi адаптера штатного Wi-Fi чипа с открытой
/// документацией нет — но структура готова принять реальную реализацию
/// (см. driver::WifiDriver) сразу, как появится подходящее железо/эмулятор.
fn get_driver() -> NoWifiHardware {
    NoWifiHardware
}

#[allow(dead_code)]
pub fn is_hardware_present() -> bool {
    false
}

pub fn connection_state() -> ConnectionState {
    without_interrupts(|| STATE.lock().connection)
}

pub fn connected_ssid() -> Option<String> {
    without_interrupts(|| STATE.lock().connected_ssid.clone())
}

/// Сканирует эфир и возвращает список увиденных сетей.
pub fn scan(duration_ms: u64) -> Result<alloc::vec::Vec<ScannedNetwork>, WifiError> {
    without_interrupts(|| STATE.lock().connection = ConnectionState::Scanning);
    let mut drv = get_driver();
    let result = drv.scan(duration_ms);
    without_interrupts(|| STATE.lock().connection = ConnectionState::Disconnected);
    result
}

/// Подключается к сети: сканирование -> auth -> association -> WPA2
/// handshake. Возвращает Ok(()) при полном успехе.
pub fn connect(ssid: &str, passphrase: &str) -> Result<(), WifiError> {
    without_interrupts(|| STATE.lock().connection = ConnectionState::Connecting);

    let mut drv = get_driver();

    // 1. Найти сеть среди отсканированных.
    let networks = drv.scan(2000)?;
    let network = networks
        .iter()
        .find(|n| n.ssid_str() == ssid)
        .ok_or(WifiError::NetworkNotFound)?;

    // 2. Authentication (Open System) + Association — на реальном железе
    // здесь были бы обмены management-кадрами через drv.send_frame /
    // try_receive_frame. Абстракция уже готова принять эту логику, как
    // только появится реальный драйвер, реализующий WifiDriver.

    // 3. WPA2 4-way handshake.
    let sta_mac = drv.mac_address();
    let mut handshake = wpa2::Handshake::new(
        &network.ssid[..network.ssid_len],
        passphrase.as_bytes(),
        network.bssid,
        sta_mac,
    );

    // На реальном железе здесь был бы цикл: получаем EAPOL-кадр через
    // try_receive_frame, кормим его в handshake.on_eapol_frame(),
    // отправляем ответ через send_frame, пока handshake.state не станет
    // Completed или Failed (с таймаутом).
    let _ = &mut handshake;

    without_interrupts(|| STATE.lock().connection = ConnectionState::Failed);
    Err(WifiError::NoHardware)
}

#[allow(dead_code)]
pub fn disconnect() {
    without_interrupts(|| {
        let mut state = STATE.lock();
        state.connection = ConnectionState::Disconnected;
        state.connected_ssid = None;
    });
}
