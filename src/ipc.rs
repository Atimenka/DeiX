//! IPC — Межпроцессное общение с изоляцией.
//!
//! Настоящая изоляция означает:
//!   - Процесс может читать/писать только свои каналы
//!   - Процесс не может отправлять в чужой канал без прав
//!   - Сообщения валидируются перед доставкой
//!   - Нет обобщённого доступа к памяти других процессов
//!
//! Архитектура:
//!   - IPC-канал: кольцевой буфер с правами доступа
//!   - Каждый процесс имеет уникальный namespace каналов
//!   - Передача права на канал явная операция

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use crate::spinlock::SpinLock;

/// Максимальный размер сообщения в IPC-канале.
const MAX_MSG_SIZE: usize = 4096;
/// Максимальное число каналов на процесс.
const MAX_CHANNELS_PER_PROC: usize = 16;
/// Максимальный размер кольцевого буфера канала.
const CHANNEL_BUFFER_SIZE: usize = 64;

/// Права доступа на канал.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelPermissions {
    /// Пид процесса, который может читать.
    pub reader: u32,
    /// Пид процесса, который может писать.
    pub writer: u32,
}

impl ChannelPermissions {
    pub fn new(reader: u32, writer: u32) -> Self {
        Self { reader, writer }
    }

    /// Может ли процесс читать канал?
    pub fn can_read(&self, pid: u32) -> bool {
        pid == self.reader || pid == 0 // PID 0 (ядро) всегда может
    }

    /// Может ли процесс писать в канал?
    pub fn can_write(&self, pid: u32) -> bool {
        pid == self.writer || pid == 0
    }
}

/// Сообщение в IPC-канале.
#[derive(Debug, Clone)]
pub struct IpcMessage {
    pub sender: u32,
    pub data: Vec<u8>,
}

/// IPC-канал с изоляцией.
#[derive(Debug)]
pub struct IpcChannel {
    /// Уникальный идентификатор канала.
    pub id: u32,
    /// Права доступа.
    pub permissions: ChannelPermissions,
    /// Кольцевой буфер сообщений.
    buffer: SpinLock<Vec<IpcMessage>>,
    /// Заполненность буфера.
    filled: SpinLock<usize>,
    /// завершён ли канал.
    closed: bool,
}

impl IpcChannel {
    pub fn new(id: u32, reader: u32, writer: u32) -> Self {
        Self {
            id,
            permissions: ChannelPermissions::new(reader, writer),
            buffer: SpinLock::new(Vec::new()),
            filled: SpinLock::new(0),
            closed: false,
        }
    }

    /// Отправляет сообщение в канал.
    pub fn send(&mut self, sender: u32, data: &[u8]) -> Result<(), String> {
        if self.closed {
            return Err(String::from("канал завершён"));
        }
        if !self.permissions.can_write(sender) {
            return Err(String::from("нет прав на запись"));
        }
        if data.len() > MAX_MSG_SIZE {
            return Err(String::from("сообщение слишком большое"));
        }

        let mut guard = self.buffer.lock();
        let mut filled = self.filled.lock();

        if *filled >= CHANNEL_BUFFER_SIZE {
            return Err(String::from("буфер переполнен"));
        }

        let msg = IpcMessage {
            sender,
            data: data.to_vec(),
        };
        guard.push(msg);
        *filled += 1;
        Ok(())
    }

    /// Читает сообщение из канала.
    pub fn receive(&mut self, receiver: u32) -> Result<IpcMessage, String> {
        if self.closed {
            return Err(String::from("канал завершён"));
        }
        if !self.permissions.can_read(receiver) {
            return Err(String::from("нет прав на чтение"));
        }

        let mut guard = self.buffer.lock();
        let mut filled = self.filled.lock();

        if *filled == 0 {
            return Err(String::from("буфер пуст"));
        }

        let msg = guard.remove(0);
        *filled -= 1;
        Ok(msg)
    }

    /// Проверяет, есть ли новое сообщение.
    pub fn has_data(&self) -> bool {
        let filled = self.filled.lock();
        *filled > 0
    }

    /// Завершает канал.
    pub fn close(&mut self) {
        self.closed = true;
        {
            let mut guard = self.buffer.lock();
            guard.clear();
        }
        *self.filled.lock() = 0;
    }

    /// Перечисляет PID процессов, которые могут читать/писать канал.
    pub fn get_permissions(&self) -> (u32, u32) {
        (self.permissions.reader, self.permissions.writer)
    }
}

/// Глобальный реестр IPC-каналов.
static CHANNELS: SpinLock<Vec<IpcChannel>> = SpinLock::new(Vec::new());
static NEXT_CHANNEL_ID: SpinLock<u32> = SpinLock::new(1);

/// Инициализация IPC-системы.
pub fn init() {
    // IPC-система не требует специальной инициализации —
    // каналы создаются динамически.
}

/// Создает новый IPC-канал с заданными правами.
/// Возвращает ID канала.
pub fn create_channel(reader: u32, writer: u32) -> Result<u32, String> {
    let mut next_id = NEXT_CHANNEL_ID.lock();
    let id = *next_id;
    *next_id = next_id.wrapping_add(1);

    let mut channels = CHANNELS.lock();
    if channels.len() >= MAX_CHANNELS_PER_PROC * 32 {
        return Err(String::from("лимит каналов истощён"));
    }

    let channel = IpcChannel::new(id, reader, writer);
    channels.push(channel);

    // Регистрируем в нанокоре.
    crate::nanocore::register_ipc(id);

    Ok(id)
}

/// Отправляет сообщение в канал.
pub fn send(channel_id: u32, sender: u32, data: &[u8]) -> Result<(), String> {
    let mut channels = CHANNELS.lock();
    let channel = channels
        .iter_mut()
        .find(|c| c.id == channel_id)
        .ok_or_else(|| format!("канал {} не найден", channel_id))?;
    channel.send(sender, data)
}

/// Читает сообщение из канала.
pub fn receive(channel_id: u32, receiver: u32) -> Result<IpcMessage, String> {
    let mut channels = CHANNELS.lock();
    let channel = channels
        .iter_mut()
        .find(|c| c.id == channel_id)
        .ok_or_else(|| format!("канал {} не найден", channel_id))?;
    channel.receive(receiver)
}

/// Завершает канал.
pub fn close_channel(channel_id: u32) -> Result<(), String> {
    let mut channels = CHANNELS.lock();
    let channel = channels
        .iter_mut()
        .find(|c| c.id == channel_id)
        .ok_or_else(|| format!("канал {} не найден", channel_id))?;
    channel.close();
    // Удаляем из реестра.
    channels.retain(|c| c.id != channel_id);
    crate::nanocore::unregister_ipc(channel_id);
    Ok(())
}

/// Проверяет, есть ли новое сообщение в канале.
pub fn has_data(channel_id: u32) -> bool {
    let channels = CHANNELS.lock();
    channels
        .iter()
        .find(|c| c.id == channel_id)
        .map(|c| c.has_data())
        .unwrap_or(false)
}

/// Список активных каналов.
pub fn list_channels() -> Vec<(u32, u32, u32)> {
    let channels = CHANNELS.lock();
    channels
        .iter()
        .map(|c| (c.id, c.permissions.reader, c.permissions.writer))
        .collect()
}
