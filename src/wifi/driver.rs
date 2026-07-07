//! Аппаратно-независимый интерфейс Wi-Fi драйвера.
//!
//! У нас нет ни одного реального Wi-Fi чипа для тестирования (QEMU умеет
//! эмулировать только проводные карты — RTL8139, e1000, virtio-net), но
//! структура рассчитана так, чтобы под конкретное железо (например,
//! Realtek RTL8188/RTL8192, Atheros AR9271, Intel iwlwifi) можно было
//! реализовать этот трейт и сразу получить рабочий стек сканирования +
//! WPA2-подключения поверх уже написанной и проверенной логики.
//!
//! Основные различия с проводной картой (rtl8139.rs):
//!   - в эфир уходят не просто Ethernet-кадры, а кадры IEEE 802.11 со
//!     своими управляющими типами (Beacon/Probe/Auth/Association)
//!   - перед передачей данных нужно провести handshake на management-
//!     уровне (сканирование -> аутентификация -> ассоциация -> WPA2)
//!   - после успешного подключения данные всё равно инкапсулируются в
//!     Data-кадры 802.11, а полезная нагрузка внутри остаётся тем же
//!     Ethernet/IP, что мы уже реализовали в net::

use crate::wifi::ieee80211::{MacAddr, ScannedNetwork};
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiError {
    NoHardware,
    NotScanned,
    NetworkNotFound,
    AuthenticationFailed,
    AssociationFailed,
    HandshakeTimeout,
    HandshakeMicMismatch,
    Timeout,
}

/// Реализуется конкретным драйвером радио-чипа. Все операции — блокирующие
/// (крутятся в hlt/опрашивают карту до готовности), что соответствует
/// остальному стилю нашего мини-ядра без полноценного асинхронного рантайма.
pub trait WifiDriver {
    /// MAC-адрес нашей карты.
    fn mac_address(&self) -> MacAddr;

    /// Просканировать эфир и вернуть список увиденных сетей (Beacon/Probe
    /// Response) за отведённое время.
    fn scan(&mut self, duration_ms: u64) -> Result<Vec<ScannedNetwork>, WifiError>;

    /// Отправить один готовый (уже собранный) кадр 802.11 в эфир.
    fn send_frame(&mut self, frame: &[u8]) -> Result<(), WifiError>;

    /// Неблокирующая попытка получить один кадр из приёмного буфера карты.
    fn try_receive_frame(&mut self, buf: &mut [u8]) -> Option<usize>;
}

/// Заглушка-реализация для случая, когда никакого реального Wi-Fi чипа не
/// найдено (что и есть в QEMU) — CLI использует её, чтобы честно сообщать
/// пользователю "нет железа", а не падать или зависать.
pub struct NoWifiHardware;

impl WifiDriver for NoWifiHardware {
    fn mac_address(&self) -> MacAddr {
        [0; 6]
    }

    fn scan(&mut self, _duration_ms: u64) -> Result<Vec<ScannedNetwork>, WifiError> {
        Err(WifiError::NoHardware)
    }

    fn send_frame(&mut self, _frame: &[u8]) -> Result<(), WifiError> {
        Err(WifiError::NoHardware)
    }

    fn try_receive_frame(&mut self, _buf: &mut [u8]) -> Option<usize> {
        None
    }
}
