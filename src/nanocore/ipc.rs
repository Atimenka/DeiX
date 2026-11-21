//! Nano-Core IPC — встроенная IPC-шина между нано-службами.
//!
//! Кольцевые буферы в статической памяти ядра. Каждый канал —
//! именованная очередь сообщений фиксированного размера. Посылка
//! `send` не блокирует: если буфер переполнен — сообщение пропускается
//! (со счётчиком сбросов), что делает шина устойчивой к всплескам.
//! Защита — SpinLock; каналы регистрируются в статическом регистре.

use crate::spinlock::SpinLock;
use alloc::string::String;
use alloc::vec::Vec;

/// Размер одного сообщения в канале.
pub const MSG_SIZE: usize = 96;
/// Кол-во сообщений в кольцевом буфере канала.
pub const CAPACITY: usize = 64;
/// Максимум каналов в шине.
pub const MAX_CHANNELS: usize = 12;

/// Сообщение IPC.
#[derive(Debug, Clone, Copy)]
pub struct Message {
    /// Текст сообщения.
    pub text: [u8; MSG_SIZE],
    /// Длина текстовой части.
    pub len: usize,
    /// Момент посылки (ms от загрузки).
    pub at_ms: u64,
}

impl Message {
    /// Нулевое сообщение для const-инициализации кольца.
    pub const ZERO: Message = Message {
        text: [0u8; MSG_SIZE],
        len: 0,
        at_ms: 0,
    };

    pub fn from_str(text: &str) -> Self {
        let mut buf = [0u8; MSG_SIZE];
        let bytes = text.as_bytes();
        let len = bytes.len().min(MSG_SIZE - 1);
        buf[..len].copy_from_slice(&bytes[..len]);
        Message {
            text: buf,
            len,
            at_ms: crate::timer::uptime_ms(),
        }
    }

    pub fn as_str(&self) -> &str {
        // Содержимое точно в ASCII (лог и статусы) — безопасно.
        unsafe { core::str::from_utf8_unchecked(&self.text[..self.len]) }
    }
}

/// Один кольцевый буфер канала.
struct Ring {
    buf: [Message; CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
    drops: u64,
}

impl Ring {
    const fn new() -> Self {
        Ring {
            buf: [Message::ZERO; CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
            drops: 0,
        }
    }

    fn push(&mut self, msg: Message) {
        if self.count == CAPACITY {
            self.drops += 1;
            return;
        }
        self.buf[self.tail] = msg;
        self.tail = (self.tail + 1) % CAPACITY;
        self.count += 1;
    }

    fn pop(&mut self) -> Option<Message> {
        if self.count == 0 {
            return None;
        }
        let msg = self.buf[self.head];
        self.head = (self.head + 1) % CAPACITY;
        self.count -= 1;
        Some(msg)
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Последнее сообщение в кольце БЕЗ извлечения (peek).
    fn last(&self) -> Option<Message> {
        if self.is_empty() {
            return None;
        }
        let idx = (self.head + self.count - 1) % CAPACITY;
        Some(self.buf[idx])
    }

    fn drops(&self) -> u64 {
        self.drops
    }
}

/// Именованный канал IPC.
pub struct Channel {
    name: String,
    lock: SpinLock<Ring>,
}

impl Channel {
    pub fn new(name: impl Into<String>) -> Self {
        Channel {
            name: name.into(),
            lock: SpinLock::new(Ring::new()),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Отправить сообщение. Безблокирующая.
    pub fn send(&self, text: &str) {
        let mut ring = self.lock.lock();
        ring.push(Message::from_str(text));
    }

    /// Безблокирующее получение одного сообщения.
    pub fn try_recv(&self) -> Option<Message> {
        let mut ring = self.lock.lock();
        ring.pop()
    }

    /// Количество ждущих сообщений.
    pub fn pending(&self) -> usize {
        self.lock.lock().count
    }

    /// Последнее сообщение без извлечения (peek) — для отчётов.
    pub fn last(&self) -> Option<Message> {
        self.lock.lock().last()
    }

    /// Пуст ли канал.
    pub fn is_empty(&self) -> bool {
        self.lock.lock().is_empty()
    }

    /// Счётчик сброшенных из-за переполнения сообщений.
    #[allow(dead_code)] // API-поверхность для отчётов
    pub fn drops(&self) -> u64 {
        self.lock.lock().drops()
    }
}

/// IPC-шина: статический регистр каналов.
pub struct IpcBus {
    channels: [Option<Channel>; MAX_CHANNELS],
}

impl IpcBus {
    pub const fn new() -> Self {
        IpcBus {
            channels: [None, None, None, None, None, None, None, None, None, None, None, None],
        }
    }

    /// Зарегистрировать или получить канал по имени.
    /// Возвращает None, если регистр исчерпан.
    pub fn channel(&mut self, name: &str) -> Option<&mut Channel> {
        if let Some(existing) = self.channels.iter_mut().find(|c| {
            c.as_ref().map(|ch| ch.name() == name).unwrap_or(false)
        }) {
            return existing.as_mut();
        }
        let slot = self.channels.iter_mut().find(|c| c.is_none())?;
        *slot = Some(Channel::new(name));
        slot.as_mut()
    }

    pub fn send(&self, name: &str, text: &str) {
        for c in self.channels.iter().flatten() {
            if c.name() == name {
                c.send(text);
                return;
            }
        }
    }

    pub fn try_recv(&self, name: &str) -> Option<Message> {
        for c in self.channels.iter().flatten() {
            if c.name() == name {
                return c.try_recv();
            }
        }
        None
    }

    pub fn pending(&self, name: &str) -> usize {
        for c in self.channels.iter().flatten() {
            if c.name() == name {
                return c.pending();
            }
        }
        0
    }

    /// Последнее сообщение в канале без извлечения (peek).
    pub fn last(&self, name: &str) -> Option<Message> {
        for c in self.channels.iter().flatten() {
            if c.name() == name {
                return c.last();
            }
        }
        None
    }

    /// Пуст ли канал.
    pub fn is_empty(&self, name: &str) -> bool {
        for c in self.channels.iter().flatten() {
            if c.name() == name {
                return c.is_empty();
            }
        }
        true
    }
}

/// Глобальная IPC-шина.
pub static IPC: SpinLock<IpcBus> = SpinLock::new(IpcBus::new());

/// Отправить сообщение в канал по имени.
pub fn send(name: &str, text: &str) {
    IPC.lock().send(name, text);
}

/// Безблокирующее получение сообщения из канала.
pub fn try_recv(name: &str) -> Option<Message> {
    IPC.lock().try_recv(name)
}

/// Количество ждущих сообщений в канале.
pub fn pending(name: &str) -> usize {
    IPC.lock().pending(name)
}

/// Последнее сообщение в канале без извлечения (peek).
pub fn last(name: &str) -> Option<Message> {
    IPC.lock().last(name)
}

/// Пуст ли канал.
pub fn is_empty(name: &str) -> bool {
    IPC.lock().is_empty(name)
}

/// Имена зарегистрированных каналов (для отчётов).
pub fn channel_names() -> Vec<String> {
    let bus = IPC.lock();
    bus.channels
        .iter()
        .flatten()
        .map(|c| String::from(c.name()))
        .collect()
}
